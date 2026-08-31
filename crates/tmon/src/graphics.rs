use std::{
    borrow::Cow,
    collections::{HashMap, HashSet, VecDeque},
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    Size, decode_base64,
    grid::{Grid, GridEffect},
    inflate::{InflateError, OutputLimit, decompress_zlib as inflate_zlib},
};

mod placement;

const MAX_UPLOAD_BYTES: usize = 64 * 1024 * 1024;
const MAX_STORED_IMAGE_BYTES: usize = 128 * 1024 * 1024;
const MAX_STORED_IMAGES: usize = 4096;
const MAX_PNG_SCANLINE_BYTES: usize = MAX_UPLOAD_BYTES + MAX_DIMENSION as usize * 2;
pub(crate) const MAX_CONTROL_BYTES: usize = 4096;
const MAX_CONTROL_FIELDS: usize = 64;
pub(crate) const MAX_COMMAND_BYTES: usize =
    MAX_UPLOAD_BYTES.div_ceil(3) * 4 + MAX_CONTROL_BYTES + 1;
const MAX_DIMENSION: u32 = 32_768;
const MAX_PIXELS: u64 = (MAX_UPLOAD_BYTES / 4) as u64;
const MAX_PLACEMENTS: usize = 4096;
const MAX_RELATIVE_DEPTH: usize = 8;

#[derive(Debug)]
pub(crate) struct GraphicsCommand {
    control: Vec<(char, String)>,
    payload: Vec<u8>,
    oversized: bool,
}

impl GraphicsCommand {
    pub(crate) fn parse(mut bytes: Vec<u8>, oversized: bool) -> Self {
        // A missing separator must not turn a payload-sized command into a control string.
        // Inspect one byte past the limit so a separator at the exact boundary is accepted.
        let separator = bytes[..bytes.len().min(MAX_CONTROL_BYTES + 1)]
            .iter()
            .position(|byte| *byte == b';');
        let control_oversized = separator.is_none() && bytes.len() > MAX_CONTROL_BYTES;
        let control_end = separator.unwrap_or(bytes.len().min(MAX_CONTROL_BYTES));
        let field_count = bytes[..control_end]
            .split(|byte| *byte == b',')
            .take(MAX_CONTROL_FIELDS + 1)
            .count();
        let fields_oversized = field_count > MAX_CONTROL_FIELDS;
        let control = bytes[..control_end]
            .split(|byte| *byte == b',')
            .take(MAX_CONTROL_FIELDS)
            .filter_map(|field| {
                let (&key, value) = field.split_first()?;
                let value = value.strip_prefix(b"=")?;
                if !key.is_ascii() {
                    return None;
                }
                let value = std::str::from_utf8(value).ok()?;
                Some((char::from(key), value.to_owned()))
            })
            .collect();
        let oversized = oversized || control_oversized || fields_oversized;
        let payload = if oversized {
            // Drop an already-rejected payload with its original allocation instead of
            // shifting up to MAX_COMMAND_BYTES only for `apply` to discard it.
            Vec::new()
        } else if let Some(index) = separator {
            // Keep the potentially large payload in the parser's allocation. Draining the
            // small control prefix moves it in place instead of cloning it into another Vec.
            bytes.drain(..=index);
            bytes
        } else {
            Vec::new()
        };
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
    number: Option<u32>,
    generation: u64,
}

#[derive(Clone, Debug)]
struct Placement {
    serial: u64,
    alternate: bool,
    image_id: u32,
    placement_id: u32,
    location: PlacementLocation,
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
enum PlacementLocation {
    Direct {
        anchor_line: i64,
        col: usize,
    },
    Virtual,
    Relative {
        parent_image_id: u32,
        parent_placement_id: u32,
        horizontal_offset: i32,
        vertical_offset: i32,
    },
}

#[derive(Clone, Copy, Debug)]
struct UnicodePlaceholder {
    viewport_row: i64,
    col: usize,
    image_id_low: u32,
    image_id_high: u8,
    image_id: u32,
    placement_id: u32,
    image_row: u32,
    image_col: u32,
}

#[derive(Clone, Copy, Debug)]
enum ResolvedOrigin {
    Buffer { anchor_line: i64, col: usize },
    Viewport { row: i64, col: usize },
}

#[derive(Debug)]
struct PendingUpload {
    command: GraphicsCommand,
    decoded: Vec<u8>,
    context: UploadContext,
}

#[derive(Clone, Copy, Debug)]
struct UploadContext {
    cursor_col: usize,
    cursor_row: usize,
    history_size: usize,
    size: Size,
    alternate: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
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
    revision: u64,
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
                        let history_mapping_changed =
                            history_before != history_after && self.has_screen_placements(false);
                        let dropped = history_before
                            .saturating_add(count)
                            .saturating_sub(history_after);
                        self.rebase_primary_history(dropped) || history_mapping_changed
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
            GridEffect::EnteredAlternate => {
                let visible_changed = self.has_screen_placements(false);
                visible_changed | self.clear_screen_placements(true)
            }
            GridEffect::ExitedAlternate => {
                self.has_screen_placements(false) || self.has_screen_placements(true)
            }
            GridEffect::ClearViewport {
                alternate,
                history_size,
                rows,
                cols,
            } => self.clear_viewport(alternate, history_size, rows, cols),
            GridEffect::RebaseHistory { dropped } => self.rebase_primary_history(dropped),
            GridEffect::Reset => {
                let changed = !self.placements.is_empty() || self.pending.is_some();
                self.placements.clear();
                self.pending = None;
                changed
            }
        }
    }

