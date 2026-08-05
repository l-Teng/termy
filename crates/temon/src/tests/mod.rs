use super::*;
use alacritty_terminal::{
    event::{Event as AlacrittyEvent, EventListener, WindowSize},
    term::{Config as AlacrittyConfig, Term as AlacrittyTerm},
    vte::ansi,
};

#[derive(Clone, Default)]
struct ReplyListener(Arc<Mutex<Vec<AlacrittyEvent>>>);

impl EventListener for ReplyListener {
    fn send_event(&self, event: AlacrittyEvent) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(event);
    }
}

fn alacritty_query_replies(bytes: &[u8], size: Size) -> Vec<u8> {
    let dimensions = termy_core::TerminalSize {
        cols: size.cols,
        rows: size.rows,
        cell_width: size.cell_width,
        cell_height: size.cell_height,
    };
    let listener = ReplyListener::default();
    let mut term = AlacrittyTerm::new(
        AlacrittyConfig {
            kitty_keyboard: true,
            ..AlacrittyConfig::default()
        },
        &dimensions,
        listener.clone(),
    );
    let mut parser: ansi::Processor = ansi::Processor::new();
    parser.advance(&mut term, bytes);
    let events = std::mem::take(
        &mut *listener
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    let window_size = WindowSize::from(dimensions);
    let mut replies = Vec::new();
    for event in events {
        match event {
            AlacrittyEvent::PtyWrite(text) => replies.extend_from_slice(text.as_bytes()),
            AlacrittyEvent::TextAreaSizeRequest(formatter) => {
                replies.extend_from_slice(formatter(window_size).as_bytes());
            }
            AlacrittyEvent::ColorRequest(index, formatter) => {
                if let Some(color) = term.colors()[index] {
                    replies.extend_from_slice(formatter(color).as_bytes());
                }
            }
            _ => {}
        }
    }
    replies
}

mod events_runtime;
mod terminal;
