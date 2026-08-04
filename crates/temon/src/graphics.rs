use std::{
    collections::{HashMap, VecDeque},
    ffi::{c_int, c_ulong},
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
    sync::Arc,
};

use crate::{
    Size, decode_base64,
    grid::{Grid, GridEffect},
};

const MAX_IMAGE_BYTES: usize = 128 * 1024 * 1024;
pub(crate) const MAX_COMMAND_BYTES: usize = MAX_IMAGE_BYTES / 3 * 4 + 4096;
const MAX_DIMENSION: u32 = 32_768;
const MAX_PIXELS: u64 = (MAX_IMAGE_BYTES / 4) as u64;
const MAX_PLACEMENTS: usize = 4096;

#[derive(Clone, Debug)]
pub(crate) struct GraphicsCommand {
    control: Vec<(char, String)>,
    payload: Vec<u8>,
    oversized: bool,
}

impl GraphicsCommand {
    pub(crate) fn parse(bytes: Vec<u8>, oversized: bool) -> Self {
        let separator = bytes.iter().position(|byte| *byte == b';');
        let (control, payload) = match separator {
            Some(index) => (&bytes[..index], bytes[index + 1..].to_vec()),
            None => (bytes.as_slice(), Vec::new()),
        };
        let control = String::from_utf8_lossy(control)
            .split(',')
            .filter_map(|field| {
                let (key, value) = field.split_once('=')?;
                let mut chars = key.chars();
                let key = chars.next()?;
                (chars.next().is_none() && key.is_ascii()).then(|| (key, value.to_string()))
            })
            .collect();
        Self {
            control,
            payload,
            oversized,
        }
    }

    fn value(&self, key: char) -> Option<&str> {
        self.control
            .iter()
            .rev()
            .find_map(|(candidate, value)| (*candidate == key).then_some(value.as_str()))
    }

    fn char_value(&self, key: char) -> Option<char> {
        let mut chars = self.value(key)?.chars();
        let value = chars.next()?;
        chars.next().is_none().then_some(value)
    }

    fn u32_value(&self, key: char) -> Option<u32> {
        self.value(key)?.parse().ok()
    }

    fn i32_value(&self, key: char) -> Option<i32> {
        self.value(key)?.parse().ok()
    }
}

#[derive(Clone, Debug)]
struct StoredImage {
    png: Arc<[u8]>,
    width: u32,
    height: u32,
    generation: u64,
}

#[derive(Clone, Debug)]
struct Placement {
    serial: u64,
    alternate: bool,
    image_id: u32,
    placement_id: u32,
    anchor_line: i64,
    col: usize,
    source_x: u32,
    source_y: u32,
    source_width: u32,
    source_height: u32,
    display_cols: Option<u32>,
    display_rows: Option<u32>,
    occupied_cols: u32,
    occupied_rows: u32,
    x_offset: u32,
    y_offset: u32,
    z_index: i32,
}

#[derive(Clone, Debug)]
struct PendingUpload {
    command: GraphicsCommand,
    decoded: Vec<u8>,
    cursor_col: usize,
    cursor_row: usize,
    history_size: usize,
    size: Size,
    alternate: bool,
}

#[derive(Clone, Debug)]
pub struct GraphicsRenderPlacement {
    pub placement_serial: u64,
    pub image_id: u32,
    pub placement_id: u32,
    pub png: Arc<[u8]>,
    pub image_width: u32,
    pub image_height: u32,
    pub image_generation: u64,
    pub viewport_row: i32,
    pub col: usize,
    pub source_x: u32,
    pub source_y: u32,
    pub source_width: u32,
    pub source_height: u32,
    pub display_cols: Option<u32>,
    pub display_rows: Option<u32>,
    pub occupied_cols: u32,
    pub occupied_rows: u32,
    pub x_offset: u32,
    pub y_offset: u32,
    pub z_index: i32,
}

#[derive(Debug, Default)]
pub(crate) struct GraphicsState {
    images: HashMap<u32, StoredImage>,
    placements: Vec<Placement>,
    insertion_order: VecDeque<u32>,
    pending: Option<PendingUpload>,
    next_anonymous_id: u32,
    stored_bytes: usize,
    next_generation: u64,
    next_serial: u64,
}

#[derive(Default)]
pub(crate) struct ApplyResult {
    pub replies: Vec<u8>,
    pub changed: bool,
}

impl GraphicsState {
    pub(crate) fn apply_grid_effect(&mut self, effect: GridEffect) -> bool {
        match effect {
            GridEffect::ScrollUp {
                alternate,
                top,
                bottom,
                count,
                recorded_history,
                history_before,
                history_after,
            } => {
                if !alternate && recorded_history {
                    let dropped = history_before
                        .saturating_add(count)
                        .saturating_sub(history_after);
                    self.rebase_primary_history(dropped)
                } else {
                    self.scroll_region_up(alternate, top, bottom, count, history_before)
                }
            }
            GridEffect::ScrollDown {
                alternate,
                top,
                bottom,
                count,
                history_size,
            } => self.scroll_region_down(alternate, top, bottom, count, history_size),
            GridEffect::EnteredAlternate => self.clear_screen_placements(true),
            GridEffect::ClearViewport {
                alternate,
                history_size,
                rows,
                cols,
            } => self.clear_viewport(alternate, history_size, rows, cols),
            GridEffect::RebaseHistory { dropped } => self.rebase_primary_history(dropped),
            GridEffect::Reflow { alternate } => self.clear_screen_placements(alternate),
            GridEffect::Reset => {
                let changed = !self.images.is_empty()
                    || !self.placements.is_empty()
                    || self.pending.is_some();
                self.clear();
                changed
            }
        }
    }