    pub(crate) fn apply(
        &mut self,
        mut command: GraphicsCommand,
        grid: &mut Grid,
        size: Size,
    ) -> ApplyResult {
        if command.oversized {
            command.payload = Vec::new();
            let response_command = self
                .pending
                .take()
                .map_or(command, |pending| pending.command);
            return failure(
                &response_command,
                "EFBIG:image command exceeds storage limit",
            );
        }
        if command.value('i').is_some() && command.value('I').is_some() {
            command.payload = Vec::new();
            self.pending = None;
            return failure(
                &command,
                "EINVAL:image id and image number are mutually exclusive",
            );
        }
        let action = command.char_value('a').unwrap_or('t');
        if action == 'd' {
            command.payload = Vec::new();
            return self.delete(&command, grid);
        }
        if action == 'p' {
            command.payload = Vec::new();
            return self.put(&command, grid, size);
        }
        if !matches!(action, 't' | 'T' | 'q') {
            command.payload = Vec::new();
            return failure(&command, "EINVAL:unsupported graphics action");
        }

        let encoded = std::mem::take(&mut command.payload);
        let remaining_capacity = self.pending.as_ref().map_or(MAX_UPLOAD_BYTES, |pending| {
            MAX_UPLOAD_BYTES.saturating_sub(pending.decoded.len())
        });
        // Padding can reduce the exact decoded length by at most two bytes. Reject
        // larger chunks before allocating their decoded form so a nearly complete
        // continuation cannot transiently allocate another full upload.
        if base64_decoded_upper_bound(&encoded) > remaining_capacity.saturating_add(2) {
            drop(encoded);
            let response_command = self
                .pending
                .take()
                .map_or(command, |pending| pending.command);
            return failure(
                &response_command,
                "EFBIG:image payload exceeds storage limit",
            );
        }
        let decoded = decode_base64(&encoded);
        drop(encoded);
        let mut decoded = match decoded {
            Some(decoded) if decoded.len() <= remaining_capacity => decoded,
            Some(_) => {
                let response_command = self
                    .pending
                    .take()
                    .map_or(command, |pending| pending.command);
                return failure(
                    &response_command,
                    "EFBIG:image payload exceeds storage limit",
                );
            }
            None => {
                let response_command = self
                    .pending
                    .take()
                    .map_or(command, |pending| pending.command);
                return failure(&response_command, "EINVAL:invalid base64 payload");
            }
        };
        let more = command.u32_value('m').unwrap_or(0) == 1;
        if let Some(mut pending) = self.pending.take() {
            if command
                .control
                .iter()
                .any(|(key, _)| !matches!(key, 'm' | 'q'))
            {
                return failure(
                    &pending.command,
                    "EINVAL:continuation contains unsupported control data",
                );
            }
            pending.decoded.append(&mut decoded);
            if more {
                self.pending = Some(pending);
                return ApplyResult::default();
            }
            let mut first = pending.command;
            for (key, value) in command.control {
                if key == 'q' {
                    first.control.push((key, value));
                }
            }
            return self.finish_upload(first, pending.decoded, pending.context, grid);
        }

        let (cursor_col, cursor_row) = grid.cursor_position();
        let context = UploadContext {
            cursor_col,
            cursor_row,
            history_size: grid.history_size(),
            size,
            alternate: grid.alternate_screen_mode(),
        };
        if more {
            self.pending = Some(PendingUpload {
                command,
                decoded,
                context,
            });
            return ApplyResult::default();
        }
        self.finish_upload(command, decoded, context, grid)
    }

