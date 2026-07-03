use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AnyElement, Context, InteractiveElement as _, IntoElement,
    MouseButton, ParentElement as _, Styled as _, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme, Colorize as _, Icon, IconName, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    color_picker::ColorPicker,
    h_flex,
    input::Input,
    scroll::ScrollableElement as _,
    v_flex,
};

use crate::ui_controls::red_icon_button_variant;

use super::{TAGS_ICON_PATH, TodoEditor, TodoPanel, color_from_hex, form_label};

const TAG_COLORS: [&str; 8] = [
    "#2563eb", "#059669", "#d97706", "#dc2626", "#7c3aed", "#0891b2", "#be185d", "#4f46e5",
];
impl TodoPanel {
    pub(crate) fn toggle_tag_manager(&mut self, open: bool, cx: &mut Context<Self>) {
        self.tag_manager_open = open;
        cx.notify();
    }

    fn set_tag_color(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.tag_color_index = ix.min(TAG_COLORS.len().saturating_sub(1));
        let color = color_from_hex(tag_color_at(self.tag_color_index));
        self.tag_color_picker
            .update(cx, |picker, cx| picker.set_value(color, window, cx));
        cx.notify();
    }

    fn save_tag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.tag_name_input.read(cx).value().to_string();
        let color = self
            .tag_color_picker
            .read(cx)
            .value()
            .map(|color| color.to_hex())
            .unwrap_or_else(|| tag_color_at(self.tag_color_index).to_owned());
        let result = if let Some(tag_id) = self.editing_tag_id {
            self.database
                .update_tag(tag_id, &name, &color)
                .map(|()| tag_id)
        } else {
            self.database.save_tag(&name, &color)
        };