    pub(crate) fn apply(
        &mut self,
        command: GraphicsCommand,
        grid: &mut Grid,
        size: Size,
    ) -> ApplyResult {
        if command.oversized {
            self.pending = None;
            return failure(&command, "EFBIG:image command exceeds storage limit");
        }
        let action = command.char_value('a').unwrap_or('t');
        if action == 'd' {
            return self.delete(&command, grid);
        }
        if action == 'p' {
            return self.put(&command, grid, size);
        }
        if !matches!(action, 't' | 'T' | 'q') {
            return failure(&command, "EINVAL:unsupported graphics action");
        }

        let decoded = match decode_base64(&command.payload) {
            Some(decoded) if decoded.len() <= MAX_IMAGE_BYTES => decoded,
            Some(_) => {
                self.pending = None;
                return failure(&command, "EFBIG:image payload exceeds storage limit");
            }
            None => {
                self.pending = None;
                return failure(&command, "EINVAL:invalid base64 payload");
            }
        };
        let more = command.u32_value('m').unwrap_or(0) == 1;
        if let Some(mut pending) = self.pending.take() {
            if pending.decoded.len().saturating_add(decoded.len()) > MAX_IMAGE_BYTES {
                return failure(
                    &pending.command,
                    "EFBIG:image payload exceeds storage limit",
                );
            }
            pending.decoded.extend_from_slice(&decoded);
            if more {
                self.pending = Some(pending);
                return ApplyResult::default();
            }
            let mut first = pending.command.clone();
            for (key, value) in command.control {
                if key != 'm' {
                    first.control.push((key, value));
                }
            }
            let decoded = std::mem::take(&mut pending.decoded);
            return self.finish_upload(first, decoded, pending, grid);
        }

        let (cursor_col, cursor_row) = grid.cursor_position();
        let pending = PendingUpload {
            command: command.clone(),
            decoded: Vec::new(),
            cursor_col,
            cursor_row,
            history_size: grid.history_size(),
            size,
            alternate: grid.alternate_screen_mode(),
        };
        if more {
            self.pending = Some(PendingUpload { decoded, ..pending });
            return ApplyResult::default();
        }
        self.finish_upload(command, decoded, pending, grid)
    }

    fn finish_upload(
        &mut self,
        command: GraphicsCommand,
        decoded: Vec<u8>,
        context: PendingUpload,
        grid: &mut Grid,
    ) -> ApplyResult {
        let data = match resolve_transmission_data(&command, decoded) {
            Ok(data) => data,
            Err(error) => return failure(&command, &error),
        };
        let data = match command.char_value('o') {
            None => data,
            Some('z') => match decompress_zlib(&command, &data) {
                Ok(data) => data,
                Err(error) => return failure(&command, &error),
            },
            Some(_) => return failure(&command, "EINVAL:unsupported compression"),
        };
        let (png, width, height) = match normalize_image(&command, &data) {
            Ok(image) => image,
            Err(error) => return failure(&command, &error),
        };
        if command.char_value('a').unwrap_or('t') == 'q' {
            return success(&command, false);
        }

        let requested_id = command.u32_value('i').unwrap_or(0);
        let image_id = if requested_id == 0 {
            self.allocate_anonymous_id()
        } else {
            requested_id
        };
        self.remove_image(image_id);
        self.next_generation = self.next_generation.wrapping_add(1).max(1);
        let byte_len = png.len();
        self.images.insert(
            image_id,
            StoredImage {
                png: Arc::from(png),
                width,
                height,
                generation: self.next_generation,
            },
        );
        self.insertion_order.push_back(image_id);
        self.stored_bytes = self.stored_bytes.saturating_add(byte_len);
        if !self.enforce_quota(Some(image_id)) {
            self.remove_image(image_id);
            return failure(&command, "ENOSPC:image storage quota exceeded");
        }

        if command.char_value('a').unwrap_or('t') == 'T' {
            let advance = match self.add_placement(
                image_id,
                &command,
                context.cursor_col,
                context.cursor_row,
                context.history_size,
                context.size,
                context.alternate,
            ) {
                Ok(advance) => advance,
                Err(error) => {
                    self.remove_image(image_id);
                    return failure(&command, &error);
                }
            };
            if let Some((cols, rows)) = advance {
                grid.cursor_forward(cols as usize);
                for _ in 0..rows.min(u32::from(context.size.rows)) {
                    grid.line_feed();
                }
            }
        }
        success(&command, true)
    }

