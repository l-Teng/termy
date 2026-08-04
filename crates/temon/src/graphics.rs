#[cfg(unix)]
use std::ffi::{c_int, c_ulong};
use std::{
    collections::{HashMap, VecDeque},
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
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
                full_screen_region,
                recorded_history,
                history_before,
                history_after,
            } => {
                if !alternate && recorded_history {
                    if full_screen_region {
                        let dropped = history_before
                            .saturating_add(count)
                            .saturating_sub(history_after);
                        self.rebase_primary_history(dropped)
                    } else {
                        let history_growth = history_after.saturating_sub(history_before);
                        let preserved = self
                            .preserve_primary_placements_across_partial_history_growth(
                                history_growth,
                            );
                        self.scroll_region_up(alternate, top, bottom, count, history_after)
                            || preserved
                    }
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
            GridEffect::Reset => {
                let primary = self.clear_screen_placements(false);
                let alternate = self.clear_screen_placements(true);
                primary || alternate
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
            let response_command = self
                .pending
                .take()
                .map_or_else(|| command.clone(), |pending| pending.command);
            return failure(
                &response_command,
                "EFBIG:image command exceeds storage limit",
            );
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
                let response_command = self
                    .pending
                    .take()
                    .map_or_else(|| command.clone(), |pending| pending.command);
                return failure(
                    &response_command,
                    "EFBIG:image payload exceeds storage limit",
                );
            }
            None => {
                let response_command = self
                    .pending
                    .take()
                    .map_or_else(|| command.clone(), |pending| pending.command);
                return failure(&response_command, "EINVAL:invalid base64 payload");
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
            if context.alternate == grid.alternate_screen_mode()
                && let Some((cols, rows)) = advance
            {
                advance_cursor_after_placement(grid, cols, rows);
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
                    advance_cursor_after_placement(grid, cols, rows);
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
                let cols = (placed_source_width as f32 / cell_width).ceil().max(1.0) as u32;
                (
                    Some(cols.min(available_cols)),
                    Some((source_height as f32 / cell_height).ceil().max(1.0) as u32),
                )
            }
        };
        let occupied_cols = display_cols.unwrap_or(1);
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

    fn preserve_primary_placements_across_partial_history_growth(&mut self, lines: usize) -> bool {
        if lines == 0 {
            return false;
        }
        let lines = i64::try_from(lines).unwrap_or(i64::MAX);
        let mut changed = false;
        for placement in self
            .placements
            .iter_mut()
            .filter(|placement| !placement.alternate)
        {
            placement.anchor_line = placement.anchor_line.saturating_add(lines);
            changed = true;
        }
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

fn advance_cursor_after_placement(grid: &mut Grid, cols: u32, rows: u32) {
    grid.cursor_forward((cols as usize).min(grid.cols()));
    let rows = (rows as usize).min(grid.rows());
    if grid.scroll_region_covers_full_screen() {
        for _ in 0..rows {
            grid.line_feed();
        }
    } else {
        // Kitty's post-placement movement must not rotate a partial DECSTBM region.
        // A cursor-down operation clamps without scrolling, matching the native engine.
        grid.cursor_down(rows);
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
            let temporary = command.char_value('t') == Some('t');
            let path = PathBuf::from(
                std::str::from_utf8(&decoded).map_err(|_| "EINVAL:file path is not UTF-8")?,
            );
            let result = read_regular_file(
                &path,
                u64::from(command.u32_value('O').unwrap_or(0)),
                command
                    .u32_value('S')
                    .filter(|size| *size > 0)
                    .map(u64::from),
            );
            if temporary && result.is_ok() && temporary_path_can_be_removed(&path) {
                let _ = std::fs::remove_file(&path);
            }
            result
        }
        's' => Err("ENOTSUP:shared-memory transmission is not supported".into()),
        _ => Err("EINVAL:unsupported transmission medium".into()),
    }
}

fn read_regular_file(path: &Path, offset: u64, size: Option<u64>) -> Result<Vec<u8>, String> {
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

fn temporary_path_can_be_removed(path: &Path) -> bool {
    let Ok(canonical) = path.canonicalize() else {
        return false;
    };
    if !canonical.components().any(|component| {
        component
            .as_os_str()
            .to_string_lossy()
            .contains("tty-graphics-protocol")
    }) {
        return false;
    }
    let mut roots = vec![
        PathBuf::from("/tmp"),
        PathBuf::from("/private/tmp"),
        PathBuf::from("/dev/shm"),
    ];
    roots.push(std::env::temp_dir());
    roots.into_iter().any(|root| {
        root.canonicalize()
            .is_ok_and(|root| canonical.starts_with(root))
    })
}

fn normalize_image(command: &GraphicsCommand, data: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    match command.u32_value('f').unwrap_or(32) {
        100 => {
            #[cfg(not(unix))]
            {
                let _ = data;
                Err("ENOTSUP:PNG images are unavailable on this host".into())
            }
            #[cfg(unix)]
            {
                let (width, height) = validate_png(data)?;
                Ok((data.to_vec(), width, height))
            }
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

#[cfg(unix)]
#[derive(Clone, Copy, Debug)]
struct PngHeader {
    width: u32,
    height: u32,
    bit_depth: u8,
    color_type: u8,
    interlace: u8,
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, Default)]
struct PngPass {
    row_bytes: usize,
    rows: usize,
}

#[cfg(unix)]
fn validate_png(data: &[u8]) -> Result<(u32, u32), String> {
    if data.len() < 8 || &data[..8] != b"\x89PNG\r\n\x1a\n" {
        return Err("EINVAL:invalid PNG image".into());
    }
    let mut offset = 8usize;
    let mut header = None;
    let mut saw_palette = false;
    let mut saw_idat = false;
    let mut idat_ended = false;
    let mut idat_bytes = 0usize;
    while offset.checked_add(12).is_some_and(|end| end <= data.len()) {
        let length = u32::from_be_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        let kind = &data[offset + 4..offset + 8];
        if !kind.iter().all(u8::is_ascii_alphabetic) || !kind[2].is_ascii_uppercase() {
            return Err("EINVAL:invalid PNG image".into());
        }
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

        if saw_idat && kind != b"IDAT" {
            idat_ended = true;
        }
        match kind {
            b"IHDR" if offset == 8 && length == 13 && header.is_none() => {
                let payload = &data[payload_start..payload_end];
                let width = u32::from_be_bytes(payload[0..4].try_into().unwrap());
                let height = u32::from_be_bytes(payload[4..8].try_into().unwrap());
                if !valid_png_format(payload[8], payload[9])
                    || payload[10] != 0
                    || payload[11] != 0
                    || payload[12] > 1
                {
                    return Err("EINVAL:invalid PNG image".into());
                }
                validate_dimensions(width, height, 4)?;
                header = Some(PngHeader {
                    width,
                    height,
                    bit_depth: payload[8],
                    color_type: payload[9],
                    interlace: payload[12],
                });
            }
            b"IHDR" => return Err("EINVAL:invalid PNG image".into()),
            b"PLTE" => {
                let Some(header) = header else {
                    return Err("EINVAL:invalid PNG image".into());
                };
                if saw_palette
                    || saw_idat
                    || matches!(header.color_type, 0 | 4)
                    || !(3..=768).contains(&length)
                    || !length.is_multiple_of(3)
                    || length / 3 > 1usize << header.bit_depth
                {
                    return Err("EINVAL:invalid PNG image".into());
                }
                saw_palette = true;
            }
            b"IDAT" if header.is_some() && !idat_ended => {
                if header.is_some_and(|header| header.color_type == 3) && !saw_palette {
                    return Err("EINVAL:invalid PNG image".into());
                }
                saw_idat = true;
                idat_bytes = idat_bytes
                    .checked_add(length)
                    .ok_or_else(|| "EFBIG:PNG payload exceeds storage limit".to_string())?;
            }
            b"IDAT" => return Err("EINVAL:invalid PNG image".into()),
            b"IEND" if length == 0 && header.is_some() && saw_idat => {
                if chunk_end != data.len() {
                    return Err("EINVAL:invalid PNG image".into());
                }
                let header = header.unwrap();
                if header.color_type == 3 && !saw_palette {
                    return Err("EINVAL:invalid PNG image".into());
                }
                let (expected, passes, pass_count) = png_scanline_layout(header)?;
                let compressed = concatenate_png_idat(data, idat_bytes)?;
                let filtered = decompress_png_idat(&compressed, expected)?;
                validate_png_filters(&filtered, &passes[..pass_count])?;
                return Ok((header.width, header.height));
            }
            b"IEND" => return Err("EINVAL:invalid PNG image".into()),
            _ if kind[0].is_ascii_uppercase() => {
                return Err("EINVAL:invalid PNG image".into());
            }
            _ => {}
        }
        offset = chunk_end;
    }
    Err("EINVAL:invalid PNG image".into())
}

#[cfg(unix)]
fn png_scanline_layout(header: PngHeader) -> Result<(usize, [PngPass; 7], usize), String> {
    const ADAM7: [(u64, u64, u64, u64); 7] = [
        (0, 0, 8, 8),
        (4, 0, 8, 8),
        (0, 4, 4, 8),
        (2, 0, 4, 4),
        (0, 2, 2, 4),
        (1, 0, 2, 2),
        (0, 1, 1, 2),
    ];
    const NON_INTERLACED: [(u64, u64, u64, u64); 1] = [(0, 0, 1, 1)];

    let samples = match header.color_type {
        0 | 3 => 1u64,
        2 => 3,
        4 => 2,
        6 => 4,
        _ => return Err("EINVAL:invalid PNG image".into()),
    };
    let patterns = if header.interlace == 0 {
        NON_INTERLACED.as_slice()
    } else {
        ADAM7.as_slice()
    };
    let mut passes = [PngPass::default(); 7];
    let mut expected = 0u64;
    let width = u64::from(header.width);
    let height = u64::from(header.height);
    for (index, &(start_x, start_y, step_x, step_y)) in patterns.iter().enumerate() {
        let pass_width = pass_extent(width, start_x, step_x)?;
        let pass_height = pass_extent(height, start_y, step_y)?;
        if pass_width == 0 || pass_height == 0 {
            continue;
        }
        let row_bits = pass_width
            .checked_mul(samples)
            .and_then(|bits| bits.checked_mul(u64::from(header.bit_depth)))
            .ok_or_else(|| "EFBIG:PNG dimensions overflow".to_string())?;
        let row_bytes = row_bits
            .checked_add(7)
            .ok_or_else(|| "EFBIG:PNG dimensions overflow".to_string())?
            / 8;
        let pass_bytes = row_bytes
            .checked_add(1)
            .and_then(|bytes| bytes.checked_mul(pass_height))
            .ok_or_else(|| "EFBIG:PNG dimensions overflow".to_string())?;
        expected = expected
            .checked_add(pass_bytes)
            .ok_or_else(|| "EFBIG:PNG dimensions overflow".to_string())?;
        if expected > MAX_IMAGE_BYTES as u64 {
            return Err("EFBIG:PNG scanlines exceed storage limit".into());
        }
        passes[index] = PngPass {
            row_bytes: usize::try_from(row_bytes)
                .map_err(|_| "EFBIG:PNG dimensions overflow".to_string())?,
            rows: usize::try_from(pass_height)
                .map_err(|_| "EFBIG:PNG dimensions overflow".to_string())?,
        };
    }
    let expected =
        usize::try_from(expected).map_err(|_| "EFBIG:PNG dimensions overflow".to_string())?;
    Ok((expected, passes, patterns.len()))
}

#[cfg(unix)]
fn pass_extent(total: u64, start: u64, step: u64) -> Result<u64, String> {
    if total <= start {
        return Ok(0);
    }
    total
        .checked_sub(start)
        .and_then(|remaining| remaining.checked_add(step - 1))
        .map(|remaining| remaining / step)
        .ok_or_else(|| "EFBIG:PNG dimensions overflow".into())
}

#[cfg(unix)]
fn concatenate_png_idat(data: &[u8], total: usize) -> Result<Vec<u8>, String> {
    let mut compressed = Vec::new();
    compressed
        .try_reserve_exact(total)
        .map_err(|_| "ENOMEM:unable to validate PNG image".to_string())?;
    let mut offset = 8usize;
    while offset + 12 <= data.len() {
        let length = u32::from_be_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        let payload_start = offset + 8;
        let payload_end = payload_start + length;
        if &data[offset + 4..offset + 8] == b"IDAT" {
            compressed.extend_from_slice(&data[payload_start..payload_end]);
        }
        offset = payload_end + 4;
    }
    if compressed.len() != total {
        return Err("EINVAL:invalid PNG image".into());
    }
    Ok(compressed)
}

#[cfg(unix)]
fn decompress_png_idat(compressed: &[u8], expected: usize) -> Result<Vec<u8>, String> {
    const Z_OK: c_int = 0;

    let mut filtered = Vec::new();
    filtered
        .try_reserve_exact(expected)
        .map_err(|_| "ENOMEM:unable to validate PNG image".to_string())?;
    filtered.resize(expected, 0);
    let mut destination_length = c_ulong::try_from(expected)
        .map_err(|_| "EFBIG:PNG scanlines exceed storage limit".to_string())?;
    let mut source_length = c_ulong::try_from(compressed.len())
        .map_err(|_| "EFBIG:PNG payload exceeds storage limit".to_string())?;
    // SAFETY: both vectors remain allocated for the call. The destination has exactly
    // `destination_length` writable bytes and libz only reads up to `source_length` bytes.
    let status = unsafe {
        uncompress2(
            filtered.as_mut_ptr(),
            &mut destination_length,
            compressed.as_ptr(),
            &mut source_length,
        )
    };
    if status != Z_OK
        || destination_length as usize != expected
        || source_length as usize != compressed.len()
    {
        return Err("EINVAL:invalid PNG image".into());
    }
    Ok(filtered)
}

#[cfg(unix)]
fn validate_png_filters(filtered: &[u8], passes: &[PngPass]) -> Result<(), String> {
    let mut offset = 0usize;
    for pass in passes {
        let scanline_bytes = pass
            .row_bytes
            .checked_add(1)
            .ok_or_else(|| "EFBIG:PNG dimensions overflow".to_string())?;
        for _ in 0..pass.rows {
            if filtered.get(offset).is_none_or(|filter| *filter > 4) {
                return Err("EINVAL:invalid PNG image".into());
            }
            offset = offset
                .checked_add(scanline_bytes)
                .ok_or_else(|| "EFBIG:PNG dimensions overflow".to_string())?;
        }
    }
    if offset != filtered.len() {
        return Err("EINVAL:invalid PNG image".into());
    }
    Ok(())
}

#[cfg(unix)]
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

    fn uncompress2(
        destination: *mut u8,
        destination_length: *mut c_ulong,
        source: *const u8,
        source_length: *mut c_ulong,
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
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TestTempPath {
        path: PathBuf,
    }

    impl TestTempPath {
        fn new() -> Self {
            static NEXT_ID: AtomicU64 = AtomicU64::new(0);
            loop {
                let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
                let path = std::env::temp_dir().join(format!(
                    "termy-temon-graphics-test-{}-{id}",
                    std::process::id()
                ));
                match std::fs::create_dir(&path) {
                    Ok(()) => return Self { path },
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => panic!("failed to create test temp path: {error}"),
                }
            }
        }

        fn file(&self, name: &str, contents: &[u8]) -> PathBuf {
            let path = self.path.join(name);
            std::fs::write(&path, contents).unwrap();
            path
        }
    }

    impl Drop for TestTempPath {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

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

    #[cfg(unix)]
    fn png_document(
        width: u32,
        height: u32,
        bit_depth: u8,
        color_type: u8,
        interlace: u8,
        chunks: Vec<([u8; 4], Vec<u8>)>,
    ) -> Vec<u8> {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut header = Vec::with_capacity(13);
        header.extend_from_slice(&width.to_be_bytes());
        header.extend_from_slice(&height.to_be_bytes());
        header.extend_from_slice(&[bit_depth, color_type, 0, 0, interlace]);
        push_png_chunk(&mut png, *b"IHDR", &header);
        for (kind, payload) in chunks {
            push_png_chunk(&mut png, kind, &payload);
        }
        push_png_chunk(&mut png, *b"IEND", &[]);
        png
    }

    #[cfg(unix)]
    fn png_with_filtered_data(
        width: u32,
        height: u32,
        bit_depth: u8,
        color_type: u8,
        interlace: u8,
        filtered: &[u8],
    ) -> Vec<u8> {
        png_document(
            width,
            height,
            bit_depth,
            color_type,
            interlace,
            vec![(*b"IDAT", zlib_store(filtered))],
        )
    }

    fn path_bytes(path: &Path) -> Vec<u8> {
        path.to_str()
            .expect("test temp path should be UTF-8")
            .as_bytes()
            .to_vec()
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
    fn continuation_failures_reply_with_the_original_image_id() {
        let mut state = GraphicsState::default();
        let mut grid = Grid::new(80, 24, 100, CursorStyle::Block);
        let first = GraphicsCommand::parse(b"a=t,f=32,s=1,v=1,i=7,m=1;AQI=".to_vec(), false);
        assert!(
            state
                .apply(first, &mut grid, test_size())
                .replies
                .is_empty()
        );

        let malformed = GraphicsCommand::parse(b"m=0;!".to_vec(), false);
        assert_eq!(
            state.apply(malformed, &mut grid, test_size()).replies,
            b"\x1b_Gi=7;EINVAL:invalid base64 payload\x1b\\"
        );

        let first = GraphicsCommand::parse(b"a=t,f=32,s=1,v=1,i=8,m=1;AQI=".to_vec(), false);
        assert!(
            state
                .apply(first, &mut grid, test_size())
                .replies
                .is_empty()
        );
        let oversized = GraphicsCommand::parse(b"m=0".to_vec(), true);
        assert_eq!(
            state.apply(oversized, &mut grid, test_size()).replies,
            b"\x1b_Gi=8;EFBIG:image command exceeds storage limit\x1b\\"
        );
    }

    #[test]
    fn chunked_placement_does_not_advance_a_different_screen() {
        let size = Size {
            cols: 8,
            rows: 4,
            ..Size::default()
        };
        let mut state = GraphicsState::default();
        let mut grid = Grid::new(size.cols, size.rows, 100, CursorStyle::Block);
        grid.set_cursor_position(1, 2);
        let first =
            GraphicsCommand::parse(b"a=T,f=32,s=1,v=1,i=7,c=2,r=1,m=1,q=1;AQI=".to_vec(), false);
        assert!(!state.apply(first, &mut grid, size).changed);

        grid.set_mode(true, 1049, true);
        let alternate_cursor = grid.cursor_position();
        let final_chunk = GraphicsCommand::parse(b"m=0;A/8=".to_vec(), false);
        assert!(state.apply(final_chunk, &mut grid, size).changed);
        assert_eq!(grid.cursor_position(), alternate_cursor);

        grid.set_mode(true, 1049, false);
        let placements = state.render_placements(&grid);
        assert_eq!(placements.len(), 1);
        assert_eq!((placements[0].viewport_row, placements[0].col), (1, 2));
    }

    #[test]
    fn terminal_reset_clears_placements_but_keeps_image_data() {
        let mut state = GraphicsState::default();
        let mut grid = Grid::new(80, 24, 100, CursorStyle::Block);
        upload_one_pixel(&mut state, &mut grid, 7);
        let placed = state.apply(command("a=p,i=7,C=1,q=1", &[]), &mut grid, test_size());
        assert!(placed.changed);
        assert_eq!(state.render_placements(&grid).len(), 1);

        assert!(state.apply_grid_effect(GridEffect::Reset));
        assert!(state.render_placements(&grid).is_empty());
        assert!(state.images.contains_key(&7));

        let reused = state.apply(command("a=p,i=7,C=1,q=1", &[]), &mut grid, test_size());
        assert!(reused.changed);
        assert_eq!(state.render_placements(&grid).len(), 1);
    }

    #[test]
    fn partial_top_region_scroll_keeps_footer_placement_fixed() {
        let size = Size {
            cols: 8,
            rows: 4,
            ..Size::default()
        };
        let mut state = GraphicsState::default();
        let mut grid = Grid::new(size.cols, size.rows, 100, CursorStyle::Block);
        grid.set_cursor_position(3, 0);
        let placed = state.apply(
            command("a=T,f=32,s=1,v=1,i=9,C=1,q=1", &[1, 2, 3, 255]),
            &mut grid,
            size,
        );
        assert!(placed.changed);
        assert_eq!(state.render_placements(&grid)[0].viewport_row, 3);

        grid.set_scroll_region(0, 2);
        grid.set_cursor_position(2, 0);
        grid.line_feed();
        while let Some(effect) = grid.pop_effect() {
            state.apply_grid_effect(effect);
        }

        assert_eq!(grid.history_size(), 1);
        assert_eq!(state.render_placements(&grid)[0].viewport_row, 3);
    }

    #[test]
    fn post_placement_cursor_advance_does_not_scroll_partial_region() {
        let size = Size {
            cols: 8,
            rows: 4,
            ..Size::default()
        };
        let mut state = GraphicsState::default();
        let mut grid = Grid::new(size.cols, size.rows, 0, CursorStyle::Block);
        grid.set_cursor_position(1, 0);
        grid.put_char('A');
        grid.set_cursor_position(2, 0);
        grid.put_char('B');
        grid.set_scroll_region(1, 2);
        grid.set_cursor_position(2, 0);
        let cells_before = grid.snapshot().cells;

        let result = state.apply(
            command("a=T,f=32,s=1,v=1,i=82,c=2,r=3,q=1", &[1, 2, 3, 255]),
            &mut grid,
            size,
        );

        assert!(result.changed);
        assert_eq!(grid.snapshot().cells, cells_before);
        assert_eq!(grid.cursor_position(), (2, 3));
        while let Some(effect) = grid.pop_effect() {
            state.apply_grid_effect(effect);
        }
        let placements = state.render_placements(&grid);
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].image_id, 82);
        assert_eq!(placements[0].viewport_row, 2);
    }

    #[test]
    fn column_resize_preserves_placements_and_clips_them_at_render_time() {
        let size = Size {
            cols: 8,
            rows: 4,
            ..Size::default()
        };
        let mut state = GraphicsState::default();
        let mut grid = Grid::new(size.cols, size.rows, 8, CursorStyle::Block);
        grid.set_cursor_position(1, 6);
        let result = state.apply(
            command("a=T,f=32,s=1,v=1,i=83,c=2,r=1,C=1,q=1", &[1, 2, 3, 255]),
            &mut grid,
            size,
        );
        assert!(result.changed);
        let initial = state.render_placements(&grid);
        assert_eq!(initial.len(), 1);
        let serial = initial[0].placement_serial;
        let anchor_line = state.placements[0].anchor_line;

        grid.resize(4, 4);
        while let Some(effect) = grid.pop_effect() {
            assert!(!state.apply_grid_effect(effect));
        }

        assert_eq!(state.placements.len(), 1);
        assert_eq!(state.placements[0].anchor_line, anchor_line);
        assert!(state.render_placements(&grid).is_empty());

        grid.resize(8, 4);
        while let Some(effect) = grid.pop_effect() {
            assert!(!state.apply_grid_effect(effect));
        }
        let restored = state.render_placements(&grid);
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].placement_serial, serial);
        assert_eq!((restored[0].viewport_row, restored[0].col), (1, 6));
    }

    #[test]
    fn column_resize_preserves_an_in_progress_chunked_upload() {
        let size = Size {
            cols: 8,
            rows: 4,
            ..Size::default()
        };
        let mut state = GraphicsState::default();
        let mut grid = Grid::new(size.cols, size.rows, 8, CursorStyle::Block);
        grid.set_cursor_position(2, 3);

        let first = state.apply(
            command("a=T,f=32,s=1,v=1,i=84,c=1,r=1,C=1,m=1,q=1", &[1, 2]),
            &mut grid,
            size,
        );
        assert!(!first.changed);
        assert!(state.pending.is_some());

        grid.resize(4, 4);
        while let Some(effect) = grid.pop_effect() {
            assert!(!state.apply_grid_effect(effect));
        }
        assert!(state.pending.is_some());

        let second = state.apply(
            command("m=0,q=1", &[3, 255]),
            &mut grid,
            Size { cols: 4, ..size },
        );
        assert!(second.changed);
        let placements = state.render_placements(&grid);
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].image_id, 84);
        assert_eq!((placements[0].viewport_row, placements[0].col), (2, 3));
    }

    #[test]
    fn explicit_dimensions_keep_full_occupancy_past_the_right_edge() {
        let size = Size {
            cols: 8,
            rows: 4,
            cell_width: 1.0,
            cell_height: 1.0,
        };
        let mut state = GraphicsState::default();
        let mut grid = Grid::new(size.cols, size.rows, 0, CursorStyle::Block);
        let upload = state.apply(
            command("a=t,f=32,s=4,v=1,i=85,q=1", &[255; 16]),
            &mut grid,
            size,
        );
        assert!(upload.changed);

        grid.set_cursor_position(1, 6);
        let placed = state.apply(
            command("a=p,i=85,p=1,c=5,r=2,z=7,C=1,q=1", &[]),
            &mut grid,
            size,
        );
        assert!(placed.changed);
        let placement = &state.render_placements(&grid)[0];
        assert_eq!(placement.display_cols, Some(5));
        assert_eq!(placement.occupied_cols, 5);
        assert!(placement_contains(&state.placements[0], 1, 10));

        let point_delete = state.apply(command("a=d,d=q,x=11,y=2,z=7,q=1", &[]), &mut grid, size);
        assert!(point_delete.changed);
        assert!(state.placements.is_empty());

        state.apply(
            command("a=p,i=85,p=2,c=5,r=2,C=1,q=1", &[]),
            &mut grid,
            size,
        );
        let column_delete = state.apply(command("a=d,d=x,x=11,q=1", &[]), &mut grid, size);
        assert!(column_delete.changed);
        assert!(state.placements.is_empty());

        state.apply(command("a=p,i=85,p=3,r=3,C=1,q=1", &[]), &mut grid, size);
        let row_sized = &state.render_placements(&grid)[0];
        assert_eq!(row_sized.display_cols, Some(12));
        assert_eq!(row_sized.occupied_cols, 12);
    }

    #[test]
    fn natural_size_placement_still_truncates_at_the_right_edge() {
        let size = Size {
            cols: 8,
            rows: 4,
            cell_width: 1.0,
            cell_height: 1.0,
        };
        let mut state = GraphicsState::default();
        let mut grid = Grid::new(size.cols, size.rows, 0, CursorStyle::Block);
        state.apply(
            command("a=t,f=32,s=4,v=1,i=86,q=1", &[255; 16]),
            &mut grid,
            size,
        );
        grid.set_cursor_position(1, 6);

        let placed = state.apply(command("a=p,i=86,p=1,C=1,q=1", &[]), &mut grid, size);

        assert!(placed.changed);
        let placement = &state.render_placements(&grid)[0];
        assert_eq!(placement.source_width, 2);
        assert_eq!(placement.display_cols, Some(2));
        assert_eq!(placement.occupied_cols, 2);
        assert!(!placement_contains(&state.placements[0], 1, 8));
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

    #[cfg(unix)]
    #[test]
    fn rejects_crc_valid_corrupt_idat_and_wrong_inflated_lengths() {
        let corrupt = png_document(1, 1, 8, 6, 0, vec![(*b"IDAT", vec![0x78, 0x01, 0xff])]);
        assert!(validate_png(&corrupt).is_err());

        let short = png_with_filtered_data(1, 1, 8, 6, 0, &[0, 0, 0, 0]);
        let long = png_with_filtered_data(1, 1, 8, 6, 0, &[0, 0, 0, 0, 0, 0]);
        assert!(validate_png(&short).is_err());
        assert!(validate_png(&long).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_invalid_png_scanline_filter() {
        let png = png_with_filtered_data(1, 1, 8, 6, 0, &[5, 0, 0, 0, 0]);
        assert!(validate_png(&png).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn accepts_consecutive_split_idat_but_rejects_interrupted_idat() {
        let compressed = zlib_store(&[0, 1, 2, 3, 4]);
        let split = compressed.len() / 2;
        let first = compressed[..split].to_vec();
        let second = compressed[split..].to_vec();
        let consecutive = png_document(
            1,
            1,
            8,
            6,
            0,
            vec![(*b"IDAT", first.clone()), (*b"IDAT", second.clone())],
        );
        assert_eq!(validate_png(&consecutive).unwrap(), (1, 1));

        let interrupted = png_document(
            1,
            1,
            8,
            6,
            0,
            vec![
                (*b"IDAT", first),
                (*b"tEXt", b"metadata".to_vec()),
                (*b"IDAT", second),
            ],
        );
        assert!(validate_png(&interrupted).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn enforces_indexed_palette_rules() {
        let compressed = zlib_store(&[0, 0]);
        let missing = png_document(1, 1, 1, 3, 0, vec![(*b"IDAT", compressed.clone())]);
        assert!(validate_png(&missing).is_err());

        let valid = png_document(
            1,
            1,
            1,
            3,
            0,
            vec![(*b"PLTE", vec![0, 0, 0]), (*b"IDAT", compressed.clone())],
        );
        assert_eq!(validate_png(&valid).unwrap(), (1, 1));

        let too_many_entries = png_document(
            1,
            1,
            1,
            3,
            0,
            vec![(*b"PLTE", vec![0; 9]), (*b"IDAT", compressed)],
        );
        assert!(validate_png(&too_many_entries).is_err());

        for color_type in [0, 4] {
            let filtered = if color_type == 0 {
                vec![0, 0]
            } else {
                vec![0, 0, 0]
            };
            let forbidden = png_document(
                1,
                1,
                8,
                color_type,
                0,
                vec![(*b"PLTE", vec![0, 0, 0]), (*b"IDAT", zlib_store(&filtered))],
            );
            assert!(validate_png(&forbidden).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_unknown_critical_chunks_but_allows_ancillary_chunks() {
        let compressed = zlib_store(&[0, 0, 0, 0, 0]);
        let critical = png_document(
            1,
            1,
            8,
            6,
            0,
            vec![(*b"ABCD", Vec::new()), (*b"IDAT", compressed.clone())],
        );
        assert!(validate_png(&critical).is_err());

        let ancillary = png_document(
            1,
            1,
            8,
            6,
            0,
            vec![(*b"abCd", b"metadata".to_vec()), (*b"IDAT", compressed)],
        );
        assert_eq!(validate_png(&ancillary).unwrap(), (1, 1));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_invalid_chunk_names_and_reserved_bits() {
        let compressed = zlib_store(&[0, 0, 0, 0, 0]);
        for kind in [*b"abcd", *b"a1Cd"] {
            let png = png_document(
                1,
                1,
                8,
                6,
                0,
                vec![(kind, Vec::new()), (*b"IDAT", compressed.clone())],
            );
            assert!(validate_png(&png).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn validates_adam7_edge_dimensions_and_pass_layout() {
        let one_pixel = png_with_filtered_data(1, 1, 8, 6, 1, &[0, 0, 0, 0, 0]);
        assert_eq!(validate_png(&one_pixel).unwrap(), (1, 1));

        // A 5x5 RGBA8 Adam7 image has 11 scanlines across all seven passes:
        // 100 pixel bytes plus 11 filter bytes.
        let filtered = vec![0; 111];
        let complete = png_with_filtered_data(5, 5, 8, 6, 1, &filtered);
        assert_eq!(validate_png(&complete).unwrap(), (5, 5));

        let short = png_with_filtered_data(5, 5, 8, 6, 1, &filtered[..110]);
        assert!(validate_png(&short).is_err());

        let mut invalid_late_filter = filtered;
        invalid_late_filter[90] = 5;
        let invalid = png_with_filtered_data(5, 5, 8, 6, 1, &invalid_late_filter);
        assert!(validate_png(&invalid).is_err());
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
    fn temporary_file_transfer_removes_protocol_file_after_bounded_read() {
        let temp = TestTempPath::new();
        let path = temp.file("tty-graphics-protocol-success", b"headerpixelsfooter");

        let data =
            resolve_transmission_data(&command("t=t,O=6,S=6", &[]), path_bytes(&path)).unwrap();

        assert_eq!(data, b"pixels");
        assert!(!path.exists());
    }

    #[test]
    fn temporary_file_transfer_preserves_protocol_file_when_read_fails() {
        let temp = TestTempPath::new();
        let path = temp.file("tty-graphics-protocol-read-error", b"x");

        let result = resolve_transmission_data(&command("t=t,O=2", &[]), path_bytes(&path));

        assert!(result.is_err());
        assert!(path.exists());
    }

    #[test]
    fn regular_file_transfer_never_removes_source_file() {
        let temp = TestTempPath::new();
        let path = temp.file("tty-graphics-protocol-regular", b"pixels");

        let data = resolve_transmission_data(&command("t=f", &[]), path_bytes(&path)).unwrap();

        assert_eq!(data, b"pixels");
        assert!(path.exists());
    }

    #[test]
    fn temporary_file_transfer_preserves_non_protocol_file() {
        let temp = TestTempPath::new();
        let path = temp.file("ordinary-image-data", b"pixels");

        let data = resolve_transmission_data(&command("t=t", &[]), path_bytes(&path)).unwrap();

        assert_eq!(data, b"pixels");
        assert!(path.exists());
    }

    #[test]
    fn temporary_file_transfer_does_not_trust_a_removed_parent_component() {
        let temp = TestTempPath::new();
        let path = temp.file("ordinary-image-data", b"pixels");
        let marker = temp.path.join("tty-graphics-protocol-decoy");
        std::fs::create_dir(&marker).unwrap();
        let disguised = marker.join("..").join("ordinary-image-data");

        let data = resolve_transmission_data(&command("t=t", &[]), path_bytes(&disguised)).unwrap();

        assert_eq!(data, b"pixels");
        assert!(path.exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn temporary_file_transfer_removes_protocol_file_from_dev_shm() {
        let root = PathBuf::from("/dev/shm");
        if !root.is_dir() {
            return;
        }
        let path = root.join(format!(
            "tty-graphics-protocol-termy-test-{}-{}",
            std::process::id(),
            TestTempPath::new()
                .path
                .file_name()
                .unwrap()
                .to_string_lossy()
        ));
        std::fs::write(&path, b"pixels").unwrap();

        let data = resolve_transmission_data(&command("t=t", &[]), path_bytes(&path)).unwrap();

        assert_eq!(data, b"pixels");
        assert!(!path.exists());
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