    fn finish_upload(
        &mut self,
        command: GraphicsCommand,
        decoded: Vec<u8>,
        context: UploadContext,
        grid: &mut Grid,
    ) -> ApplyResult {
        let data = match resolve_transmission_data(&command, decoded) {
            Ok(data) => data,
            Err(error) => return failure(&command, &error),
        };
        let data = match command.char_value('o') {
            None => data,
            Some('z') => match decompress_zlib(&command, data) {
                Ok(data) => data,
                Err(error) => return failure(&command, &error),
            },
            Some(_) => return failure(&command, "EINVAL:unsupported compression"),
        };
        let (png, width, height) = match normalize_image(&command, data) {
            Ok(image) => image,
            Err(error) => return failure(&command, &error),
        };
        if command.char_value('a').unwrap_or('t') == 'q' {
            return success(&command, false);
        }

        let requested_id = command.u32_value('i').unwrap_or(0);
        let image_number = command.u32_value('I').filter(|number| *number != 0);
        let (image_id, next_anonymous_id) = if requested_id == 0 {
            let (image_id, next_id) = self.next_anonymous_image_id();
            (image_id, Some(next_id))
        } else {
            (requested_id, None)
        };
        let next_generation = self.next_generation.wrapping_add(1).max(1);
        let image = StoredImage {
            png: Arc::from(png),
            width,
            height,
            number: image_number,
            generation: next_generation,
        };
        let byte_len = image.png.len();
        let Some(evictions) = self.quota_evictions_for_replacement(image_id, byte_len) else {
            return failure(&command, "ENOSPC:image storage quota exceeded");
        };

        let staged_placement = if command.char_value('a').unwrap_or('t') == 'T' {
            match self.placement_for_image(
                &image,
                image_id,
                &command,
                context.cursor_col,
                context.cursor_row,
                context.history_size,
                context.size,
                context.alternate,
                self.next_serial.wrapping_add(1).max(1),
            ) {
                Ok(placement) => Some(placement),
                Err(error) => return failure(&command, &error),
            }
        } else {
            None
        };

        for evicted_id in evictions {
            self.remove_image(evicted_id);
        }
        self.remove_image(image_id);
        self.next_generation = next_generation;
        if let Some(next_anonymous_id) = next_anonymous_id {
            self.next_anonymous_id = next_anonymous_id;
        }
        self.images.insert(image_id, image);
        self.insertion_order.push_back(image_id);
        self.stored_bytes = self.stored_bytes.saturating_add(byte_len);
        if let Some((placement, advance)) = staged_placement {
            self.commit_placement(placement);
            if context.alternate == grid.alternate_screen_mode()
                && let Some((cols, rows)) = advance
            {
                advance_cursor_after_placement(grid, cols, rows);
            }
        }
        success_for_image(&command, true, image_id)
    }

