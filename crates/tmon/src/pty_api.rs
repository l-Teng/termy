use super::{Engine, Size, Terminal};
use std::io;

impl Terminal {
    pub fn write(&self, input: &[u8]) {
        let _ = self.try_write(input);
    }

    /// Try to enqueue user input for the child PTY.
    ///
    /// Display-only terminals accept the write as a no-op. PTY-backed
    /// terminals report bounded-backlog and disconnected-writer failures.
    pub fn try_write(&self, input: &[u8]) -> io::Result<()> {
        #[cfg(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "windows"
        ))]
        if let Some(pty) = &self.pty {
            return pty.write(input);
        }
        let _ = input;
        Ok(())
    }

    pub fn write_owned(&self, input: Vec<u8>) {
        let _ = self.try_write_owned(input);
    }

    /// Try to enqueue owned user input without copying it into the PTY queue.
    pub fn try_write_owned(&self, input: Vec<u8>) -> io::Result<()> {
        #[cfg(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "windows"
        ))]
        if let Some(pty) = &self.pty {
            return pty.write_owned(input);
        }
        let _ = input;
        Ok(())
    }

    pub fn resize(&self, size: Size) {
        let _ = self.try_resize(size);
    }

    /// Resize both the child PTY and terminal grid as one observable operation.
    ///
    /// For PTY-backed terminals, the OS resize is acknowledged before the
    /// in-memory grid changes. A rejected or failed PTY resize therefore leaves
    /// the previous grid size intact instead of silently diverging.
    pub fn try_resize(&self, size: Size) -> io::Result<()> {
        let size = size.clamped();
        let mut engine = self
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if size == engine.size {
            return Ok(());
        }
        #[cfg(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "windows"
        ))]
        if let Some(pty) = &self.pty {
            pty.resize(size)?;
        }
        let Engine {
            parser,
            grid,
            size: engine_size,
            render_generation,
        } = &mut *engine;
        let graphics_can_change = parser.has_graphics_placements();
        grid.resize(size.cols, size.rows);
        *engine_size = size;
        parser.set_size(size);
        grid.set_cell_metrics(size.cell_width, size.cell_height);
        let graphics_changed = parser.sync_grid_effects(grid);
        if graphics_can_change && !graphics_changed {
            parser.bump_graphics_revision();
        }
        *render_generation = render_generation.wrapping_add(1);
        Ok(())
    }

    pub fn nudge_resize(&self) {
        let _ = self.try_nudge_resize();
    }

    /// Re-send the current dimensions and report whether the PTY accepted them.
    pub fn try_nudge_resize(&self) -> io::Result<()> {
        #[cfg(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "windows"
        ))]
        let engine = self
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        #[cfg(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "windows"
        ))]
        if let Some(pty) = &self.pty {
            pty.resize(engine.size)?;
        }
        Ok(())
    }
}
