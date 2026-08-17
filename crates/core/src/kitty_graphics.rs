//! Renderer-neutral support for the Kitty terminal graphics protocol.
//!
//! The core owns parsing, upload assembly, storage, placement, deletion and
//! protocol replies. Hosts only turn the returned PNG bytes into textures.

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use flate2::read::ZlibDecoder;
use std::{
    collections::{HashMap, VecDeque},
    fs::File,
    io::{Cursor, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::TerminalSize;

const MAX_IMAGE_BYTES: usize = 128 * 1024 * 1024;
const MAX_COMMAND_BYTES: usize = MAX_IMAGE_BYTES * 2;
const MAX_DIMENSION: u32 = 32_768;
const MAX_PIXELS: u64 = (MAX_IMAGE_BYTES / 4) as u64;

#[derive(Clone, Debug)]
pub struct KittyGraphicsCommand {
    control: Vec<(char, String)>,
    payload: Vec<u8>,
    oversized: bool,
}

impl KittyGraphicsCommand {
    fn parse(bytes: Vec<u8>, oversized: bool) -> Self {
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
pub enum KittyGraphicsItem {
    Text(Vec<u8>),
    Command(KittyGraphicsCommand),
}

#[derive(Clone, Copy, Debug, Default)]
enum InterceptorState {
    #[default]
    Ground,
    Escape,
    ApcStart {
        c1: bool,
    },
    OtherApc,
    OtherApcEscape,
    Kitty,
    KittyEscape,
}

#[derive(Default)]
pub struct KittyGraphicsInterceptor {
    state: InterceptorState,
    command: Vec<u8>,
    oversized: bool,
}

impl KittyGraphicsInterceptor {
    pub fn process(&mut self, bytes: &[u8]) -> Vec<KittyGraphicsItem> {
        let mut items = Vec::new();
        let mut text = Vec::with_capacity(bytes.len());

        let flush_text = |items: &mut Vec<KittyGraphicsItem>, text: &mut Vec<u8>| {
            if !text.is_empty() {
                items.push(KittyGraphicsItem::Text(std::mem::take(text)));
            }
        };

        for &byte in bytes {
            match self.state {
                InterceptorState::Ground => match byte {
                    0x1b => self.state = InterceptorState::Escape,
                    0x9f => self.state = InterceptorState::ApcStart { c1: true },
                    _ => text.push(byte),
                },
                InterceptorState::Escape => {
                    if byte == b'_' {
                        self.state = InterceptorState::ApcStart { c1: false };
                    } else if byte == 0x1b {
                        text.push(0x1b);
                        self.state = InterceptorState::Escape;
                    } else {
                        text.push(0x1b);
                        text.push(byte);
                        self.state = InterceptorState::Ground;
                    }
                }
                InterceptorState::ApcStart { c1 } => {
                    if byte == b'G' {
                        flush_text(&mut items, &mut text);
                        self.command.clear();
                        self.oversized = false;
                        self.state = InterceptorState::Kitty;
                    } else {
                        if c1 {
                            text.push(0x9f);
                        } else {
                            text.extend_from_slice(b"\x1b_");
                        }
                        text.push(byte);
                        self.state = InterceptorState::OtherApc;
                    }
                }
                InterceptorState::OtherApc => {
                    text.push(byte);
                    if byte == 0x9c {
                        self.state = InterceptorState::Ground;
                    } else if byte == 0x1b {
                        self.state = InterceptorState::OtherApcEscape;
                    }
                }
                InterceptorState::OtherApcEscape => {
                    text.push(byte);
                    if byte == b'\\' || byte == 0x9c {
                        self.state = InterceptorState::Ground;
                    } else if byte != 0x1b {
                        self.state = InterceptorState::OtherApc;
                    }
                }
                InterceptorState::Kitty => {
                    if byte == 0x1b {
                        self.state = InterceptorState::KittyEscape;
                    } else if byte == 0x9c {
                        flush_text(&mut items, &mut text);
                        items.push(KittyGraphicsItem::Command(KittyGraphicsCommand::parse(
                            std::mem::take(&mut self.command),
                            self.oversized,
                        )));
                        self.state = InterceptorState::Ground;
                    } else if self.command.len() < MAX_COMMAND_BYTES {
                        self.command.push(byte);
                    } else {
                        self.oversized = true;
                    }
                }
                InterceptorState::KittyEscape => {
                    if byte == b'\\' {
                        flush_text(&mut items, &mut text);
                        items.push(KittyGraphicsItem::Command(KittyGraphicsCommand::parse(
                            std::mem::take(&mut self.command),
                            self.oversized,
                        )));
                        self.state = InterceptorState::Ground;
                    } else {
                        if self.command.len() < MAX_COMMAND_BYTES {
                            self.command.push(0x1b);
                            self.command.push(byte);
                        } else {
                            self.oversized = true;
                        }
                        self.state = InterceptorState::Kitty;
                    }
                }
            }
        }
        flush_text(&mut items, &mut text);
        items
    }
}

#[derive(Clone, Debug)]
struct StoredImage {
    png: Arc<[u8]>,
    width: u32,
    height: u32,
    byte_len: usize,
    generation: u64,
}

#[derive(Clone, Debug)]
struct Placement {
    placement_serial: u64,
    screen: KittyGraphicsScreen,
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
    command: KittyGraphicsCommand,
    decoded: Vec<u8>,
    screen: KittyGraphicsScreen,
    cursor_col: usize,
    cursor_row: usize,
    history_size: usize,
    size: TerminalSize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KittyGraphicsScreen {
    #[default]
    Primary,
    Alternate,
}

impl KittyGraphicsScreen {
    pub fn from_alternate_screen(alternate_screen: bool) -> Self {
        if alternate_screen {
            Self::Alternate
        } else {
            Self::Primary
        }
    }
}

#[derive(Clone, Debug)]
pub struct KittyGraphicsRenderPlacement {
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

#[derive(Default)]
pub struct KittyGraphicsApplyResult {
    pub response: Option<Vec<u8>>,
    pub cursor_advance: Option<(u32, u32)>,
    pub cursor_advance_screen: Option<KittyGraphicsScreen>,
    pub changed: bool,
}

#[derive(Default)]
pub struct KittyGraphicsState {
    images: HashMap<u32, StoredImage>,
    placements: Vec<Placement>,
    insertion_order: VecDeque<u32>,
    pending: Option<PendingUpload>,
    next_anonymous_id: u32,
    stored_bytes: usize,
    next_generation: u64,
    next_placement_serial: u64,
}

impl KittyGraphicsState {
    pub fn apply(
        &mut self,
        command: KittyGraphicsCommand,
        cursor_col: usize,
        cursor_row: usize,
        history_size: usize,
        size: TerminalSize,
    ) -> KittyGraphicsApplyResult {
        self.apply_on_screen(
            command,
            cursor_col,
            cursor_row,
            history_size,
            size,
            KittyGraphicsScreen::Primary,
        )
    }

    pub fn apply_on_screen(
        &mut self,
        command: KittyGraphicsCommand,
        cursor_col: usize,
        cursor_row: usize,
        history_size: usize,
        size: TerminalSize,
        screen: KittyGraphicsScreen,
    ) -> KittyGraphicsApplyResult {
        log::debug!(
            "kitty graphics command: a={:?} i={:?} p={:?} q={:?} m={:?} f={:?} t={:?} s={:?} v={:?} c={:?} r={:?} payload={}B cursor=({cursor_col},{cursor_row}) screen={screen:?}",
            command.char_value('a'),
            command.u32_value('i'),
            command.u32_value('p'),
            command.u32_value('q'),
            command.u32_value('m'),
            command.u32_value('f'),
            command.char_value('t'),
            command.u32_value('s'),
            command.u32_value('v'),
            command.u32_value('c'),
            command.u32_value('r'),
            command.payload.len(),
        );

        if command.oversized {
            let response_command = self
                .pending
                .take()
                .map_or_else(|| command.clone(), |pending| pending.command);
            return self.failure(
                &response_command,
                "EFBIG:image command exceeds storage limit",
            );
        }

        let action = command.char_value('a').unwrap_or('t');
        if action == 'd' {
            return self.delete(&command, cursor_col, cursor_row, history_size, screen);
        }
        if action == 'p' {
            return self.put(command, cursor_col, cursor_row, history_size, size, screen);
        }
        if !matches!(action, 't' | 'T' | 'q') {
            return self.failure(&command, "EINVAL:unsupported graphics action");
        }

        let decoded = match BASE64.decode(&command.payload) {
            Ok(decoded) if decoded.len() <= MAX_IMAGE_BYTES => decoded,
            Ok(_) => {
                let response_command = self
                    .pending
                    .take()
                    .map_or_else(|| command.clone(), |pending| pending.command);
                return self.failure(
                    &response_command,
                    "EFBIG:image payload exceeds storage limit",
                );
            }
            Err(_) => {
                let response_command = self
                    .pending
                    .take()
                    .map_or_else(|| command.clone(), |pending| pending.command);
                return self.failure(&response_command, "EINVAL:invalid base64 payload");
            }
        };

        let more = command.u32_value('m').unwrap_or(0) == 1;
        if let Some(mut pending) = self.pending.take() {
            if pending.decoded.len().saturating_add(decoded.len()) > MAX_IMAGE_BYTES {
                return self.failure(
                    &pending.command,
                    "EFBIG:image payload exceeds storage limit",
                );
            }
            pending.decoded.extend_from_slice(&decoded);
            if more {
                self.pending = Some(pending);
                return KittyGraphicsApplyResult::default();
            }
            let mut first = pending.command;
            for (key, value) in command.control {
                if key != 'm' {
                    first.control.push((key, value));
                }
            }
            return self.finish_upload(
                first,
                pending.decoded,
                pending.cursor_col,
                pending.cursor_row,
                pending.history_size,
                pending.size,
                pending.screen,
            );
        }

        if more {
            self.pending = Some(PendingUpload {
                command,
                decoded,
                screen,
                cursor_col,
                cursor_row,
                history_size,
                size,
            });
            return KittyGraphicsApplyResult::default();
        }
        self.finish_upload(
            command,
            decoded,
            cursor_col,
            cursor_row,
            history_size,
            size,
            screen,
        )
    }

    pub fn render_placements(
        &self,
        history_size: usize,
        display_offset: usize,
        rows: usize,
        cols: usize,
    ) -> Vec<KittyGraphicsRenderPlacement> {
        self.render_placements_on_screen(
            history_size,
            display_offset,
            rows,
            cols,
            KittyGraphicsScreen::Primary,
        )
    }

    pub fn render_placements_on_screen(
        &self,
        history_size: usize,
        display_offset: usize,
        rows: usize,
        cols: usize,
        screen: KittyGraphicsScreen,
    ) -> Vec<KittyGraphicsRenderPlacement> {
        let history_size = i64::try_from(history_size).unwrap_or(i64::MAX);
        let display_offset = i64::try_from(display_offset).unwrap_or(i64::MAX);
        let rows_i64 = i64::try_from(rows).unwrap_or(i64::MAX);
        self.placements
            .iter()
            .filter(|placement| placement.screen == screen)
            .filter_map(|placement| {
                let image = self.images.get(&placement.image_id)?;
                let viewport_row = placement
                    .anchor_line
                    .saturating_sub(history_size)
                    .saturating_add(display_offset);
                let bottom = viewport_row.saturating_add(i64::from(placement.occupied_rows));
                if bottom <= 0
                    || viewport_row >= rows_i64
                    || placement.col >= cols
                    || placement.occupied_cols == 0
                {
                    return None;
                }
                Some(KittyGraphicsRenderPlacement {
                    placement_serial: placement.placement_serial,
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

    pub fn clear_visible(&mut self) -> bool {
        self.clear_visible_on_screen(KittyGraphicsScreen::Primary)
    }

    pub fn has_placements(&self) -> bool {
        !self.placements.is_empty()
    }

    pub fn clear_visible_on_screen(&mut self, screen: KittyGraphicsScreen) -> bool {
        let before = self.placements.len();
        self.placements
            .retain(|placement| placement.screen != screen);
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.screen == screen)
        {
            self.pending = None;
        }
        before != self.placements.len()
    }

    pub fn clear_viewport_on_screen(
        &mut self,
        screen: KittyGraphicsScreen,
        history_size: usize,
        rows: usize,
        cols: usize,
    ) -> bool {
        if rows == 0 || cols == 0 {
            return false;
        }

        let viewport_start = i64::try_from(history_size).unwrap_or(i64::MAX);
        let viewport_end = viewport_start.saturating_add(i64::try_from(rows).unwrap_or(i64::MAX));
        let before = self.placements.len();
        self.placements.retain(|placement| {
            if placement.screen != screen {
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

    /// Move placements with a screen scroll that was not recorded in history.
    ///
    /// Normal primary-screen scrolling is represented by a growing history
    /// size and is handled by [`Self::render_placements`]. Alternate-screen,
    /// zero-history, and full-history grids rotate rows without growing that
    /// value, so their placements need the equivalent anchor adjustment here.
    pub fn scroll_up_without_history(&mut self, lines: usize) -> bool {
        self.scroll_up_without_history_on_screen(lines, KittyGraphicsScreen::Primary)
    }

    pub fn scroll_up_without_history_on_screen(
        &mut self,
        lines: usize,
        screen: KittyGraphicsScreen,
    ) -> bool {
        if lines == 0
            || !self
                .placements
                .iter()
                .any(|placement| placement.screen == screen)
        {
            return false;
        }

        let lines = i64::try_from(lines).unwrap_or(i64::MAX);
        for placement in self
            .placements
            .iter_mut()
            .filter(|placement| placement.screen == screen)
        {
            placement.anchor_line = placement.anchor_line.saturating_sub(lines);
        }
        self.placements.retain(|placement| {
            placement.screen != screen
                || placement
                    .anchor_line
                    .saturating_add(i64::from(placement.occupied_rows))
                    > 0
        });
        true
    }

    pub fn preserve_primary_placements_across_partial_history_growth(
        &mut self,
        lines: usize,
    ) -> bool {
        if lines == 0
            || !self
                .placements
                .iter()
                .any(|placement| placement.screen == KittyGraphicsScreen::Primary)
        {
            return false;
        }

        let lines = i64::try_from(lines).unwrap_or(i64::MAX);
        for placement in self
            .placements
            .iter_mut()
            .filter(|placement| placement.screen == KittyGraphicsScreen::Primary)
        {
            // Alacritty grows history when a partial DECSTBM region starts at
            // the top. Kitty placements are not region-aware, so cancel that
            // global history offset rather than moving fixed footer images.
            placement.anchor_line = placement.anchor_line.saturating_add(lines);
        }
        true
    }

    fn finish_upload(
        &mut self,
        command: KittyGraphicsCommand,
        decoded: Vec<u8>,
        cursor_col: usize,
        cursor_row: usize,
        history_size: usize,
        size: TerminalSize,
        screen: KittyGraphicsScreen,
    ) -> KittyGraphicsApplyResult {
        let data = match self.resolve_transmission_data(&command, decoded) {
            Ok(data) => data,
            Err(error) => return self.failure(&command, &error),
        };
        let data = match command.char_value('o') {
            None => data,
            Some('z') => match decompress_zlib(&data) {
                Ok(data) => data,
                Err(error) => return self.failure(&command, &error),
            },
            Some(_) => return self.failure(&command, "EINVAL:unsupported compression"),
        };
        let (png, width, height) = match normalize_image(&command, &data) {
            Ok(image) => image,
            Err(error) => return self.failure(&command, &error),
        };

        if command.char_value('a').unwrap_or('t') == 'q' {
            return self.success(&command, false, None, screen);
        }

        let requested_id = command.u32_value('i').unwrap_or(0);
        let image_id = if requested_id == 0 {
            self.allocate_anonymous_id()
        } else {
            requested_id
        };
        self.remove_image(image_id);
        let byte_len = png.len();
        self.next_generation = self.next_generation.wrapping_add(1).max(1);
        self.images.insert(
            image_id,
            StoredImage {
                png: Arc::from(png),
                width,
                height,
                byte_len,
                generation: self.next_generation,
            },
        );
        self.insertion_order.push_back(image_id);
        self.stored_bytes = self.stored_bytes.saturating_add(byte_len);
        if !self.enforce_quota(Some(image_id)) {
            self.remove_image(image_id);
            return self.failure(&command, "ENOSPC:image storage quota exceeded");
        }

        let display = command.char_value('a').unwrap_or('t') == 'T';
        let cursor_advance = if display {
            match self.add_placement(
                image_id,
                &command,
                cursor_col,
                cursor_row,
                history_size,
                size,
                screen,
            ) {
                Ok(advance) => advance,
                Err(error) => {
                    self.remove_image(image_id);
                    return self.failure(&command, &error);
                }
            }
        } else {
            None
        };
        self.success(&command, true, cursor_advance, screen)
    }

    fn put(
        &mut self,
        command: KittyGraphicsCommand,
        cursor_col: usize,
        cursor_row: usize,
        history_size: usize,
        size: TerminalSize,
        screen: KittyGraphicsScreen,
    ) -> KittyGraphicsApplyResult {
        let image_id = command.u32_value('i').unwrap_or(0);
        if image_id == 0 || !self.images.contains_key(&image_id) {
            return self.failure(&command, "ENOENT:image id not found");
        }
        match self.add_placement(
            image_id,
            &command,
            cursor_col,
            cursor_row,
            history_size,
            size,
            screen,
        ) {
            Ok(advance) => self.success(&command, true, advance, screen),
            Err(error) => self.failure(&command, &error),
        }
    }

    fn add_placement(
        &mut self,
        image_id: u32,
        command: &KittyGraphicsCommand,
        cursor_col: usize,
        cursor_row: usize,
        history_size: usize,
        size: TerminalSize,
        screen: KittyGraphicsScreen,
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
        // Columns remaining from the cursor to the right edge of the screen.
        // Used for Kitty's natural-size right-edge truncation rule.
        let available_cols = u32::try_from(size.cols as usize)
            .unwrap_or(u32::MAX)
            .saturating_sub(u32::try_from(cursor_col).unwrap_or(u32::MAX))
            .max(1);
        let available_width_px = available_cols as f32 * cell_width;

        // Optional source-rectangle width after natural-size truncation. When
        // the client omits c/r, Kitty shows the image at 1:1 pixels and
        // truncates (not scales) anything past the available width.
        let mut placed_source_width = source_width;
        let (display_cols, display_rows) = match (requested_cols, requested_rows) {
            (Some(cols), Some(rows)) => (Some(cols), Some(rows)),
            (Some(cols), None) => {
                let width_px = cols as f32 * cell_width;
                let height_px = width_px * source_height as f32 / source_width as f32;
                (
                    Some(cols),
                    Some((height_px / cell_height).ceil().max(1.0) as u32),
                )
            }
            (None, Some(rows)) => {
                let height_px = rows as f32 * cell_height;
                let width_px = height_px * source_width as f32 / source_height as f32;
                (
                    Some((width_px / cell_width).ceil().max(1.0) as u32),
                    Some(rows),
                )
            }
            (None, None) => {
                if source_width as f32 > available_width_px {
                    placed_source_width = available_width_px.floor().max(1.0) as u32;
                }
                let cols = (placed_source_width as f32 / cell_width).ceil().max(1.0) as u32;
                let rows = (source_height as f32 / cell_height).ceil().max(1.0) as u32;
                // Materialize explicit cell size so hosts paint with one path
                // (cell box) instead of raw PNG pixel dimensions.
                (Some(cols.min(available_cols)), Some(rows))
            }
        };
        let occupied_cols = display_cols.unwrap_or(1);
        let occupied_rows = display_rows.unwrap_or(1);
        let placement_id = command.u32_value('p').unwrap_or(0);
        if placement_id != 0 {
            self.placements.retain(|placement| {
                placement.screen != screen
                    || placement.image_id != image_id
                    || placement.placement_id != placement_id
            });
        }
        self.next_placement_serial = self.next_placement_serial.wrapping_add(1).max(1);
        self.placements.push(Placement {
            placement_serial: self.next_placement_serial,
            screen,
            image_id,
            placement_id,
            anchor_line: i64::try_from(history_size)
                .unwrap_or(i64::MAX)
                .saturating_add(i64::try_from(cursor_row).unwrap_or(i64::MAX)),
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
        Ok((command.u32_value('C').unwrap_or(0) == 0).then_some((occupied_cols, occupied_rows)))
    }

    fn delete(
        &mut self,
        command: &KittyGraphicsCommand,
        cursor_col: usize,
        cursor_row: usize,
        history_size: usize,
        screen: KittyGraphicsScreen,
    ) -> KittyGraphicsApplyResult {
        self.pending = None;
        let selector = command.char_value('d').unwrap_or('a');
        let free_data = selector.is_ascii_uppercase();
        let selector = selector.to_ascii_lowercase();
        let before = self.placements.len();
        match selector {
            'a' => self
                .placements
                .retain(|placement| placement.screen != screen),
            'i' => {
                let image_id = command.u32_value('i').unwrap_or(0);
                let placement_id = command.u32_value('p').unwrap_or(0);
                self.placements.retain(|placement| {
                    placement.screen != screen
                        || placement.image_id != image_id
                        || (placement_id != 0 && placement.placement_id != placement_id)
                });
                if free_data && !self.placements.iter().any(|p| p.image_id == image_id) {
                    self.remove_image(image_id);
                }
            }
            'c' => {
                let line = i64::try_from(history_size)
                    .unwrap_or(i64::MAX)
                    .saturating_add(i64::try_from(cursor_row).unwrap_or(i64::MAX));
                self.placements
                    .retain(|p| p.screen != screen || !placement_contains(p, line, cursor_col));
            }
            'p' | 'q' => {
                let col = command.u32_value('x').unwrap_or(1).saturating_sub(1) as usize;
                let row = command.u32_value('y').unwrap_or(1).saturating_sub(1) as i64
                    + i64::try_from(history_size).unwrap_or(i64::MAX);
                let z = command.i32_value('z');
                self.placements.retain(|p| {
                    p.screen != screen
                        || !placement_contains(p, row, col)
                        || (selector == 'q' && z.is_some_and(|z| p.z_index != z))
                });
            }
            'x' => {
                let col = command.u32_value('x').unwrap_or(1).saturating_sub(1) as usize;
                self.placements.retain(|p| {
                    p.screen != screen
                        || col < p.col
                        || col >= p.col.saturating_add(p.occupied_cols as usize)
                });
            }
            'y' => {
                let row = command.u32_value('y').unwrap_or(1).saturating_sub(1) as i64
                    + i64::try_from(history_size).unwrap_or(i64::MAX);
                self.placements.retain(|p| {
                    p.screen != screen
                        || row < p.anchor_line
                        || row >= p.anchor_line + i64::from(p.occupied_rows)
                });
            }
            'z' => {
                let z = command.i32_value('z').unwrap_or(0);
                self.placements
                    .retain(|p| p.screen != screen || p.z_index != z);
            }
            _ => return self.failure(command, "EINVAL:unsupported delete selector"),
        }
        if free_data {
            self.drop_unplaced_images();
        }
        self.success(command, before != self.placements.len(), None, screen)
    }

    fn resolve_transmission_data(
        &self,
        command: &KittyGraphicsCommand,
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
                    command.u32_value('O').unwrap_or(0) as u64,
                    command
                        .u32_value('S')
                        .filter(|size| *size > 0)
                        .map(u64::from),
                );
                if temporary && temporary_path_can_be_removed(&path) {
                    let _ = std::fs::remove_file(&path);
                }
                result
            }
            's' => Err("ENOTSUP:shared-memory transmission is not supported".into()),
            _ => Err("EINVAL:unsupported transmission medium".into()),
        }
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
                continue;
            }
            self.remove_image(candidate);
        }
        self.stored_bytes <= MAX_IMAGE_BYTES
    }

    fn drop_unplaced_images(&mut self) {
        let ids: Vec<u32> = self
            .images
            .keys()
            .copied()
            .filter(|id| {
                !self
                    .placements
                    .iter()
                    .any(|placement| placement.image_id == *id)
            })
            .collect();
        for id in ids {
            self.remove_image(id);
        }
    }

    fn remove_image(&mut self, image_id: u32) {
        if let Some(image) = self.images.remove(&image_id) {
            self.stored_bytes = self.stored_bytes.saturating_sub(image.byte_len);
        }
        self.placements
            .retain(|placement| placement.image_id != image_id);
        self.insertion_order.retain(|id| *id != image_id);
    }

    fn success(
        &self,
        command: &KittyGraphicsCommand,
        changed: bool,
        cursor_advance: Option<(u32, u32)>,
        screen: KittyGraphicsScreen,
    ) -> KittyGraphicsApplyResult {
        log::debug!(
            "kitty graphics ok: a={:?} i={:?} p={:?} changed={changed} cursor_advance={cursor_advance:?}",
            command.char_value('a'),
            command.u32_value('i'),
            command.u32_value('p'),
        );
        KittyGraphicsApplyResult {
            response: response(command, true, "OK"),
            cursor_advance,
            cursor_advance_screen: cursor_advance.map(|_| screen),
            changed,
        }
    }

    fn failure(&self, command: &KittyGraphicsCommand, message: &str) -> KittyGraphicsApplyResult {
        // Logged even when q=2 suppresses the wire reply, so silent client
        // failures remain visible in host logs.
        log::warn!(
            "kitty graphics error: a={:?} i={:?} p={:?} q={:?} {message}",
            command.char_value('a'),
            command.u32_value('i'),
            command.u32_value('p'),
            command.u32_value('q'),
        );
        KittyGraphicsApplyResult {
            response: response(command, false, message),
            ..KittyGraphicsApplyResult::default()
        }
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

fn response(command: &KittyGraphicsCommand, success: bool, message: &str) -> Option<Vec<u8>> {
    let quiet = command.u32_value('q').unwrap_or(0);
    if (success && quiet >= 1) || (!success && quiet >= 2) {
        return None;
    }
    let image_id = command.u32_value('i').or_else(|| command.u32_value('I'))?;
    let mut control = format!("i={image_id}");
    if let Some(placement_id) = command.u32_value('p') {
        control.push_str(&format!(",p={placement_id}"));
    }
    Some(format!("\x1b_G{control};{message}\x1b\\").into_bytes())
}

fn normalize_image(
    command: &KittyGraphicsCommand,
    data: &[u8],
) -> Result<(Vec<u8>, u32, u32), String> {
    match command.u32_value('f').unwrap_or(32) {
        100 => {
            let decoder = png::Decoder::new(Cursor::new(data));
            let mut reader = decoder
                .read_info()
                .map_err(|_| "EINVAL:invalid PNG image".to_string())?;
            let (width, height) = (reader.info().width, reader.info().height);
            validate_dimensions(width, height, 4)?;
            let decoded_len = reader
                .output_buffer_size()
                .filter(|length| *length <= MAX_IMAGE_BYTES)
                .ok_or_else(|| "EFBIG:decoded PNG exceeds storage limit".to_string())?;
            let mut decoded = vec![0; decoded_len];
            reader
                .next_frame(&mut decoded)
                .and_then(|_| reader.finish())
                .map_err(|_| "EINVAL:invalid PNG image".to_string())?;
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
            let mut png = Vec::new();
            {
                let mut encoder = png::Encoder::new(&mut png, width, height);
                encoder.set_depth(png::BitDepth::Eight);
                encoder.set_color(if channels == 3 {
                    png::ColorType::Rgb
                } else {
                    png::ColorType::Rgba
                });
                encoder
                    .write_header()
                    .and_then(|mut writer| writer.write_image_data(data))
                    .map_err(|_| "EINVAL:failed to encode pixel data".to_string())?;
            }
            Ok((png, width, height))
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

fn decompress_zlib(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    ZlibDecoder::new(data)
        .take(MAX_IMAGE_BYTES as u64 + 1)
        .read_to_end(&mut output)
        .map_err(|_| "EINVAL:invalid zlib payload".to_string())?;
    if output.len() > MAX_IMAGE_BYTES {
        return Err("EFBIG:decompressed image exceeds storage limit".into());
    }
    Ok(output)
}

fn read_regular_file(path: &Path, offset: u64, size: Option<u64>) -> Result<Vec<u8>, String> {
    let mut file = File::open(path).map_err(|_| "ENOENT:unable to open image file".to_string())?;
    let metadata = file
        .metadata()
        .map_err(|_| "EIO:unable to inspect image file".to_string())?;
    if !metadata.file_type().is_file() {
        return Err("EINVAL:image path is not a regular file".into());
    }
    if offset > metadata.len() {
        return Err("EINVAL:file offset is past end of file".into());
    }
    let available = metadata.len() - offset;
    let length = size.unwrap_or(available).min(available);
    if length > MAX_IMAGE_BYTES as u64 {
        return Err("EFBIG:image file exceeds storage limit".into());
    }
    file.seek(SeekFrom::Start(offset))
        .map_err(|_| "EIO:unable to seek image file".to_string())?;
    let mut data = Vec::with_capacity(length as usize);
    file.take(length)
        .read_to_end(&mut data)
        .map_err(|_| "EIO:unable to read image file".to_string())?;
    Ok(data)
}

fn temporary_path_can_be_removed(path: &Path) -> bool {
    let path_text = path.to_string_lossy();
    if !path_text.contains("tty-graphics-protocol") {
        return false;
    }
    let Ok(canonical) = path.canonicalize() else {
        return false;
    };
    let mut roots = vec![PathBuf::from("/tmp"), PathBuf::from("/private/tmp")];
    roots.push(std::env::temp_dir());
    roots.into_iter().any(|root| {
        root.canonicalize()
            .is_ok_and(|root| canonical.starts_with(root))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn size() -> TerminalSize {
        TerminalSize {
            cols: 80,
            rows: 24,
            cell_width: 10.0,
            cell_height: 20.0,
        }
    }

    fn command(control: &str, payload: &[u8]) -> KittyGraphicsCommand {
        KittyGraphicsCommand::parse(
            [control.as_bytes(), b";", BASE64.encode(payload).as_bytes()].concat(),
            false,
        )
    }

    fn one_pixel_png() -> Vec<u8> {
        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&[255, 0, 0, 255])
                .unwrap();
        }
        png
    }

    #[test]
    fn interceptor_removes_kitty_apc_and_preserves_text() {
        let png = one_pixel_png();
        let sequence = format!(
            "before\x1b_Ga=T,f=100,i=7;{}\x1b\\after",
            BASE64.encode(png)
        );
        let mut interceptor = KittyGraphicsInterceptor::default();
        let items = interceptor.process(sequence.as_bytes());
        assert!(matches!(&items[0], KittyGraphicsItem::Text(text) if text == b"before"));
        assert!(matches!(&items[1], KittyGraphicsItem::Command(_)));
        assert!(matches!(&items[2], KittyGraphicsItem::Text(text) if text == b"after"));
    }

    #[test]
    fn interceptor_handles_sequence_split_across_reads() {
        let mut interceptor = KittyGraphicsInterceptor::default();
        assert!(
            matches!(interceptor.process(b"x\x1b_").as_slice(), [KittyGraphicsItem::Text(text)] if text == b"x")
        );
        assert!(
            interceptor
                .process(b"Ga=T,f=32,s=1,v=1,i=9;//AA")
                .is_empty()
        );
        let items = interceptor.process(b"/w==\x1b\\y");
        assert!(matches!(&items[0], KittyGraphicsItem::Command(_)));
        assert!(matches!(&items[1], KittyGraphicsItem::Text(text) if text == b"y"));
    }

    #[test]
    fn interceptor_preserves_non_kitty_apc_sequences() {
        let mut interceptor = KittyGraphicsInterceptor::default();
        let input = b"a\x1b_not-kitty\x1b\\b";
        let items = interceptor.process(input);
        assert!(matches!(items.as_slice(), [KittyGraphicsItem::Text(text)] if text == input));
    }

    #[test]
    fn uploads_raw_rgba_and_places_at_cursor() {
        let mut state = KittyGraphicsState::default();
        let result = state.apply(
            command("a=T,f=32,s=1,v=1,i=42,c=2,r=3", &[1, 2, 3, 255]),
            4,
            5,
            10,
            size(),
        );
        assert!(result.changed);
        assert_eq!(result.cursor_advance, Some((2, 3)));
        assert_eq!(result.response.unwrap(), b"\x1b_Gi=42;OK\x1b\\");
        let placements = state.render_placements(10, 0, 24, 80);
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].placement_serial, 1);
        assert_eq!(placements[0].viewport_row, 5);
        assert_eq!(placements[0].col, 4);
        assert_eq!(placements[0].occupied_cols, 2);
        assert_eq!(placements[0].occupied_rows, 3);
        assert!(placements[0].png.starts_with(b"\x89PNG"));
    }

    #[test]
    fn natural_size_uses_cell_metrics_for_occupancy() {
        // 100×40 px image, 10×20 cell → 10 cols × 2 rows at 1:1.
        let pixels = vec![0u8; 100 * 40 * 4];
        let mut state = KittyGraphicsState::default();
        let result = state.apply(
            command("a=T,f=32,s=100,v=40,i=50,q=1,C=0", &pixels),
            0,
            0,
            0,
            size(),
        );
        assert!(result.changed);
        assert_eq!(result.cursor_advance, Some((10, 2)));
        let placements = state.render_placements(0, 0, 24, 80);
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].occupied_cols, 10);
        assert_eq!(placements[0].occupied_rows, 2);
        assert_eq!(placements[0].display_cols, Some(10));
        assert_eq!(placements[0].display_rows, Some(2));
        assert_eq!(placements[0].source_width, 100);
        assert_eq!(placements[0].source_height, 40);
    }

    #[test]
    fn natural_size_truncates_to_available_width_not_scale() {
        // 500×20 px image at col 70 on an 80-col grid with 10px cells:
        // only 10 cols / 100px remain → truncate source width to 100, occupy 10.
        let pixels = vec![0u8; 500 * 20 * 4];
        let mut state = KittyGraphicsState::default();
        let result = state.apply(
            command("a=T,f=32,s=500,v=20,i=51,q=1,C=0", &pixels),
            70,
            0,
            0,
            size(),
        );
        assert!(result.changed);
        assert_eq!(result.cursor_advance, Some((10, 1)));
        let placements = state.render_placements(0, 0, 24, 80);
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].col, 70);
        assert_eq!(placements[0].occupied_cols, 10);
        assert_eq!(placements[0].occupied_rows, 1);
        assert_eq!(
            placements[0].source_width, 100,
            "right-edge truncation must crop source width, not scale the whole image"
        );
        assert_eq!(placements[0].source_height, 20);
    }

    #[test]
    fn explicit_cell_size_is_not_truncated_to_available_width() {
        // Client-requested c/r is authoritative (Grok fit_image_to_cells path).
        let pixels = vec![0u8; 500 * 20 * 4];
        let mut state = KittyGraphicsState::default();
        let result = state.apply(
            command("a=T,f=32,s=500,v=20,i=52,c=40,r=4,q=1,C=1", &pixels),
            70,
            0,
            0,
            size(),
        );
        assert!(result.changed);
        assert_eq!(result.cursor_advance, None);
        let placements = state.render_placements(0, 0, 24, 80);
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].occupied_cols, 40);
        assert_eq!(placements[0].occupied_rows, 4);
        assert_eq!(placements[0].source_width, 500);
    }

    #[test]
    fn place_with_only_cols_derives_rows_from_aspect_ratio() {
        let pixels = vec![0u8; 100 * 40 * 4];
        let mut state = KittyGraphicsState::default();
        state.apply(
            command("a=t,f=32,s=100,v=40,i=53,q=1", &pixels),
            0,
            0,
            0,
            size(),
        );
        let result = state.apply(command("a=p,i=53,c=20,q=1,C=1", &[]), 0, 0, 0, size());
        assert!(result.changed);
        let placements = state.render_placements(0, 0, 24, 80);
        assert_eq!(placements.len(), 1);
        // width 20 cols = 200px; height keeps aspect → 80px → 4 rows.
        assert_eq!(placements[0].occupied_cols, 20);
        assert_eq!(placements[0].occupied_rows, 4);
    }

    #[test]
    fn each_placement_gets_a_stable_monotonic_serial() {
        let png = one_pixel_png();
        let mut state = KittyGraphicsState::default();
        state.apply(command("a=t,f=100,i=3,q=1", &png), 0, 0, 0, size());
        state.apply(command("a=p,i=3,q=1", &[]), 0, 0, 0, size());
        state.apply(command("a=p,i=3,q=1", &[]), 2, 0, 0, size());

        let placements = state.render_placements(0, 0, 24, 80);
        assert_eq!(placements.len(), 2);
        assert_eq!(placements[0].placement_serial, 1);
        assert_eq!(placements[1].placement_serial, 2);
    }

    #[test]
    fn clearing_a_viewport_preserves_scrollback_and_other_screens() {
        let png = one_pixel_png();
        let mut state = KittyGraphicsState::default();
        state.apply(
            command("a=T,f=100,i=3,c=2,r=2,C=1,q=1", &png),
            0,
            0,
            0,
            size(),
        );
        state.apply(command("a=p,i=3,c=2,r=2,C=1,q=1", &[]), 0, 1, 10, size());
        state.apply_on_screen(
            command("a=p,i=3,c=2,r=2,C=1,q=1", &[]),
            0,
            0,
            0,
            size(),
            KittyGraphicsScreen::Alternate,
        );

        assert!(state.clear_viewport_on_screen(KittyGraphicsScreen::Primary, 10, 24, 80,));
        assert!(
            state
                .render_placements_on_screen(10, 0, 24, 80, KittyGraphicsScreen::Primary)
                .is_empty()
        );
        assert_eq!(
            state
                .render_placements_on_screen(10, 10, 24, 80, KittyGraphicsScreen::Primary)
                .len(),
            1,
            "placements already in scrollback must survive ED2"
        );
        assert_eq!(
            state
                .render_placements_on_screen(0, 0, 24, 80, KittyGraphicsScreen::Alternate)
                .len(),
            1,
            "clearing the primary viewport must not affect the alternate screen"
        );
    }

    #[test]
    fn assembles_chunked_upload_before_displaying() {
        let png = one_pixel_png();
        let split = png.len() / 2;
        let first = command("a=T,f=100,i=8,m=1", &png[..split]);
        let second = command("m=0", &png[split..]);
        let mut state = KittyGraphicsState::default();
        assert!(!state.apply(first, 0, 0, 0, size()).changed);
        assert!(state.render_placements(0, 0, 24, 80).is_empty());
        assert!(state.apply(second, 0, 0, 0, size()).changed);
        assert_eq!(state.render_placements(0, 0, 24, 80).len(), 1);
    }

    #[test]
    fn chunked_upload_keeps_its_origin_screen_and_cursor() {
        let png = one_pixel_png();
        let split = png.len() / 2;
        let first = command("a=T,f=100,i=88,c=2,r=3,m=1", &png[..split]);
        let second = command("m=0", &png[split..]);
        let mut state = KittyGraphicsState::default();

        let first_result =
            state.apply_on_screen(first, 4, 5, 10, size(), KittyGraphicsScreen::Primary);
        assert!(!first_result.changed);
        assert!(!state.clear_visible_on_screen(KittyGraphicsScreen::Alternate));

        let result = state.apply_on_screen(second, 0, 0, 0, size(), KittyGraphicsScreen::Alternate);

        assert!(result.changed);
        assert_eq!(result.cursor_advance, Some((2, 3)));
        assert_eq!(
            result.cursor_advance_screen,
            Some(KittyGraphicsScreen::Primary)
        );
        let primary =
            state.render_placements_on_screen(10, 0, 24, 80, KittyGraphicsScreen::Primary);
        assert_eq!(primary.len(), 1);
        assert_eq!(primary[0].viewport_row, 5);
        assert!(
            state
                .render_placements_on_screen(0, 0, 24, 80, KittyGraphicsScreen::Alternate,)
                .is_empty()
        );
    }

    #[test]
    fn rejects_truncated_png_after_valid_header() {
        let mut png = one_pixel_png();
        png.truncate(png.len() - 16);
        let mut state = KittyGraphicsState::default();

        let result = state.apply(command("a=T,f=100,i=9,q=1", &png), 0, 0, 0, size());

        assert!(!result.changed);
        assert_eq!(
            result.response.unwrap(),
            b"\x1b_Gi=9;EINVAL:invalid PNG image\x1b\\"
        );
        assert!(state.render_placements(0, 0, 24, 80).is_empty());
    }

    #[test]
    fn delete_by_image_and_placement_id_is_precise() {
        let png = one_pixel_png();
        let mut state = KittyGraphicsState::default();
        state.apply(command("a=t,f=100,i=3,q=1", &png), 0, 0, 0, size());
        state.apply(command("a=p,i=3,p=1,q=1", &[]), 0, 0, 0, size());
        state.apply(command("a=p,i=3,p=2,q=1", &[]), 2, 0, 0, size());
        state.apply(command("a=d,d=i,i=3,p=1,q=1", &[]), 0, 0, 0, size());
        let placements = state.render_placements(0, 0, 24, 80);
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].placement_id, 2);
    }

    #[test]
    fn quiet_mode_suppresses_success_but_not_errors_at_level_one() {
        let mut state = KittyGraphicsState::default();
        let success = state.apply(
            command("a=T,f=32,s=1,v=1,i=1,q=1", &[0, 0, 0, 0]),
            0,
            0,
            0,
            size(),
        );
        assert!(success.response.is_none());
        let failure = state.apply(command("a=p,i=999,q=1", &[]), 0, 0, 0, size());
        assert!(failure.response.is_some());
    }

    #[test]
    fn accepts_zlib_compressed_raw_pixels() {
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&[12, 34, 56, 255]).unwrap();
        let compressed = encoder.finish().unwrap();
        let mut state = KittyGraphicsState::default();

        let result = state.apply(
            command("a=T,f=32,s=1,v=1,o=z,i=12,q=1", &compressed),
            0,
            0,
            0,
            size(),
        );

        assert!(result.changed);
        assert_eq!(state.render_placements(0, 0, 24, 80).len(), 1);
    }

    #[test]
    fn reads_and_removes_safe_temporary_file_transfers() {
        let png = one_pixel_png();
        let mut file = tempfile::Builder::new()
            .prefix("tty-graphics-protocol-")
            .tempfile()
            .unwrap();
        file.write_all(&png).unwrap();
        file.flush().unwrap();
        let path = file.path().to_path_buf();
        let mut state = KittyGraphicsState::default();

        let result = state.apply(
            command("a=T,f=100,t=t,i=13,q=1", path.to_string_lossy().as_bytes()),
            0,
            0,
            0,
            size(),
        );

        assert!(result.changed);
        assert!(!path.exists());
        assert_eq!(state.render_placements(0, 0, 24, 80).len(), 1);
    }
}
