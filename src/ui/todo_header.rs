use gpui::{Context, IntoElement, ParentElement as _, Styled as _, div, px};
use gpui_component::{
    Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
};

use crate::todo_ui::{PICTURE_IN_PICTURE_ICON_PATH, TAGS_ICON_PATH};

use super::Dashboard;

impl Dashboard {
    pub(super) fn render_todo_header_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let month_label = self.todo_panel.read(cx).month_label();

        h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .gap_3()
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("todo-header-prev-month")
                            .ghost()
                            .small()
                            .compact()
                            .rounded(px(7.))
                            .icon(IconName::ChevronLeft)
                            .tooltip("上个月")
                            .on_click({
                                let panel = self.todo_panel.clone();
                                move |_, _, cx| {
                                    panel.update(cx, |panel, cx| panel.previous_month(cx));
                                }
                            }),
                    )
                    .child(
                        div()
                            .min_w(px(112.))
                            .text_center()
                            .text_lg()
                            .font_weight(gpui::FontWeight::BOLD)
                            .child(month_label),
                    )
                    .child(
                        Button::new("todo-header-next-month")
                            .ghost()
                            .small()
                            .compact()
                            .rounded(px(7.))
                            .icon(IconName::ChevronRight)
                            .tooltip("下个月")
                            .on_click({
                                let panel = self.todo_panel.clone();
                                move |_, _, cx| {
                                    panel.update(cx, |panel, cx| panel.next_month(cx));
                                }
                            }),
                    )
                    .child(
                        Button::new("todo-header-this-month")
                            .small()
                            .compact()
                            .rounded(px(7.))
                            .label("今天")
                            .on_click({
                                let panel = self.todo_panel.clone();
                                move |_, _, cx| {
                                    panel.update(cx, |panel, cx| panel.current_month(cx));
                                }
                            }),
                    ),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("todo-header-tags")
                            .small()
                            .compact()
                            .rounded(px(7.))
                            .icon(Icon::empty().path(TAGS_ICON_PATH))
                            .tooltip("标签")
                            .on_click({
                                let panel = self.todo_panel.clone();
                                move |_, _, cx| {
                                    panel
                                        .update(cx, |panel, cx| panel.toggle_tag_manager(true, cx));
                                }
                            }),
                    )
                    .child(
                        Button::new("todo-header-open-window")
                            .small()
                            .compact()
                            .rounded(px(7.))
                            .icon(Icon::empty().path(PICTURE_IN_PICTURE_ICON_PATH))
                            .tooltip("独立窗口")
                            .on_click({
                                let panel = self.todo_panel.clone();
                                move |_, _, cx| {
                                    panel.update(cx, |panel, cx| panel.open_standalone(cx));
                                }
                            }),
                    )
                    .child(
                        Button::new("todo-header-new")
                            .small()
                            .compact()
                            .rounded(px(7.))
                            .icon(IconName::Plus)
                            .label("新建")
                            .on_click({
                                let panel = self.todo_panel.clone();
                                move |_, window, cx| {
                                    panel.update(cx, |panel, cx| panel.start_new(window, cx));
                                }
                            }),
                    ),
            )
    }
}