    fn put(&mut self, command: &GraphicsCommand, grid: &mut Grid, size: Size) -> ApplyResult {
        let Some(image_id) = self.resolve_image_id(command) else {
            return failure(command, "ENOENT:image id not found");
        };
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
                success_for_image(command, true, image_id)
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
        let next_serial = self.next_serial.wrapping_add(1).max(1);
        let image = self
            .images
            .get(&image_id)
            .ok_or_else(|| "ENOENT:image id not found".to_string())?;
        let (placement, advance) = self.placement_for_image(
            image,
            image_id,
            command,
            cursor_col,
            cursor_row,
            history_size,
            size,
            alternate,
            next_serial,
        )?;
        self.commit_placement(placement);
        Ok(advance)
    }

    #[allow(clippy::too_many_arguments)]
    fn placement_for_image(
        &self,
        image: &StoredImage,
        image_id: u32,
        command: &GraphicsCommand,
        cursor_col: usize,
        cursor_row: usize,
        history_size: usize,
        size: Size,
        alternate: bool,
        serial: u64,
    ) -> Result<(Placement, Option<(u32, u32)>), String> {
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
        let virtual_placement = command.u32_value('U').unwrap_or(0) == 1;
        let relative_parent = command.u32_value('P');
        if virtual_placement && relative_parent.is_some() {
            return Err("EINVAL:a virtual placement cannot be relative".into());
        }
        let location = if virtual_placement {
            PlacementLocation::Virtual
        } else if let Some(parent_image_id) = relative_parent {
            let parent_placement_id = command.u32_value('Q').unwrap_or(0);
            self.validate_relative_parent(
                alternate,
                image_id,
                placement_id,
                parent_image_id,
                parent_placement_id,
            )?;
            PlacementLocation::Relative {
                parent_image_id,
                parent_placement_id,
                horizontal_offset: command.i32_value('H').unwrap_or(0),
                vertical_offset: command.i32_value('V').unwrap_or(0),
            }
        } else {
            PlacementLocation::Direct {
                anchor_line: if alternate {
                    i64::try_from(cursor_row).unwrap_or(i64::MAX)
                } else {
                    i64::try_from(history_size)
                        .unwrap_or(i64::MAX)
                        .saturating_add(i64::try_from(cursor_row).unwrap_or(i64::MAX))
                },
                col: cursor_col,
            }
        };
        let placement = Placement {
            serial,
            alternate,
            image_id,
            placement_id,
            location,
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
        };
        let advances_cursor = !virtual_placement && relative_parent.is_none();
        let advance = (advances_cursor && command.u32_value('C').unwrap_or(0) == 0)
            .then_some((occupied_cols, occupied_rows));
        Ok((placement, advance))
    }

    fn commit_placement(&mut self, placement: Placement) {
        if placement.placement_id != 0 {
            self.placements.retain(|existing| {
                existing.alternate != placement.alternate
                    || existing.image_id != placement.image_id
                    || existing.placement_id != placement.placement_id
            });
        }
        self.next_serial = placement.serial;
        self.placements.push(placement);
        if self.placements.len() > MAX_PLACEMENTS {
            let overflow = self.placements.len() - MAX_PLACEMENTS;
            self.placements.drain(..overflow);
            self.remove_orphaned_relative_placements();
        }
    }