        match result {
            Ok(_) => {
                self.editing_tag_id = None;
                self.tag_name_input
                    .update(cx, |input, cx| input.set_value("", window, cx));
                self.reload(cx);
            }
            Err(error) => {
                self.status = format!("保存失败: {error}");
                cx.notify();
            }
        }
    }

    fn edit_tag(
        &mut self,
        tag_id: i64,
        name: String,
        color: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editing_tag_id = Some(tag_id);
        self.tag_name_input
            .update(cx, |input, cx| input.set_value(name, window, cx));
        self.tag_color_picker.update(cx, |picker, cx| {
            picker.set_value(color_from_hex(&color), window, cx)
        });
        if let Some(ix) = TAG_COLORS.iter().position(|preset| *preset == color) {
            self.tag_color_index = ix;
        }
        cx.notify();
    }

    fn delete_tag(&mut self, tag_id: i64, cx: &mut Context<Self>) {
        match self.database.delete_tag(tag_id) {
            Ok(()) => {
                if self.editing_tag_id == Some(tag_id) {
                    self.editing_tag_id = None;
                }
                if let Some(editor) = self.editor.as_mut() {
                    editor.selected_tag_ids.retain(|id| *id != tag_id);
                }
                self.reload(cx);
            }
            Err(error) => {
                self.status = format!("删除失败: {error}");
                cx.notify();
            }
        }
    }

    fn toggle_editor_tag(&mut self, tag_id: i64, cx: &mut Context<Self>) {
        if let Some(editor) = self.editor.as_mut() {
            if let Some(index) = editor.selected_tag_ids.iter().position(|id| *id == tag_id) {
                editor.selected_tag_ids.remove(index);
            } else {
                editor.selected_tag_ids.push(tag_id);
            }
        }
        cx.notify();
    }

    pub(super) fn render_editor_tags(
        &self,
        editor: &TodoEditor,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let tags = self
            .tags
            .iter()
            .map(|tag| {
                let selected = editor.selected_tag_ids.iter().any(|id| *id == tag.id);
                let tag_id = tag.id;
                let color = color_from_hex(&tag.color);
                Button::new(("editor-tag", tag.id as u64))
                    .small()
                    .compact()
                    .rounded(px(7.))
                    .selected(selected)
                    .label(tag.name.clone())
                    .on_click(cx.listener(move |view, _, _, cx| view.toggle_editor_tag(tag_id, cx)))
                    .child(
                        div()
                            .size_2()
                            .rounded_full()
                            .bg(color)
                            .border_1()
                            .border_color(color.opacity(0.55)),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>();

        v_flex().gap_2().child(form_label("标签", cx)).child(
            h_flex()
                .gap_2()
                .flex_wrap()
                .children(tags)
                .when(self.tags.is_empty(), |this| {
                    this.child(
                        Button::new("editor-open-tags")
                            .small()
                            .compact()
                            .rounded(px(7.))
                            .icon(Icon::empty().path(TAGS_ICON_PATH))
                            .label("新建标签")
                            .on_click(
                                cx.listener(|view, _, _, cx| view.toggle_tag_manager(true, cx)),
                            ),
                    )
                }),
        )
    }

    pub(super) fn render_tag_manager_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.tag_manager_open {
            return None;
        }

        let tag_rows = self
            .tags
            .iter()
            .map(|tag| {
                let color = color_from_hex(&tag.color);
                let tag_id = tag.id;
                let tag_name = tag.name.clone();
                let tag_color = tag.color.clone();
                h_flex()
                    .items_center()
                    .gap_2()
                    .rounded(px(7.))
                    .border_1()
                    .border_color(cx.theme().border.opacity(0.5))
                    .px_3()
                    .py_2()
                    .child(div().size_3().rounded_full().bg(color))
                    .child(
                        div()
                            .text_sm()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .font_weight(gpui::FontWeight::BOLD)
                            .child(tag.name.clone()),
                    )
                    .child(
                        Button::new(("tag-edit", tag.id as u64))
                            .ghost()
                            .small()
                            .compact()
                            .rounded(px(7.))
                            .icon(IconName::Settings2)
                            .tooltip("修改")
                            .on_click(cx.listener(move |view, _, window, cx| {
                                view.edit_tag(
                                    tag_id,
                                    tag_name.clone(),
                                    tag_color.clone(),
                                    window,
                                    cx,
                                )
                            })),
                    )
                    .child(
                        Button::new(("tag-delete", tag.id as u64))
                            .ghost()
                            .small()
                            .compact()
                            .rounded(px(7.))
                            .icon(IconName::Delete)
                            .tooltip("删除")
                            .on_click(
                                cx.listener(move |view, _, _, cx| view.delete_tag(tag_id, cx)),
                            ),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let swatches = TAG_COLORS
            .iter()
            .enumerate()
            .map(|(ix, color)| self.render_tag_color_button(ix, color, cx))
            .collect::<Vec<_>>();
        let picker_colors = TAG_COLORS
            .iter()
            .map(|color| color_from_hex(color))
            .collect::<Vec<_>>();

        Some(
            div()
                .absolute()
                .inset_0()
                .bg(cx.theme().background.opacity(0.72))
                .flex()
                .items_center()
                .justify_center()
                .p_4()
                .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
                .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_up(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                .on_mouse_up(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
                .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                .child(
                    v_flex()
                        .w(px(620.))
                        .h(px(560.))
                        .max_w_full()
                        .max_h_full()
                        .rounded(px(10.))
                        .border_1()
                        .border_color(cx.theme().border.opacity(0.62))
                        .bg(cx.theme().background)
                        .shadow_lg()
                        .overflow_hidden()
                        .child(
                            h_flex()
                                .h(px(50.))
                                .flex_none()
                                .items_center()
                                .justify_between()
                                .px_4()
                                .border_b_1()
                                .border_color(cx.theme().border.opacity(0.45))
                                .child(
                                    div()
                                        .text_lg()
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .child("标签"),
                                )
                                .child(
                                    Button::new("tag-manager-close")
                                        .custom(red_icon_button_variant(cx))
                                        .compact()
                                        .small()
                                        .rounded(px(7.))
                                        .icon(IconName::Close)
                                        .tooltip("关闭")
                                        .on_click(cx.listener(|view, _, _, cx| {
                                            view.toggle_tag_manager(false, cx)
                                        })),
                                ),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_h(px(0.))
                                .gap_3()
                                .p_4()
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .child(
                                            div()
                                                .flex_1()
                                                .child(Input::new(&self.tag_name_input).small()),
                                        )
                                        .child(
                                            Button::new("tag-save")
                                                .small()
                                                .rounded(px(7.))
                                                .icon(if self.editing_tag_id.is_some() {
                                                    IconName::Check
                                                } else {
                                                    IconName::Plus
                                                })
                                                .label(if self.editing_tag_id.is_some() {
                                                    "更新"
                                                } else {
                                                    "保存"
                                                })
                                                .on_click(cx.listener(|view, _, window, cx| {
                                                    view.save_tag(window, cx)
                                                })),
                                        ),
                                )
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .items_center()
                                        .flex_wrap()
                                        .children(swatches)
                                        .child(
                                            ColorPicker::new(&self.tag_color_picker)
                                                .small()
                                                .featured_colors(picker_colors),
                                        ),
                                )
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .min_h(px(0.))
                                        .gap_2()
                                        .overflow_y_scrollbar()
                                        .children(tag_rows),
                                ),
                        ),
                )
                .with_animation(
                    "tag-manager-overlay",
                    Animation::new(Duration::from_millis(180)).with_easing(gpui::ease_out_quint()),
                    |this, delta| this.opacity((0.28 + delta * 0.72).min(1.0)),
                )
                .into_any_element(),
        )
    }

    fn render_tag_color_button(
        &self,
        ix: usize,
        color: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        Button::new(("tag-color", ix as u64))
            .small()
            .compact()
            .rounded(px(7.))
            .selected(self.tag_color_index == ix)
            .on_click(cx.listener(move |view, _, window, cx| view.set_tag_color(ix, window, cx)))
            .child(div().size_4().rounded_full().bg(color_from_hex(color)))
            .into_any_element()
    }
}

fn tag_color_at(ix: usize) -> &'static str {
    TAG_COLORS.get(ix).copied().unwrap_or(TAG_COLORS[0])
}
