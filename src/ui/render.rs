use std::time::Duration;

use gpui::{
    Animation, AnimationExt, Context, IntoElement, ParentElement as _, Render, Styled as _, Window,
    div, px,
};
use gpui_component::{ActiveTheme, PixelsExt as _, v_flex};

use super::{Dashboard, ShellMode, ViewMode};

impl Render for Dashboard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport_size = window.viewport_size();
        self.window_width = viewport_size.width.as_f32();
        self.window_height = viewport_size.height.as_f32();
        let body = match self.shell_mode {
            ShellMode::Activity => match self.mode {
                ViewMode::Overview => self.render_overview(cx).into_any_element(),
                ViewMode::Timeline => self.render_timeline(cx).into_any_element(),
                ViewMode::Settings => self.render_settings(cx).into_any_element(),
            },
            ShellMode::Todo => self.todo_panel.clone().into_any_element(),
        };
        let shell_key = match self.shell_mode {
            ShellMode::Activity => 0_u32,
            ShellMode::Todo => 1_u32,
        };

        div()
            .size_full()
            .bg(cx.theme().muted)
            .text_color(cx.theme().foreground)
            .font_family("Microsoft YaHei")
            .child(
                v_flex().size_full().child(
                    v_flex()
                        .size_full()
                        .rounded_xl()
                        .border_1()
                        .border_color(cx.theme().border.opacity(0.55))
                        .bg(cx.theme().background)
                        .overflow_hidden()
                        .child(self.render_header(cx))
                        .child(
                            div().flex_1().min_h(px(0.)).overflow_hidden().child(
                                div().size_full().child(body).with_animation(
                                    ("shell-body", shell_key),
                                    Animation::new(Duration::from_millis(260))
                                        .with_easing(gpui::ease_out_quint()),
                                    |this, delta| {
                                        this.opacity((0.62 + delta * 0.38).min(1.0))
                                            .mt(px((1.0 - delta) * 8.0))
                                    },
                                ),
                            ),
                        ),
                ),
            )
    }
}