    fn delete(&mut self, command: &GraphicsCommand, grid: &Grid) -> ApplyResult {
        self.pending = None;
        let selector = command.char_value('d').unwrap_or('a');
        let free_data = selector.is_ascii_uppercase();
        let selector = selector.to_ascii_lowercase();
        let alternate = grid.alternate_screen_mode();
        let before = self.placements.len();
        let mut affected_images = HashSet::new();
        let mut resolved_image_id = None;
        match selector {
            'a' => self.placements.retain(|placement| {
                let remove = placement.alternate == alternate
                    && !matches!(placement.location, PlacementLocation::Virtual);
                if remove {
                    affected_images.insert(placement.image_id);
                }
                !remove
            }),
            'i' => {
                let image_id = command.u32_value('i').unwrap_or(0);
                let placement_id = command.u32_value('p').unwrap_or(0);
                if image_id != 0 {
                    affected_images.insert(image_id);
                    resolved_image_id = Some(image_id);
                }
                self.placements.retain(|placement| {
                    placement.image_id != image_id
                        || (placement_id != 0 && placement.placement_id != placement_id)
                });
            }
            'n' => {
                let placement_id = command.u32_value('p').unwrap_or(0);
                if let Some(image_id) = self.resolve_image_id(command) {
                    affected_images.insert(image_id);
                    resolved_image_id = Some(image_id);
                    self.placements.retain(|placement| {
                        placement.image_id != image_id
                            || (placement_id != 0 && placement.placement_id != placement_id)
                    });
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
                    let remove = placement.alternate == alternate
                        && placement_contains(placement, line, col);
                    if remove {
                        affected_images.insert(placement.image_id);
                    }
                    !remove
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
                    let remove = placement.alternate == alternate
                        && placement_contains(placement, row, col)
                        && (selector != 'q'
                            || z_index.is_none_or(|z_index| placement.z_index == z_index));
                    if remove {
                        affected_images.insert(placement.image_id);
                    }
                    !remove
                });
            }
            'r' => {
                let first = command.u32_value('x').unwrap_or(0);
                let last = command.u32_value('y').unwrap_or(u32::MAX);
                if first > last {
                    return failure(command, "EINVAL:invalid image id range");
                }
                affected_images.extend(
                    self.images
                        .keys()
                        .copied()
                        .filter(|image_id| (first..=last).contains(image_id)),
                );
                self.placements
                    .retain(|placement| !(first..=last).contains(&placement.image_id));
            }
            'x' => {
                let col = command.u32_value('x').unwrap_or(1).saturating_sub(1) as usize;
                self.placements.retain(|placement| {
                    let remove = match placement.location {
                        PlacementLocation::Direct {
                            col: placement_col, ..
                        } => {
                            placement.alternate == alternate
                                && col >= placement_col
                                && col
                                    < placement_col.saturating_add(placement.occupied_cols as usize)
                        }
                        PlacementLocation::Virtual | PlacementLocation::Relative { .. } => false,
                    };
                    if remove {
                        affected_images.insert(placement.image_id);
                    }
                    !remove
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
                    let remove = match placement.location {
                        PlacementLocation::Direct { anchor_line, .. } => {
                            placement.alternate == alternate
                                && row >= anchor_line
                                && row
                                    < anchor_line.saturating_add(i64::from(placement.occupied_rows))
                        }
                        PlacementLocation::Virtual | PlacementLocation::Relative { .. } => false,
                    };
                    if remove {
                        affected_images.insert(placement.image_id);
                    }
                    !remove
                });
            }
            'z' => {
                let z_index = command.i32_value('z').unwrap_or(0);
                self.placements.retain(|placement| {
                    let remove = placement.alternate == alternate
                        && !matches!(placement.location, PlacementLocation::Virtual)
                        && placement.z_index == z_index;
                    if remove {
                        affected_images.insert(placement.image_id);
                    }
                    !remove
                });
            }
            _ => return failure(command, "EINVAL:unsupported delete selector"),
        }
        self.remove_orphaned_relative_placements();
        if free_data {
            for image_id in affected_images {
                if !self
                    .placements
                    .iter()
                    .any(|placement| placement.image_id == image_id)
                {
                    self.remove_image(image_id);
                }
            }
        }
        if let Some(image_id) = resolved_image_id {
            success_for_image(command, before != self.placements.len(), image_id)
        } else {
            success(command, before != self.placements.len())
        }
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn has_placements(&self) -> bool {
        !self.placements.is_empty()
    }

    pub(crate) fn has_primary_placements(&self) -> bool {
        self.has_screen_placements(false)
    }

    pub(crate) fn clear_pending_upload(&mut self) {
        self.pending = None;
    }