    fn put(&mut self, command: &GraphicsCommand, grid: &mut Grid, size: Size) -> ApplyResult {
        let image_id = command.u32_value('i').unwrap_or(0);
        if image_id == 0 || !self.images.contains_key(&image_id) {
            return failure(command, "ENOENT:image id not found");
        }
        let (cursor_col, cursor_row) = grid.cursor_position();
        match self.add_placement(
            image_id,
            command,
            cursor_col,
            cursor_row,
            grid.history_size(),
            size,
            grid.alternate_screen_mode(),
        ) {
            Ok(advance) => {
                if let Some((cols, rows)) = advance {
                    grid.cursor_forward(cols as usize);
                    for _ in 0..rows.min(u32::from(size.rows)) {
                        grid.line_feed();
                    }
                }
                success(command, true)
            }
            Err(error) => failure(command, &error),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn add_placement(
        &mut self,
        image_id: u32,
        command: &GraphicsCommand,
        cursor_col: usize,
        cursor_row: usize,
        history_size: usize,
        size: Size,
        alternate: bool,
    ) -> Result<Option<(u32, u32)>, String> {
        if command.u32_value('U').unwrap_or(0) == 1 {
            return Err("EINVAL:Unicode placeholder placements are not supported".into());
        }
        let image = self
            .images
            .get(&image_id)
            .ok_or_else(|| "ENOENT:image id not found".to_string())?;
        let source_x = command.u32_value('x').unwrap_or(0).min(image.width);
        let source_y = command.u32_value('y').unwrap_or(0).min(image.height);
        let source_width = command
            .u32_value('w')
            .unwrap_or(image.width.saturating_sub(source_x))
            .min(image.width.saturating_sub(source_x));
        let source_height = command
            .u32_value('h')
            .unwrap_or(image.height.saturating_sub(source_y))
            .min(image.height.saturating_sub(source_y));
        if source_width == 0 || source_height == 0 {
            return Err("EINVAL:empty source rectangle".into());
        }

        let cell_width = size.cell_width.max(1.0);
        let cell_height = size.cell_height.max(1.0);
        let requested_cols = command.u32_value('c').filter(|value| *value > 0);
        let requested_rows = command.u32_value('r').filter(|value| *value > 0);
        let available_cols = u32::from(size.cols)
            .saturating_sub(u32::try_from(cursor_col).unwrap_or(u32::MAX))
            .max(1);
        let mut placed_source_width = source_width;
        let (display_cols, display_rows) = match (requested_cols, requested_rows) {
            (Some(cols), Some(rows)) => (Some(cols), Some(rows)),
            (Some(cols), None) => {
                let height = cols as f32 * cell_width * source_height as f32 / source_width as f32;
                (
                    Some(cols),
                    Some((height / cell_height).ceil().max(1.0) as u32),
                )
            }
            (None, Some(rows)) => {
                let width = rows as f32 * cell_height * source_width as f32 / source_height as f32;
                (
                    Some((width / cell_width).ceil().max(1.0) as u32),
                    Some(rows),
                )
            }
            (None, None) => {
                let available_width = available_cols as f32 * cell_width;
                if source_width as f32 > available_width {
                    placed_source_width = available_width.floor().max(1.0) as u32;
                }
                (
                    Some((placed_source_width as f32 / cell_width).ceil().max(1.0) as u32),
                    Some((source_height as f32 / cell_height).ceil().max(1.0) as u32),
                )
            }
        };
        let occupied_cols = display_cols.unwrap_or(1).min(available_cols);
        let occupied_rows = display_rows.unwrap_or(1);
        let placement_id = command.u32_value('p').unwrap_or(0);
        if placement_id != 0 {
            self.placements.retain(|placement| {
                placement.alternate != alternate
                    || placement.image_id != image_id
                    || placement.placement_id != placement_id
            });
        }
        self.next_serial = self.next_serial.wrapping_add(1).max(1);
        self.placements.push(Placement {
            serial: self.next_serial,
            alternate,
            image_id,
            placement_id,
            anchor_line: if alternate {
                i64::try_from(cursor_row).unwrap_or(i64::MAX)
            } else {
                i64::try_from(history_size)
                    .unwrap_or(i64::MAX)
                    .saturating_add(i64::try_from(cursor_row).unwrap_or(i64::MAX))
            },
            col: cursor_col,
            source_x,
            source_y,
            source_width: placed_source_width,
            source_height,
            display_cols,
            display_rows,
            occupied_cols,
            occupied_rows,
            x_offset: command.u32_value('X').unwrap_or(0),
            y_offset: command.u32_value('Y').unwrap_or(0),
            z_index: command.i32_value('z').unwrap_or(0),
        });
        if self.placements.len() > MAX_PLACEMENTS {
            let overflow = self.placements.len() - MAX_PLACEMENTS;
            self.placements.drain(..overflow);
        }
        Ok((command.u32_value('C').unwrap_or(0) == 0).then_some((occupied_cols, occupied_rows)))
    }

    fn delete(&mut self, command: &GraphicsCommand, grid: &Grid) -> ApplyResult {
        self.pending = None;
        let selector = command.char_value('d').unwrap_or('a');
        let free_data = selector.is_ascii_uppercase();
        let selector = selector.to_ascii_lowercase();
        let alternate = grid.alternate_screen_mode();
        let before = self.placements.len();
        match selector {
            'a' => self
                .placements
                .retain(|placement| placement.alternate != alternate),
            'i' => {
                let image_id = command.u32_value('i').unwrap_or(0);
                let placement_id = command.u32_value('p').unwrap_or(0);
                self.placements.retain(|placement| {
                    placement.alternate != alternate
                        || placement.image_id != image_id
                        || (placement_id != 0 && placement.placement_id != placement_id)
                });
                if free_data && !self.placements.iter().any(|p| p.image_id == image_id) {
                    self.remove_image(image_id);
                }
            }
            'c' => {
                let (col, row) = grid.cursor_position();
                let line = if alternate {
                    row as i64
                } else {
                    grid.history_size() as i64 + row as i64
                };
                self.placements.retain(|placement| {
                    placement.alternate != alternate || !placement_contains(placement, line, col)
                });
            }
            'p' | 'q' => {
                let col = command.u32_value('x').unwrap_or(1).saturating_sub(1) as usize;
                let row = command.u32_value('y').unwrap_or(1).saturating_sub(1) as i64
                    + if alternate {
                        0
                    } else {
                        grid.history_size() as i64
                    };
                let z_index = command.i32_value('z');
                self.placements.retain(|placement| {
                    placement.alternate != alternate
                        || !placement_contains(placement, row, col)
                        || (selector == 'q'
                            && z_index.is_some_and(|z_index| placement.z_index != z_index))
                });
            }
            'x' => {
                let col = command.u32_value('x').unwrap_or(1).saturating_sub(1) as usize;
                self.placements.retain(|placement| {
                    placement.alternate != alternate
                        || col < placement.col
                        || col
                            >= placement
                                .col
                                .saturating_add(placement.occupied_cols as usize)
                });
            }
            'y' => {
                let row = command.u32_value('y').unwrap_or(1).saturating_sub(1) as i64
                    + if alternate {
                        0
                    } else {
                        grid.history_size() as i64
                    };
                self.placements.retain(|placement| {
                    placement.alternate != alternate
                        || row < placement.anchor_line
                        || row
                            >= placement
                                .anchor_line
                                .saturating_add(i64::from(placement.occupied_rows))
                });
            }
            'z' => {
                let z_index = command.i32_value('z').unwrap_or(0);
                self.placements.retain(|placement| {
                    placement.alternate != alternate || placement.z_index != z_index
                });
            }
            _ => return failure(command, "EINVAL:unsupported delete selector"),
        }
        if free_data {
            self.drop_unplaced_images();
        }
        success(command, before != self.placements.len())
    }

    pub(crate) fn render_placements(&self, grid: &Grid) -> Vec<GraphicsRenderPlacement> {
        let alternate = grid.alternate_screen_mode();
        let history = i64::try_from(grid.history_size()).unwrap_or(i64::MAX);
        let offset = i64::try_from(grid.display_offset()).unwrap_or(i64::MAX);
        let rows = i64::try_from(grid.rows()).unwrap_or(i64::MAX);
        self.placements
            .iter()
            .filter(|placement| placement.alternate == alternate)
            .filter_map(|placement| {
                let image = self.images.get(&placement.image_id)?;
                let viewport_row = if alternate {
                    placement.anchor_line
                } else {
                    placement
                        .anchor_line
                        .saturating_sub(history)
                        .saturating_add(offset)
                };
                if viewport_row.saturating_add(i64::from(placement.occupied_rows)) <= 0
                    || viewport_row >= rows
                    || placement.col >= grid.cols()
                {
                    return None;
                }
                Some(GraphicsRenderPlacement {
                    placement_serial: placement.serial,
                    image_id: placement.image_id,
                    placement_id: placement.placement_id,
                    png: image.png.clone(),
                    image_width: image.width,
                    image_height: image.height,
                    image_generation: image.generation,
                    viewport_row: i32::try_from(viewport_row).unwrap_or(if viewport_row < 0 {
                        i32::MIN
                    } else {
                        i32::MAX
                    }),
                    col: placement.col,
                    source_x: placement.source_x,
                    source_y: placement.source_y,
                    source_width: placement.source_width,
                    source_height: placement.source_height,
                    display_cols: placement.display_cols,
                    display_rows: placement.display_rows,
                    occupied_cols: placement.occupied_cols,
                    occupied_rows: placement.occupied_rows,
                    x_offset: placement.x_offset,
                    y_offset: placement.y_offset,
                    z_index: placement.z_index,
                })
            })
            .collect()
    }

    fn rebase_primary_history(&mut self, dropped: usize) -> bool {
        if dropped == 0 {
            return false;
        }
        let dropped = i64::try_from(dropped).unwrap_or(i64::MAX);
        let mut changed = false;
        self.placements.retain_mut(|placement| {
            if placement.alternate {
                return true;
            }
            placement.anchor_line = placement.anchor_line.saturating_sub(dropped);
            changed = true;
            placement
                .anchor_line
                .saturating_add(i64::from(placement.occupied_rows))
                > 0
        });
        changed
    }

    fn scroll_region_up(
        &mut self,
        alternate: bool,
        top: usize,
        bottom: usize,
        count: usize,
        history_size: usize,
    ) -> bool {
        if count == 0 {
            return false;
        }
        let base = if alternate {
            0
        } else {
            i64::try_from(history_size).unwrap_or(i64::MAX)
        };
        let top = i64::try_from(top).unwrap_or(i64::MAX);
        let bottom = i64::try_from(bottom).unwrap_or(i64::MAX);
        let count = i64::try_from(count).unwrap_or(i64::MAX);
        let mut changed = false;
        self.placements.retain_mut(|placement| {
            if placement.alternate != alternate {
                return true;
            }
            let row = placement.anchor_line.saturating_sub(base);
            if row < top || row > bottom {
                return true;
            }
            let new_row = row.saturating_sub(count);
            placement.anchor_line = base.saturating_add(new_row);
            changed = true;
            new_row.saturating_add(i64::from(placement.occupied_rows)) > top
        });
        changed
    }

    fn scroll_region_down(
        &mut self,
        alternate: bool,
        top: usize,
        bottom: usize,
        count: usize,
        history_size: usize,
    ) -> bool {
        if count == 0 {
            return false;
        }
        let base = if alternate {
            0
        } else {
            i64::try_from(history_size).unwrap_or(i64::MAX)
        };
        let top = i64::try_from(top).unwrap_or(i64::MAX);
        let bottom = i64::try_from(bottom).unwrap_or(i64::MAX);
        let count = i64::try_from(count).unwrap_or(i64::MAX);
        let mut changed = false;
        self.placements.retain_mut(|placement| {
            if placement.alternate != alternate {
                return true;
            }
            let row = placement.anchor_line.saturating_sub(base);
            if row < top || row > bottom {
                return true;
            }
            let new_row = row.saturating_add(count);
            placement.anchor_line = base.saturating_add(new_row);
            changed = true;
            new_row <= bottom
        });
        changed
    }

    fn clear_screen_placements(&mut self, alternate: bool) -> bool {
        let before = self.placements.len();
        self.placements
            .retain(|placement| placement.alternate != alternate);
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.alternate == alternate)
        {
            self.pending = None;
        }
        before != self.placements.len()
    }

    fn clear_viewport(
        &mut self,
        alternate: bool,
        history_size: usize,
        rows: usize,
        cols: usize,
    ) -> bool {
        if rows == 0 || cols == 0 {
            return false;
        }
        let viewport_start = if alternate {
            0
        } else {
            i64::try_from(history_size).unwrap_or(i64::MAX)
        };
        let viewport_end = viewport_start.saturating_add(i64::try_from(rows).unwrap_or(i64::MAX));
        let before = self.placements.len();
        self.placements.retain(|placement| {
            if placement.alternate != alternate {
                return true;
            }
            let placement_end = placement
                .anchor_line
                .saturating_add(i64::from(placement.occupied_rows));
            let vertically_visible =
                placement.anchor_line < viewport_end && placement_end > viewport_start;
            let horizontally_visible = placement.col < cols && placement.occupied_cols > 0;
            !(vertically_visible && horizontally_visible)
        });
        before != self.placements.len()
    }

    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    fn allocate_anonymous_id(&mut self) -> u32 {
        if self.next_anonymous_id == 0 {
            self.next_anonymous_id = u32::MAX;
        }
        while self.images.contains_key(&self.next_anonymous_id) {
            self.next_anonymous_id = self.next_anonymous_id.saturating_sub(1).max(1);
        }
        let id = self.next_anonymous_id;
        self.next_anonymous_id = self.next_anonymous_id.saturating_sub(1).max(1);
        id
    }

    fn enforce_quota(&mut self, protected: Option<u32>) -> bool {
        while self.stored_bytes > MAX_IMAGE_BYTES {
            let Some(candidate) = self.insertion_order.pop_front() else {
                break;
            };
            if protected == Some(candidate)
                || self
                    .placements
                    .iter()
                    .any(|placement| placement.image_id == candidate)
            {
                self.insertion_order.push_back(candidate);
                if self.insertion_order.iter().all(|id| {
                    protected == Some(*id)
                        || self
                            .placements
                            .iter()
                            .any(|placement| placement.image_id == *id)
                }) {
                    break;
                }
            } else {
                self.remove_image(candidate);
            }
        }
        self.stored_bytes <= MAX_IMAGE_BYTES
    }

    fn drop_unplaced_images(&mut self) {
        let ids = self
            .images
            .keys()
            .copied()
            .filter(|id| {
                !self
                    .placements
                    .iter()
                    .any(|placement| placement.image_id == *id)
            })
            .collect::<Vec<_>>();
        for id in ids {
            self.remove_image(id);
        }
    }

    fn remove_image(&mut self, image_id: u32) {
        if let Some(image) = self.images.remove(&image_id) {
            self.stored_bytes = self.stored_bytes.saturating_sub(image.png.len());
        }
        self.placements
            .retain(|placement| placement.image_id != image_id);
        self.insertion_order.retain(|id| *id != image_id);
    }
}

fn placement_contains(placement: &Placement, line: i64, col: usize) -> bool {
    line >= placement.anchor_line
        && line
            < placement
                .anchor_line
                .saturating_add(i64::from(placement.occupied_rows))
        && col >= placement.col
        && col
            < placement
                .col
                .saturating_add(placement.occupied_cols as usize)
}

fn success(command: &GraphicsCommand, changed: bool) -> ApplyResult {
    ApplyResult {
        replies: response(command, true, "OK").unwrap_or_default(),
        changed,
    }
}

fn failure(command: &GraphicsCommand, message: &str) -> ApplyResult {
    ApplyResult {
        replies: response(command, false, message).unwrap_or_default(),
        changed: false,
    }
}

fn response(command: &GraphicsCommand, successful: bool, message: &str) -> Option<Vec<u8>> {
    let quiet = command.u32_value('q').unwrap_or(0);
    if (successful && quiet >= 1) || (!successful && quiet >= 2) {
        return None;
    }
    let image_id = command.u32_value('i').or_else(|| command.u32_value('I'))?;
    let mut control = format!("i={image_id}");
    if let Some(placement_id) = command.u32_value('p') {
        control.push_str(&format!(",p={placement_id}"));
    }
    Some(format!("\x1b_G{control};{message}\x1b\\").into_bytes())
}

fn resolve_transmission_data(
    command: &GraphicsCommand,
    decoded: Vec<u8>,
) -> Result<Vec<u8>, String> {
    match command.char_value('t').unwrap_or('d') {
        'd' => Ok(decoded),
        'f' | 't' => {
            let path = PathBuf::from(
                std::str::from_utf8(&decoded).map_err(|_| "EINVAL:file path is not UTF-8")?,
            );
            read_regular_file(
                path,
                u64::from(command.u32_value('O').unwrap_or(0)),
                command
                    .u32_value('S')
                    .filter(|size| *size > 0)
                    .map(u64::from),
            )
        }
        's' => Err("ENOTSUP:shared-memory transmission is not supported".into()),
        _ => Err("EINVAL:unsupported transmission medium".into()),
    }
}

fn read_regular_file(path: PathBuf, offset: u64, size: Option<u64>) -> Result<Vec<u8>, String> {
    let mut file = File::open(path).map_err(|_| "ENOENT:unable to open image file".to_string())?;
    let metadata = file
        .metadata()
        .map_err(|_| "EIO:unable to inspect image file".to_string())?;
    if !metadata.is_file() || offset > metadata.len() {
        return Err("EINVAL:invalid image file".into());
    }
    let length = size
        .unwrap_or(metadata.len() - offset)
        .min(metadata.len() - offset);
    if length > MAX_IMAGE_BYTES as u64 {
        return Err("EFBIG:image file exceeds storage limit".into());
    }
    file.seek(SeekFrom::Start(offset))
        .map_err(|_| "EIO:unable to seek image file".to_string())?;
    let mut output = Vec::with_capacity(length as usize);
    file.take(length)
        .read_to_end(&mut output)
        .map_err(|_| "EIO:unable to read image file".to_string())?;
    Ok(output)
}

fn normalize_image(command: &GraphicsCommand, data: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    match command.u32_value('f').unwrap_or(32) {
        100 => {
            let (width, height) = png_dimensions(data)?;
            validate_dimensions(width, height, 4)?;
            Ok((data.to_vec(), width, height))
        }
        format @ (24 | 32) => {
            let width = command.u32_value('s').unwrap_or(0);
            let height = command.u32_value('v').unwrap_or(0);
            let channels = if format == 24 { 3 } else { 4 };
            let expected = validate_dimensions(width, height, channels)?;
            if data.len() != expected {
                return Err("EINVAL:pixel data length does not match dimensions".into());
            }
            Ok((
                encode_png(width, height, channels as u8, data),
                width,
                height,
            ))
        }
        _ => Err("EINVAL:unsupported image format".into()),
    }
}

fn validate_dimensions(width: u32, height: u32, channels: usize) -> Result<usize, String> {
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err("EINVAL:invalid image dimensions".into());
    }
    let pixels = u64::from(width) * u64::from(height);
    if pixels > MAX_PIXELS {
        return Err("EFBIG:image dimensions exceed storage limit".into());
    }
    usize::try_from(pixels)
        .ok()
        .and_then(|pixels| pixels.checked_mul(channels))
        .ok_or_else(|| "EFBIG:image dimensions overflow".into())
}

