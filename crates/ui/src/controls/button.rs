use gpui::{
    App, ClickEvent, ElementId, InteractiveElement, IntoElement, ParentElement, Pixels, RenderOnce,
    Rgba, StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::controls::ClickHandler;
use crate::icon::{Icon, IconName};
use crate::metrics::{RESET_ICON_SIZE, RESET_SLOT_SIZE};
use crate::theme::tokens;

/// The bare glyph button used for a row's reset affordance. It occupies the
/// row's reserved lane so rows without one still line up.
#[derive(IntoElement)]
pub struct IconButton {
    id: ElementId,
    icon: IconName,
    color: Option<Rgba>,
    size: Pixels,
    on_click: Option<ClickHandler>,
}

impl IconButton {
    pub fn new(id: impl Into<ElementId>, icon: IconName) -> Self {
        Self {
            id: id.into(),
            icon,
            color: None,
            size: RESET_ICON_SIZE,
            on_click: None,
        }
    }

    pub fn color(mut self, color: Rgba) -> Self {
        self.color = Some(color);
        self
    }

    pub fn size(mut self, size: Pixels) -> Self {
        self.size = size;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for IconButton {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = tokens(cx);
        let color = self.color.unwrap_or(theme.accent);

        let mut button = div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(RESET_SLOT_SIZE)
            .rounded(px(4.0))
            .cursor_pointer()
            .hover(move |style| style.bg(theme.bg_hover));

        if let Some(handler) = self.on_click {
            button = button.on_click(move |event, window, cx| handler(event, window, cx));
        }

        button.child(Icon::new(self.icon).size(self.size).color(color))
    }
}