    pub(crate) fn render_placements(&self, grid: &Grid) -> Vec<GraphicsRenderPlacement> {
        let alternate = grid.alternate_screen_mode();
        let history = i64::try_from(grid.history_size()).unwrap_or(i64::MAX);
        let offset = i64::try_from(grid.display_offset()).unwrap_or(i64::MAX);
        let rows = i64::try_from(grid.rows()).unwrap_or(i64::MAX);
        let placeholders = self.unicode_placeholders(grid);
        self.placements
            .iter()
            .filter(|placement| placement.alternate == alternate)
            .filter_map(|placement| {
                let origin = self.resolve_render_origin(placement, &placeholders)?;
                let image = self.images.get(&placement.image_id)?;
                let (viewport_row, col) = match origin {
                    ResolvedOrigin::Buffer { anchor_line, col } => {
                        let row = if alternate {
                            anchor_line
                        } else {
                            anchor_line.saturating_sub(history).saturating_add(offset)
                        };
                        (row, col)
                    }
                    ResolvedOrigin::Viewport { row, col } => (row, col),
                };
                if viewport_row.saturating_add(i64::from(placement.occupied_rows)) <= 0
                    || viewport_row >= rows
                    || col >= grid.cols()
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
                    col,
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
            if placement.alternate
                || !matches!(placement.location, PlacementLocation::Direct { .. })
            {
                return true;
            }
            let PlacementLocation::Direct { anchor_line, .. } = &mut placement.location else {
                unreachable!()
            };
            *anchor_line = anchor_line.saturating_sub(dropped);
            changed = true;
            anchor_line.saturating_add(i64::from(placement.occupied_rows)) > 0
        });
        if changed {
            self.remove_orphaned_relative_placements();
        }
        changed
    }