fn png_dimensions(data: &[u8]) -> Result<(u32, u32), String> {
    if data.len() < 8 || &data[..8] != b"\x89PNG\r\n\x1a\n" {
        return Err("EINVAL:invalid PNG image".into());
    }
    let mut offset = 8usize;
    let mut dimensions = None;
    let mut saw_idat = false;
    while offset.checked_add(12).is_some_and(|end| end <= data.len()) {
        let length = u32::from_be_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        let kind = &data[offset + 4..offset + 8];
        let payload_start = offset + 8;
        let Some(payload_end) = payload_start.checked_add(length) else {
            return Err("EINVAL:invalid PNG image".into());
        };
        let Some(chunk_end) = payload_end.checked_add(4) else {
            return Err("EINVAL:invalid PNG image".into());
        };
        if chunk_end > data.len() {
            return Err("EINVAL:invalid PNG image".into());
        }
        let expected_crc = u32::from_be_bytes(data[payload_end..chunk_end].try_into().unwrap());
        if crc32_parts(&[kind, &data[payload_start..payload_end]]) != expected_crc {
            return Err("EINVAL:invalid PNG image".into());
        }

        match kind {
            b"IHDR" if offset == 8 && length == 13 => {
                let header = &data[payload_start..payload_end];
                let width = u32::from_be_bytes(header[0..4].try_into().unwrap());
                let height = u32::from_be_bytes(header[4..8].try_into().unwrap());
                if !valid_png_format(header[8], header[9])
                    || header[10] != 0
                    || header[11] != 0
                    || header[12] > 1
                {
                    return Err("EINVAL:invalid PNG image".into());
                }
                dimensions = Some((width, height));
            }
            b"IHDR" => return Err("EINVAL:invalid PNG image".into()),
            b"IDAT" if dimensions.is_some() => saw_idat = true,
            b"IEND" if length == 0 && dimensions.is_some() && saw_idat => {
                if chunk_end != data.len() {
                    return Err("EINVAL:invalid PNG image".into());
                }
                return dimensions.ok_or_else(|| "EINVAL:invalid PNG image".into());
            }
            b"IEND" => return Err("EINVAL:invalid PNG image".into()),
            _ => {}
        }
        offset = chunk_end;
    }
    Err("EINVAL:invalid PNG image".into())
}

