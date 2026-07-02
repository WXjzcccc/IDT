use std::time::Duration;

use gpui::{
    AnyElement, Context, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    Styled as _, Window, WindowControlArea, div, hsla, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme, Icon, IconName, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    scroll::ScrollableElement as _,
    v_flex,
};
use smol::Timer;

use crate::{
    todo_db::{TodoDatabase, TodoItem, TodoWindowSettings, TodoWindowTheme},
    window_util,
};

use super::{color_from_hex, format_date_ms, format_duration};

struct TodoWindowPalette {
    background: Hsla,
    muted: Hsla,
    foreground: Hsla,
    muted_foreground: Hsla,
    border: Hsla,
}

pub(super) struct TodoWindow {
    database: TodoDatabase,
    items: Vec<TodoItem>,
    settings: TodoWindowSettings,
    hwnd: Option<isize>,
    topmost: bool,
    desktop_attached: bool,
    status: String,
}

impl TodoWindow {
    pub(super) fn new(
        database: TodoDatabase,
        settings: TodoWindowSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let hwnd = window_util::hwnd_from_window(window);
        if let Some(hwnd) = hwnd {
            window_util::set_window_opacity(hwnd, settings.opacity_percent);
        }
        let mut view = Self {
            database,
            items: Vec::new(),
            settings,
            hwnd,
            topmost: false,
            desktop_attached: false,
            status: String::new(),
        };
        view.reload(cx);
        view.spawn_refresh(cx);
        view
    }

    fn spawn_refresh(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |view, cx| {
            loop {
                Timer::after(Duration::from_millis(1_000)).await;
                let Some(view) = view.upgrade() else {
                    break;
                };
                let should_continue = view
                    .update(cx, |view, cx| {
                        view.reload(cx);
                        true
                    })
                    .unwrap_or(false);
                if !should_continue {
                    break;
                }
            }
        })
        .detach();
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        match self.database.load_open_items() {
            Ok(items) => {
                self.items = items;
                self.status.clear();
            }
            Err(error) => {
                self.items.clear();
                self.status = format!("读取失败: {error}");
            }
        }
        cx.notify();
    }

    fn set_item_completed(&mut self, id: i64, completed: bool, cx: &mut Context<Self>) {
        match self.database.set_item_completed(id, completed) {
            Ok(()) => self.reload(cx),
            Err(error) => {
                self.status = format!("保存失败: {error}");
                cx.notify();
            }
        }
    }

    fn set_topmost(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.topmost = enabled;
        if enabled {
            self.desktop_attached = false;
        }
        if let Some(hwnd) = self.hwnd {
            if enabled {
                window_util::detach_from_desktop(hwnd);
            }
            window_util::set_topmost(hwnd, enabled);
        }
        cx.notify();
    }

    fn set_desktop_attached(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.desktop_attached = enabled;
        if let Some(hwnd) = self.hwnd {
            if enabled {
                self.topmost = false;
                window_util::set_topmost(hwnd, false);
                window_util::attach_to_desktop(hwnd);
            } else {
                window_util::detach_from_desktop(hwnd);
                window_util::set_topmost(hwnd, self.topmost);
            }
        }
        cx.notify();
    }

    fn toggle_theme(&mut self, cx: &mut Context<Self>) {
        self.settings.theme = self.settings.theme.toggled();
        self.persist_settings();
        cx.notify();
    }

    fn adjust_opacity(&mut self, delta: i16, cx: &mut Context<Self>) {
        let next = (self.settings.opacity_percent as i16 + delta).clamp(40, 100) as u8;
        self.settings.opacity_percent = next;
        if let Some(hwnd) = self.hwnd {
            window_util::set_window_opacity(hwnd, next);
        }
        self.persist_settings();
        cx.notify();
    }

    fn close_window(&mut self, window: &mut Window) {
        if let Some(hwnd) = self.hwnd {
            window_util::detach_from_desktop(hwnd);
        }
        window.remove_window();
    }

    fn persist_settings(&mut self) {
        if let Err(error) = self.database.set_todo_window_settings(&self.settings) {
            self.status = format!("保存失败: {error}");
        }
    }

    fn render_window_header(
        &self,
        palette: &TodoWindowPalette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .h(px(48.))
            .flex_none()
            .items_center()
            .justify_between()
            .gap_2()
            .px_3()
            .window_control_area(WindowControlArea::Drag)
            .border_b_1()
            .border_color(palette.border.opacity(0.48))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(Icon::new(IconName::CircleCheck).small())
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::BOLD)
                            .child("I Did Today"),
                    ),
            )
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("todo-window-theme")
                            .small()
                            .compact()
                            .rounded(px(7.))
                            .icon(if self.settings.theme == TodoWindowTheme::Dark {
                                IconName::Sun
                            } else {
                                IconName::Moon
                            })
                            .tooltip("主题")
                            .on_click(cx.listener(|view, _, _, cx| view.toggle_theme(cx))),
                    )
                    .child(
                        Button::new("todo-window-opacity-down")
                            .small()
                            .compact()
                            .rounded(px(7.))
                            .icon(IconName::Minus)
                            .tooltip("降低透明度")
                            .on_click(cx.listener(|view, _, _, cx| view.adjust_opacity(-8, cx))),
                    )
                    .child(
                        div()
                            .min_w(px(34.))
                            .text_center()
                            .text_xs()
                            .text_color(palette.muted_foreground)
                            .child(format!("{}%", self.settings.opacity_percent)),
                    )
                    .child(
                        Button::new("todo-window-opacity-up")
                            .small()
                            .compact()
                            .rounded(px(7.))
                            .icon(IconName::Plus)
                            .tooltip("提高透明度")
                            .on_click(cx.listener(|view, _, _, cx| view.adjust_opacity(8, cx))),
                    )
                    .child(
                        Button::new("todo-window-topmost")
                            .small()
                            .compact()
                            .rounded(px(7.))
                            .selected(self.topmost)
                            .icon(IconName::PanelBottomOpen)
                            .tooltip("置顶")
                            .on_click(
                                cx.listener(|view, _, _, cx| view.set_topmost(!view.topmost, cx)),
                            ),
                    )
                    .child(
                        Button::new("todo-window-desktop")
                            .small()
                            .compact()
                            .rounded(px(7.))
                            .selected(self.desktop_attached)
                            .icon(IconName::PanelBottom)
                            .tooltip("附加至桌面")
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.set_desktop_attached(!view.desktop_attached, cx)
                            })),
                    )
                    .child(
                        Button::new("todo-window-close")
                            .ghost()
                            .small()
                            .compact()
                            .rounded(px(7.))
                            .icon(IconName::Close)
                            .tooltip("关闭")
                            .on_click(cx.listener(|view, _, window, _| view.close_window(window))),
                    ),
            )
    }

    fn render_item(&self, item: &TodoItem, cx: &mut Context<Self>) -> AnyElement {
        let id = item.id;
        let palette = todo_window_palette(self.settings.theme, cx);
        let (done, total) = item.subtask_counts();
        let due = item.due_at_ms.map(format_date_ms);

        h_flex()
            .w_full()
            .items_center()
            .gap_3()
            .rounded(px(8.))
            .border_1()
            .border_color(palette.border.opacity(0.52))
            .bg(palette.background)
            .p_3()
            .child(
                Button::new(("window-complete", item.id as u64))
                    .ghost()
                    .small()
                    .compact()
                    .rounded(px(7.))
                    .icon(IconName::CircleCheck)
                    .tooltip("完成")
                    .on_click(
                        cx.listener(move |view, _, _, cx| view.set_item_completed(id, true, cx)),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(0.))
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::BOLD)
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(item.title.clone()),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .text_xs()
                            .text_color(palette.muted_foreground)
                            .child(format_duration(item.started_at_ms, TodoDatabase::now_ms()))
                            .when(total > 0, |this| this.child(format!("{done}/{total}")))
                            .when_some(due, |this, due| this.child(due)),
                    )
                    .when(!item.tags.is_empty(), |this| {
                        this.child(h_flex().gap_1().flex_wrap().children(
                            item.tags.iter().take(4).map(|tag| {
                                let color = color_from_hex(&tag.color);
                                div()
                                    .rounded(px(5.))
                                    .bg(color.opacity(0.12))
                                    .text_color(color)
                                    .text_xs()
                                    .px_1()
                                    .child(tag.name.clone())
                            }),
                        ))
                    })
                    .when(!item.subtasks.is_empty(), |this| {
                        this.child(v_flex().gap_1().children(item.subtasks.iter().take(3).map(
                            |subtask| {
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .text_xs()
                                    .text_color(palette.muted_foreground)
                                    .child(
                                        Icon::new(if subtask.completed {
                                            IconName::Check
                                        } else {
                                            IconName::Dash
                                        })
                                        .xsmall(),
                                    )
                                    .child(subtask.title.clone())
                            },
                        )))
                    }),
            )
            .into_any_element()
    }
}