    fn preserve_primary_placements_across_partial_history_growth(&mut self, lines: usize) -> bool {
        if lines == 0 {
            return false;
        }
        let lines = i64::try_from(lines).unwrap_or(i64::MAX);
        let mut changed = false;
        for placement in self.placements.iter_mut().filter(|placement| {
            !placement.alternate && matches!(placement.location, PlacementLocation::Direct { .. })
        }) {
            if let PlacementLocation::Direct { anchor_line, .. } = &mut placement.location {
                *anchor_line = anchor_line.saturating_add(lines);
            }
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
            if placement.alternate != alternate
                || !matches!(placement.location, PlacementLocation::Direct { .. })
            {
                return true;
            }
            let PlacementLocation::Direct { anchor_line, .. } = &mut placement.location else {
                unreachable!()
            };
            let row = anchor_line.saturating_sub(base);
            if row < top || row > bottom {
                return true;
            }
            let new_row = row.saturating_sub(count);
            *anchor_line = base.saturating_add(new_row);
            changed = true;
            new_row.saturating_add(i64::from(placement.occupied_rows)) > top
        });
        if changed {
            self.remove_orphaned_relative_placements();
        }
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
            if placement.alternate != alternate
                || !matches!(placement.location, PlacementLocation::Direct { .. })
            {
                return true;
            }
            let PlacementLocation::Direct { anchor_line, .. } = &mut placement.location else {
                unreachable!()
            };
            let row = anchor_line.saturating_sub(base);
            if row < top || row > bottom {
                return true;
            }
            let new_row = row.saturating_add(count);
            *anchor_line = base.saturating_add(new_row);
            changed = true;
            new_row <= bottom
        });
        if changed {
            self.remove_orphaned_relative_placements();
        }
        changed
    }

    fn has_screen_placements(&self, alternate: bool) -> bool {
        self.placements
            .iter()
            .any(|placement| placement.alternate == alternate)
    }

    fn clear_screen_placements(&mut self, alternate: bool) -> bool {
        let before = self.placements.len();
        self.placements.retain(|placement| {
            placement.alternate != alternate
                || matches!(placement.location, PlacementLocation::Virtual)
        });
        self.remove_orphaned_relative_placements();
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.context.alternate == alternate)
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
            let PlacementLocation::Direct { anchor_line, col } = placement.location else {
                return true;
            };
            let placement_end = anchor_line.saturating_add(i64::from(placement.occupied_rows));
            let vertically_visible = anchor_line < viewport_end && placement_end > viewport_start;
            let horizontally_visible = col < cols && placement.occupied_cols > 0;
            !(vertically_visible && horizontally_visible)
        });
        self.remove_orphaned_relative_placements();
        before != self.placements.len()
    }

    fn resolve_image_id(&self, command: &GraphicsCommand) -> Option<u32> {
        if let Some(image_id) = command.u32_value('i').filter(|image_id| *image_id != 0) {
            return self.images.contains_key(&image_id).then_some(image_id);
        }
        let image_number = command.u32_value('I').filter(|number| *number != 0)?;
        self.images
            .iter()
            .filter(|(_, image)| image.number == Some(image_number))
            .max_by_key(|(_, image)| image.generation)
            .map(|(image_id, _)| *image_id)
    }

    fn next_anonymous_image_id(&self) -> (u32, u32) {
        let mut candidate = if self.next_anonymous_id == 0 {
            u32::MAX
        } else {
            self.next_anonymous_id
        };
        for _ in 0..=self.images.len() {
            if !self.images.contains_key(&candidate) {
                return (candidate, previous_image_id(candidate));
            }
            candidate = previous_image_id(candidate);
        }

        unreachable!("the bounded image store always has a free image ID")
    }

    #[cfg(test)]
    fn allocate_anonymous_id(&mut self) -> u32 {
        let (image_id, next_id) = self.next_anonymous_image_id();
        self.next_anonymous_id = next_id;
        image_id
    }

    fn quota_evictions_for_replacement(&self, image_id: u32, new_bytes: usize) -> Option<Vec<u32>> {
        let old_bytes = self
            .images
            .get(&image_id)
            .map_or(0, |image| image.png.len());
        let mut stored_bytes = self
            .stored_bytes
            .saturating_sub(old_bytes)
            .saturating_add(new_bytes);
        let mut image_count = self.images.len() + usize::from(!self.images.contains_key(&image_id));
        if stored_bytes <= MAX_STORED_IMAGE_BYTES && image_count <= MAX_STORED_IMAGES {
            return Some(Vec::new());
        }

        let placed = self
            .placements
            .iter()
            .map(|placement| placement.image_id)
            .collect::<HashSet<_>>();
        let mut evictions = Vec::new();
        for candidate in self
            .insertion_order
            .iter()
            .copied()
            .filter(|candidate| *candidate != image_id && !placed.contains(candidate))
        {
            if stored_bytes <= MAX_STORED_IMAGE_BYTES && image_count <= MAX_STORED_IMAGES {
                break;
            }
            let Some(image) = self.images.get(&candidate) else {
                continue;
            };
            stored_bytes = stored_bytes.saturating_sub(image.png.len());
            image_count = image_count.saturating_sub(1);
            evictions.push(candidate);
        }
        (stored_bytes <= MAX_STORED_IMAGE_BYTES && image_count <= MAX_STORED_IMAGES)
            .then_some(evictions)
    }

    #[cfg(test)]
    fn enforce_quota(&mut self, protected: Option<u32>) -> bool {
        if self.stored_bytes <= MAX_STORED_IMAGE_BYTES && self.images.len() <= MAX_STORED_IMAGES {
            return true;
        }

        let placed = self
            .placements
            .iter()
            .map(|placement| placement.image_id)
            .collect::<HashSet<_>>();
        let candidates = self
            .insertion_order
            .iter()
            .copied()
            .filter(|candidate| protected != Some(*candidate) && !placed.contains(candidate))
            .collect::<Vec<_>>();
        for candidate in candidates {
            if self.stored_bytes <= MAX_STORED_IMAGE_BYTES && self.images.len() <= MAX_STORED_IMAGES
            {
                break;
            }
            self.remove_image(candidate);
        }
        self.stored_bytes <= MAX_STORED_IMAGE_BYTES && self.images.len() <= MAX_STORED_IMAGES
    }

    fn remove_image(&mut self, image_id: u32) {
        if let Some(image) = self.images.remove(&image_id) {
            self.stored_bytes = self.stored_bytes.saturating_sub(image.png.len());
        }
        self.placements
            .retain(|placement| placement.image_id != image_id);
        self.remove_orphaned_relative_placements();
        self.insertion_order.retain(|id| *id != image_id);
    }

    pub(crate) fn bump_revision(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
}