fn valid_png_format(bit_depth: u8, color_type: u8) -> bool {
    match color_type {
        0 => matches!(bit_depth, 1 | 2 | 4 | 8 | 16),
        2 => matches!(bit_depth, 8 | 16),
        3 => matches!(bit_depth, 1 | 2 | 4 | 8),
        4 | 6 => matches!(bit_depth, 8 | 16),
        _ => false,
    }
}

fn encode_png(width: u32, height: u32, channels: u8, pixels: &[u8]) -> Vec<u8> {
    let row_bytes = width as usize * usize::from(channels);
    let mut filtered = Vec::with_capacity(pixels.len().saturating_add(height as usize));
    for row in pixels.chunks_exact(row_bytes) {
        filtered.push(0);
        filtered.extend_from_slice(row);
    }
    let compressed = zlib_store(&filtered);
    let mut png = Vec::with_capacity(compressed.len().saturating_add(64));
    png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.push(8);
    header.push(if channels == 3 { 2 } else { 6 });
    header.extend_from_slice(&[0, 0, 0]);
    push_png_chunk(&mut png, *b"IHDR", &header);
    push_png_chunk(&mut png, *b"IDAT", &compressed);
    push_png_chunk(&mut png, *b"IEND", &[]);
    png
}

fn zlib_store(data: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(data.len().saturating_add(data.len() / 65_535 * 5 + 8));
    output.extend_from_slice(&[0x78, 0x01]);
    if data.is_empty() {
        output.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
    } else {
        let chunks = data.chunks(65_535);
        let count = chunks.len();
        for (index, chunk) in chunks.enumerate() {
            output.push(u8::from(index + 1 == count));
            let length = chunk.len() as u16;
            output.extend_from_slice(&length.to_le_bytes());
            output.extend_from_slice(&(!length).to_le_bytes());
            output.extend_from_slice(chunk);
        }
    }
    output.extend_from_slice(&adler32(data).to_be_bytes());
    output
}

