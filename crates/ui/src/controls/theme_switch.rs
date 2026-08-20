use std::rc::Rc;

use gpui::{
    App, ClickEvent, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _, px,
};

use crate::theme::tokens;

type ChangeHandler = Rc<dyn Fn(bool, &mut Window, &mut App) + 'static>;

/// Compact switch whose active track follows Termy's current terminal accent.
///
/// Glassy 0.1.1 does not expose its switch-track color, so Settings uses this
/// theme-bound control until that color becomes part of Glassy's public theme.
#[derive(IntoElement)]
pub struct ThemeSwitch {
    id: ElementId,
    on: bool,
    disabled: bool,
    on_change: Option<ChangeHandler>,
}

impl ThemeSwitch {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            on: false,
            disabled: false,
            on_change: None,
        }
    }

    pub fn on(mut self, on: bool) -> Self {
        self.on = on;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_change(mut self, listener: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(listener));
        self
    }
}

impl RenderOnce for ThemeSwitch {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = tokens(cx);
        let interactive = !self.disabled;
        let on = self.on;
        let track = if on { theme.accent } else { theme.bg_input };
        let border = if on { theme.accent } else { theme.border };
        let thumb = if on {
            theme.text_on_accent
        } else {
            theme.text_primary
        };

        let mut element = div()
            .id(self.id)
            .w(px(36.0))
            .h(px(20.0))
            .p(px(1.0))
            .flex()
            .items_center()
            .when(on, |element| element.justify_end())
            .when(!on, |element| element.justify_start())
            .rounded_full()
            .border_1()
            .border_color(border)
            .bg(track)
            .when(self.disabled, |element| element.opacity(0.5))
            .child(div().size(px(16.0)).rounded_full().bg(thumb).shadow_sm());

        if interactive {
            element = element.cursor_pointer();
            if let Some(on_change) = self.on_change {
                element = element.on_click(move |_: &ClickEvent, window, cx| {
                    on_change(!on, window, cx);
                });
            }
        }

        element
    }
}