impl Render for TodoWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let items = self
            .items
            .iter()
            .map(|item| self.render_item(item, cx))
            .collect::<Vec<_>>();
        let palette = todo_window_palette(self.settings.theme, cx);

        div()
            .size_full()
            .bg(palette.muted)
            .text_color(palette.foreground)
            .font_family("Microsoft YaHei")
            .child(
                v_flex()
                    .size_full()
                    .rounded_xl()
                    .border_1()
                    .border_color(palette.border.opacity(0.62))
                    .bg(palette.background)
                    .overflow_hidden()
                    .child(self.render_window_header(&palette, cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_h(px(0.))
                            .overflow_y_scrollbar()
                            .gap_2()
                            .p_3()
                            .when(items.is_empty(), |this| {
                                this.child(
                                    div()
                                        .h_full()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_color(palette.muted_foreground)
                                        .child(if self.status.is_empty() {
                                            "暂无未完成待办".to_owned()
                                        } else {
                                            self.status.clone()
                                        }),
                                )
                            })
                            .children(items),
                    ),
            )
    }
}

fn todo_window_palette<T>(theme: TodoWindowTheme, cx: &mut Context<T>) -> TodoWindowPalette {
    match theme {
        TodoWindowTheme::Light => TodoWindowPalette {
            background: cx.theme().background,
            muted: cx.theme().muted,
            foreground: cx.theme().foreground,
            muted_foreground: cx.theme().muted_foreground,
            border: cx.theme().border,
        },
        TodoWindowTheme::Dark => TodoWindowPalette {
            background: hsla(220.0 / 360.0, 0.18, 0.11, 1.0),
            muted: hsla(220.0 / 360.0, 0.18, 0.075, 1.0),
            foreground: hsla(210.0 / 360.0, 0.25, 0.92, 1.0),
            muted_foreground: hsla(215.0 / 360.0, 0.12, 0.68, 1.0),
            border: hsla(218.0 / 360.0, 0.16, 0.28, 1.0),
        },
    }
}
