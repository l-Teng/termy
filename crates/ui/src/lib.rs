//! Termy's settings design system, in GPUI.
//!
//! This crate owns the visual language of Termy's chrome — surfaces, controls,
//! rows, and status affordances — as stateless components. It knows nothing
//! about config keys, plugins, or SSH hosts; product surfaces pass values in
//! and get elements back.
//!
//! ```no_run
//! use termy_ui::{SettingRow, SettingsGroup, Switch, theme};
//! use gpui::{App, ParentElement as _};
//!
//! fn appearance_group(_cx: &mut App) -> SettingsGroup {
//!     SettingsGroup::new("THEME").child(
//!         SettingRow::new("Increase Chrome Contrast")
//!             .description("Increase contrast of non-terminal UI surfaces")
//!             .control(Switch::new("chrome-contrast").on(true)),
//!     )
//! }
//! ```
//!
//! Hosts install colors once with [`theme::set_tokens`] and register
//! [`icon::Assets`] (or delegate to [`icon::icon_bytes`] from their own asset
//! source) so the icon set resolves.

pub mod controls;
pub mod feedback;
pub mod icon;
pub mod metrics;
pub mod row;
pub mod section;
pub mod sidebar;
pub mod theme;

pub use controls::{
    IconButton, Platform, SegmentedControl, ShortcutBox, ShortcutState, Slider, Stepper,
    ThemeSwitch, format_binding,
};
pub use feedback::{Banner, EmptyState, Toast};
pub use glassy_ui::{
    ActiveTheme as GlassyActiveTheme, AlertDialog, AnimatePresence, Badge, BadgeVariant, Button,
    ButtonChrome, ButtonGroup, ButtonSize, ButtonVariant, CheckState, Checkbox, CircularProgress,
    ContextMenu, Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
    DropdownMenu, DropdownMenuEntry, DropdownMenuItem, Ease, FieldChrome, FieldState,
    Icon as GlassyIcon, IconName as GlassyIconName, Input, Kbd, Label, Motion, MotionStyle,
    MotionValue, MotionValueStore, Popover, PopoverContent, PopoverDescription, PopoverPlacement,
    PopoverTitle, PresenceMode, Progress, Radio, Select, SelectItem, Separator,
    SeparatorOrientation, Skeleton, SkeletonShape, Spinner, SpinnerSize, SpinnerTone, Stagger,
    StyledSlot, Switch, Theme as GlassyTheme, ThemeKind as GlassyThemeKind, Tooltip,
    TooltipPlacement, Transition, Variants, cubic_bezier, ease_in_cubic, ease_out_cubic,
    load_fonts, paint as glassy_paint, rgb as glassy_rgb, textarea, use_motion_value,
};
pub use icon::{Assets, Icon, IconName, icon_bytes};
pub use row::{RowTone, SettingRow};
pub use section::{Card, SectionHeader, SettingsContent, SettingsGroup};
pub use sidebar::{Sidebar, SidebarGroupLabel, SidebarItem, SidebarSearch};
pub use theme::{Palette, Tokens, Tone, set_tokens, tokens};

/// Register Glassy's motion, theme, and input bindings once at app startup.
pub fn init_glassy(cx: &mut gpui::App) {
    glassy_ui::init_motion(cx);
    glassy_ui::init_theme(cx);
    glassy_ui::init(cx);
}

/// Derive Glassy's semantic palette from Termy's active terminal-theme tokens.
///
/// Glassy still owns its translucent material recipes, while every exposed
/// color role follows the current Termy theme instead of its stock zinc theme.
pub fn sync_glassy_theme(tokens: &Tokens, cx: &mut gpui::App) {
    use glassy_ui::ActiveTheme as _;

    let theme = glassy_theme(tokens);
    if cx.theme() != theme {
        cx.set_theme(theme);
    }
}

fn glassy_theme(tokens: &Tokens) -> glassy_ui::Theme {
    let luminance = (0.2126 * tokens.bg_window.r)
        + (0.7152 * tokens.bg_window.g)
        + (0.0722 * tokens.bg_window.b);
    let mut theme = if luminance >= 0.5 {
        glassy_ui::Theme::light()
    } else {
        glassy_ui::Theme::dark()
    };
    theme.canvas = tokens.bg_window.into();
    theme.heading = tokens.text_primary.into();
    theme.body = tokens.text_secondary.into();
    theme.label = tokens.text_muted.into();
    theme.on_solid = tokens.text_primary.into();
    theme.ink = tokens.text_primary.into();
    theme.placeholder = tokens.text_muted.into();
    theme.destructive = tokens.danger.into();
    theme.destructive_soft = tokens.danger.into();
    theme
}

#[cfg(test)]
mod glassy_theme_tests {
    use super::*;

    #[test]
    fn glassy_semantic_colors_follow_termy_tokens() {
        let tokens = Tokens::dark();
        let theme = glassy_theme(&tokens);

        assert_eq!(theme.kind, glassy_ui::ThemeKind::Dark);
        assert_eq!(theme.canvas, tokens.bg_window.into());
        assert_eq!(theme.ink, tokens.text_primary.into());
        assert_eq!(theme.body, tokens.text_secondary.into());
        assert_eq!(theme.placeholder, tokens.text_muted.into());
        assert_eq!(theme.destructive, tokens.danger.into());
    }
}