fn push_png_chunk(output: &mut Vec<u8>, kind: [u8; 4], data: &[u8]) {
    output.extend_from_slice(&(data.len() as u32).to_be_bytes());
    output.extend_from_slice(&kind);
    output.extend_from_slice(data);
    output.extend_from_slice(&crc32_parts(&[&kind, data]).to_be_bytes());
}

fn crc32_parts(parts: &[&[u8]]) -> u32 {
    let mut crc = u32::MAX;
    for data in parts {
        for byte in *data {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
            }
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let mut a = 1u32;
    let mut b = 0u32;
    for byte in data {
        a = (a + u32::from(*byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

#[cfg(unix)]
#[link(name = "z")]
unsafe extern "C" {
    fn uncompress(
        destination: *mut u8,
        destination_length: *mut c_ulong,
        source: *const u8,
        source_length: c_ulong,
    ) -> c_int;
}

fn decompress_zlib(command: &GraphicsCommand, data: &[u8]) -> Result<Vec<u8>, String> {
    #[cfg(not(unix))]
    {
        let _ = command;
        let _ = data;
        return Err("ENOTSUP:zlib compression is unavailable on this host".into());
    }
    #[cfg(unix)]
    {
        const Z_OK: c_int = 0;
        const Z_BUF_ERROR: c_int = -5;
        let expected =
            match command.u32_value('f').unwrap_or(32) {
                format @ (24 | 32) => command.u32_value('s').zip(command.u32_value('v')).and_then(
                    |(width, height)| {
                        validate_dimensions(width, height, if format == 24 { 3 } else { 4 }).ok()
                    },
                ),
                _ => None,
            };
        let mut capacity = expected.unwrap_or_else(|| data.len().saturating_mul(4).max(4096));
        loop {
            capacity = capacity.min(MAX_IMAGE_BYTES);
            let mut output = vec![0u8; capacity];
            let mut length = capacity as c_ulong;
            // SAFETY: both buffers are valid for the lengths passed and libz only writes to
            // the destination buffer. The output is truncated to libz's reported length.
            let status = unsafe {
                uncompress(
                    output.as_mut_ptr(),
                    &mut length,
                    data.as_ptr(),
                    data.len() as c_ulong,
                )
            };
            if status == Z_OK {
                output.truncate(length as usize);
                return Ok(output);
            }
            if status != Z_BUF_ERROR || capacity == MAX_IMAGE_BYTES {
                return Err("EINVAL:invalid zlib payload".into());
            }
            capacity = capacity.saturating_mul(2).min(MAX_IMAGE_BYTES);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CursorStyle;

    fn test_size() -> Size {
        Size {
            cols: 80,
            rows: 24,
            cell_width: 10.0,
            cell_height: 20.0,
        }
    }

    fn encode_base64(input: &[u8]) -> String {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut output = String::new();
        for chunk in input.chunks(3) {
            let value = (u32::from(chunk[0]) << 16)
                | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
                | u32::from(*chunk.get(2).unwrap_or(&0));
            output.push(TABLE[((value >> 18) & 63) as usize] as char);
            output.push(TABLE[((value >> 12) & 63) as usize] as char);
            output.push(if chunk.len() > 1 {
                TABLE[((value >> 6) & 63) as usize] as char
            } else {
                '='
            });
            output.push(if chunk.len() > 2 {
                TABLE[(value & 63) as usize] as char
            } else {
                '='
            });
        }
        output
    }

    fn command(control: &str, payload: &[u8]) -> GraphicsCommand {
        GraphicsCommand::parse(
            format!("{control};{}", encode_base64(payload)).into_bytes(),
            false,
        )
    }

    fn upload_one_pixel(state: &mut GraphicsState, grid: &mut Grid, image_id: u32) {
        let result = state.apply(
            command(
                &format!("a=t,f=32,s=1,v=1,i={image_id},q=1"),
                &[255, 0, 0, 255],
            ),
            grid,
            test_size(),
        );
        assert!(result.changed);
    }

    #[test]
    fn raw_rgba_upload_encodes_png_and_places_at_cursor() {
        let mut state = GraphicsState::default();
        let mut grid = Grid::new(80, 24, 100, CursorStyle::Block);
        grid.set_cursor_position(5, 4);
        let payload = encode_base64(&[255, 0, 0, 255]);
        let command = GraphicsCommand::parse(
            format!("a=T,f=32,s=1,v=1,i=42,c=2,r=3;{payload}").into_bytes(),
            false,
        );
        let result = state.apply(command, &mut grid, test_size());

        assert!(result.changed);
        assert_eq!(result.replies, b"\x1b_Gi=42;OK\x1b\\");
        let placements = state.render_placements(&grid);
        assert_eq!(placements.len(), 1);
        assert!(placements[0].png.starts_with(b"\x89PNG"));
        assert_eq!((placements[0].viewport_row, placements[0].col), (5, 4));
    }

    #[test]
    fn rejects_truncated_and_crc_corrupted_png_payloads() {
        let png = encode_png(1, 1, 4, &[255, 0, 0, 255]);
        for invalid in [png[..png.len() - 5].to_vec(), {
            let mut corrupted = png.clone();
            corrupted[29] ^= 1;
            corrupted
        }] {
            let payload = encode_base64(&invalid);
            let mut state = GraphicsState::default();
            let mut grid = Grid::new(80, 24, 0, CursorStyle::Block);
            let result = state.apply(
                GraphicsCommand::parse(format!("a=T,f=100,i=9;{payload}").into_bytes(), false),
                &mut grid,
                Size::default(),
            );
            assert!(!result.changed);
            assert_eq!(result.replies, b"\x1b_Gi=9;EINVAL:invalid PNG image\x1b\\");
            assert!(state.render_placements(&grid).is_empty());
        }
    }

    #[test]
    fn assembles_chunked_and_zlib_compressed_uploads() {
        let mut state = GraphicsState::default();
        let mut grid = Grid::new(80, 24, 0, CursorStyle::Block);
        let pixels = [12, 34, 56, 255];

        let first = state.apply(
            command("a=T,f=32,s=1,v=1,i=20,m=1,q=1", &pixels[..2]),
            &mut grid,
            test_size(),
        );
        assert!(!first.changed);
        let second = state.apply(command("m=0,q=1", &pixels[2..]), &mut grid, test_size());
        assert!(second.changed);

        let compressed = zlib_store(&pixels);
        let compressed_result = state.apply(
            command("a=T,f=32,s=1,v=1,o=z,i=21,C=1,q=1", &compressed),
            &mut grid,
            test_size(),
        );
        assert!(compressed_result.changed);
        assert_eq!(state.render_placements(&grid).len(), 2);
    }

    #[test]
    fn delete_selectors_target_coordinates_rows_columns_z_and_ids() {
        let mut state = GraphicsState::default();
        let mut grid = Grid::new(80, 24, 0, CursorStyle::Block);
        upload_one_pixel(&mut state, &mut grid, 30);

        for (row, col, placement_id, z_index) in [(0, 0, 1, 7), (0, 4, 2, 8), (4, 0, 3, 7)] {
            grid.set_cursor_position(row, col);
            let result = state.apply(
                command(
                    &format!("a=p,i=30,p={placement_id},c=2,r=2,z={z_index},C=1,q=1"),
                    &[],
                ),
                &mut grid,
                test_size(),
            );
            assert!(result.changed);
        }

        state.apply(
            command("a=d,d=q,x=1,y=1,z=7,q=1", &[]),
            &mut grid,
            test_size(),
        );
        let placements = state.render_placements(&grid);
        assert_eq!(placements.len(), 2);
        assert!(
            placements
                .iter()
                .all(|placement| placement.placement_id != 1)
        );

        state.apply(command("a=d,d=x,x=1,q=1", &[]), &mut grid, test_size());
        let placements = state.render_placements(&grid);
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].placement_id, 2);

        state.apply(command("a=d,d=i,i=30,p=2,q=1", &[]), &mut grid, test_size());
        assert!(state.render_placements(&grid).is_empty());
        assert!(
            state.images.contains_key(&30),
            "lowercase delete keeps image data"
        );

        state.apply(command("a=d,d=I,i=30,q=1", &[]), &mut grid, test_size());
        assert!(!state.images.contains_key(&30));
    }

    #[test]
    fn placement_count_is_bounded_and_evicts_the_oldest() {
        let mut state = GraphicsState::default();
        let mut grid = Grid::new(80, 24, 0, CursorStyle::Block);
        upload_one_pixel(&mut state, &mut grid, 40);
        for index in 0..=MAX_PLACEMENTS {
            grid.set_cursor_position(index % 24, index % 80);
            state.apply(
                command("a=p,i=40,c=1,r=1,C=1,q=1", &[]),
                &mut grid,
                test_size(),
            );
        }
        assert_eq!(state.placements.len(), MAX_PLACEMENTS);
        assert_eq!(state.placements.first().unwrap().serial, 2);
        assert_eq!(
            state.placements.last().unwrap().serial,
            (MAX_PLACEMENTS + 1) as u64
        );
    }
}