fn previous_image_id(image_id: u32) -> u32 {
    if image_id <= 1 {
        u32::MAX
    } else {
        image_id - 1
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
    let PlacementLocation::Direct {
        anchor_line,
        col: placement_col,
    } = placement.location
    else {
        return false;
    };
    line >= anchor_line
        && line < anchor_line.saturating_add(i64::from(placement.occupied_rows))
        && col >= placement_col
        && col < placement_col.saturating_add(placement.occupied_cols as usize)
}

fn success(command: &GraphicsCommand, changed: bool) -> ApplyResult {
    success_with_resolved_image(command, changed, None)
}

fn success_for_image(command: &GraphicsCommand, changed: bool, image_id: u32) -> ApplyResult {
    success_with_resolved_image(command, changed, Some(image_id))
}

fn success_with_resolved_image(
    command: &GraphicsCommand,
    changed: bool,
    image_id: Option<u32>,
) -> ApplyResult {
    ApplyResult {
        replies: response(command, true, "OK", image_id).unwrap_or_default(),
        changed,
    }
}

fn failure(command: &GraphicsCommand, message: &str) -> ApplyResult {
    ApplyResult {
        replies: response(command, false, message, None).unwrap_or_default(),
        changed: false,
    }
}

fn response(
    command: &GraphicsCommand,
    successful: bool,
    message: &str,
    resolved_image_id: Option<u32>,
) -> Option<Vec<u8>> {
    let quiet = command.u32_value('q').unwrap_or(0);
    if (successful && quiet >= 1) || (!successful && quiet >= 2) {
        return None;
    }
    let image_id = resolved_image_id
        .or_else(|| command.u32_value('i'))
        .filter(|image_id| *image_id != 0);
    let image_number = command.u32_value('I').filter(|number| *number != 0);
    let mut control = match (image_id, image_number) {
        (Some(image_id), Some(image_number)) => format!("i={image_id},I={image_number}"),
        (Some(image_id), None) => format!("i={image_id}"),
        (None, Some(image_number)) => format!("I={image_number}"),
        (None, None) => return None,
    };
    if let Some(placement_id) = command.u32_value('p') {
        control.push_str(&format!(",p={placement_id}"));
    }
    Some(format!("\x1b_G{control};{message}\x1b\\").into_bytes())
}

fn base64_decoded_upper_bound(input: &[u8]) -> usize {
    input
        .iter()
        .filter(|byte| !matches!(byte, b'\r' | b'\n' | b'\t' | b' '))
        .count()
        .div_ceil(4)
        .saturating_mul(3)
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
    // Reject FIFOs and devices before opening: a child controls this path and a
    // blocking FIFO open would otherwise stall the parser thread indefinitely.
    let initial_metadata =
        std::fs::metadata(path).map_err(|_| "ENOENT:unable to open image file".to_string())?;
    if !initial_metadata.is_file() {
        return Err("EINVAL:invalid image file".into());
    }
    let mut file = open_image_file(path)?;
    let metadata = file
        .metadata()
        .map_err(|_| "EIO:unable to inspect image file".to_string())?;
    if !metadata.is_file() || offset > metadata.len() {
        return Err("EINVAL:invalid image file".into());
    }
    let length = size
        .unwrap_or(metadata.len() - offset)
        .min(metadata.len() - offset);
    if length > MAX_UPLOAD_BYTES as u64 {
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

fn open_image_file(path: &Path) -> Result<File, String> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    const NONBLOCK: i32 = 0o4000;
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    const NONBLOCK: i32 = 0x0004;

    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    {
        use std::{fs::OpenOptions, os::unix::fs::OpenOptionsExt};

        OpenOptions::new()
            .read(true)
            .custom_flags(NONBLOCK)
            .open(path)
            .map_err(|_| "ENOENT:unable to open image file".to_string())
    }

    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    )))]
    File::open(path).map_err(|_| "ENOENT:unable to open image file".to_string())
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

include!("graphics/image.rs");

#[cfg(test)]
#[path = "graphics_tests.rs"]
mod tests;
