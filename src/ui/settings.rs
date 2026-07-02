use gpui::{Context, IntoElement, ParentElement as _, Styled as _, div, px};
use gpui_component::{
    ActiveTheme, Selectable as _, Sizable as _, button::Button, h_flex,
    scroll::ScrollableElement as _, switch::Switch, v_flex,
};

use crate::db::CloseBehavior;

use super::{
    CACHE_FLUSH_PRESETS, Dashboard, INTERVAL_PRESETS, close_behavior_index, format_interval,
};

impl Dashboard {
    pub(super) fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let interval_buttons = INTERVAL_PRESETS
            .iter()
            .map(|value| {
                Button::new(("interval", *value))
                    .label(format_interval(*value))
                    .selected(self.data.interval_ms == *value)
                    .on_click(cx.listener({
                        let value = *value;
                        move |view, _, _, cx| view.set_interval(value, cx)
                    }))
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let cache_flush_buttons = CACHE_FLUSH_PRESETS
            .iter()
            .map(|value| {
                Button::new(("cache-flush", *value))
                    .label(format_interval(*value))
                    .selected(self.cache_flush_interval_value_ms == *value)
                    .on_click(cx.listener({
                        let value = *value;
                        move |view, _, _, cx| view.set_cache_flush_interval(value, cx)
                    }))
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let close_buttons = [
            (CloseBehavior::Minimize, "最小化"),
            (CloseBehavior::HideToTray, "隐藏到托盘"),
            (CloseBehavior::Exit, "退出程序"),
        ]
        .into_iter()
        .map(|(behavior, label)| {
            Button::new(("close-behavior", close_behavior_index(behavior)))
                .label(label)
                .compact()
                .small()
                .rounded(px(6.))
                .selected(self.close_behavior == behavior)
                .on_click(cx.listener(move |view, _, _, cx| view.set_close_behavior(behavior, cx)))
                .into_any_element()
        })
        .collect::<Vec<_>>();

        div().size_full().overflow_y_scrollbar().child(
            v_flex()
                .w_full()
                .gap_4()
                .p_1()
                .child(
                    div()
                        .rounded_xl()
                        .border_1()
                        .border_color(cx.theme().border.opacity(0.55))
                        .bg(cx.theme().background)
                        .p_5()
                        .child(
                            h_flex()
                                .w_full()
                                .items_start()
                                .gap_6()
                                .flex_wrap()
                                .child(
                                    v_flex()
                                        .min_w(px(360.))
                                        .flex_1()
                                        .gap_3()
                                        .child(
                                            div()
                                                .text_lg()
                                                .font_weight(gpui::FontWeight::BOLD)
                                                .child("采样间隔"),
                                        )
                                        .child(
                                            h_flex().gap_2().flex_wrap().children(interval_buttons),
                                        ),
                                )
                                .child(
                                    v_flex()
                                        .min_w(px(360.))
                                        .flex_1()
                                        .gap_3()
                                        .child(
                                            div()
                                                .text_lg()
                                                .font_weight(gpui::FontWeight::BOLD)
                                                .child("缓存写入周期"),
                                        )
                                        .child(
                                            h_flex()
                                                .gap_2()
                                                .flex_wrap()
                                                .children(cache_flush_buttons),
                                        ),
                                ),
                        ),
                )
                .child(
                    div()
                        .rounded_xl()
                        .border_1()
                        .border_color(cx.theme().border.opacity(0.55))
                        .bg(cx.theme().background)
                        .p_5()
                        .child(
                            v_flex()
                                .gap_4()
                                .child(
                                    div()
                                        .text_lg()
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .child("启动与窗口"),
                                )
                                .child(
                                    h_flex()
                                        .justify_between()
                                        .items_center()
                                        .gap_4()
                                        .child(div().text_sm().child("开机自启"))
                                        .child(
                                            Switch::new("autostart")
                                                .checked(self.autostart_enabled)
                                                .on_click(cx.listener(|view, checked, _, cx| {
                                                    view.set_autostart(*checked, cx)
                                                })),
                                        ),
                                )
                                .child(
                                    h_flex()
                                        .justify_between()
                                        .items_center()
                                        .gap_4()
                                        .child(div().text_sm().child("静默启动"))
                                        .child(
                                            Switch::new("silent-start")
                                                .checked(self.silent_start)
                                                .on_click(cx.listener(|view, checked, _, cx| {
                                                    view.set_silent_start(*checked, cx)
                                                })),
                                        ),
                                )
                                .child(
                                    h_flex()
                                        .justify_between()
                                        .items_center()
                                        .gap_4()
                                        .child(div().text_sm().child("关闭按钮"))
                                        .child(h_flex().gap_2().children(close_buttons)),
                                ),
                        ),
                )
                .child(
                    div()
                        .rounded_xl()
                        .border_1()
                        .border_color(cx.theme().border.opacity(0.55))
                        .bg(cx.theme().background)
                        .p_5()
                        .child(
                            v_flex()
                                .gap_3()
                                .child(
                                    div()
                                        .text_lg()
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .child("数据文件"),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(self.database.path().display().to_string()),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(self.database.icons_path().display().to_string()),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(self.todo_database_path.display().to_string()),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(self.database.archive_dir().display().to_string()),
                                ),
                        ),
                ),
        )
    }

    pub(super) fn render_metric(
        &self,
        label: &'static str,
        value: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex_1()
            .rounded_xl()
            .bg(cx.theme().muted.opacity(0.45))
            .p_4()
            .child(
                v_flex().gap_2().child(div().text_sm().child(label)).child(
                    div()
                        .text_2xl()
                        .font_weight(gpui::FontWeight::BOLD)
                        .child(value),
                ),
            )
    }
}
