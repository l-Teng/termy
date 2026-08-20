//! The control library: everything that lives in a setting row's control lane.
//!
//! Controls are stateless [`RenderOnce`](gpui::RenderOnce) components. The host
//! owns the value and passes it in, which keeps the kit usable from a GPUI view,
//! a preview, or a test without dragging app state along.

mod button;
mod segmented;
mod shortcut;
mod slider;
mod stepper;
mod theme_switch;

pub use button::IconButton;
pub use segmented::SegmentedControl;
pub use shortcut::{KeyChip, Platform, ShortcutBox, ShortcutState, format_binding};
pub use slider::Slider;
pub use stepper::Stepper;
pub use theme_switch::ThemeSwitch;

use gpui::{App, ClickEvent, Window};

/// Click callback shared by the interactive controls.
pub type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;
