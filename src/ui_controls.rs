use gpui::App;
use gpui_component::{ActiveTheme, button::ButtonCustomVariant};

pub(crate) fn red_icon_button_variant(cx: &App) -> ButtonCustomVariant {
    ButtonCustomVariant::new(cx)
        .foreground(cx.theme().red)
        .hover(cx.theme().red.opacity(0.12))
        .active(cx.theme().red.opacity(0.2))
}
