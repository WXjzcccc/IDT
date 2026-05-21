use std::{
    collections::HashMap,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering},
    },
    time::Duration,
};

use chrono::{Datelike, Duration as ChronoDuration, Local, NaiveDate, TimeZone};
use gpui::{
    AnyElement, AppContext as _, Context, Entity, Hsla, Image, ImageFormat,
    InteractiveElement as _, IntoElement, ObjectFit, ParentElement as _, Pixels, Render,
    SharedString, Size as GpuiSize, Styled as _, StyledImage as _, Subscription, Window,
    WindowControlArea, div, img, linear_color_stop, linear_gradient, prelude::FluentBuilder as _,
    px, relative, size,
};
use gpui_component::{
    ActiveTheme, Icon, IconName, PixelsExt as _, Selectable as _, Sizable as _, Theme, ThemeMode,
    VirtualListScrollHandle,
    button::{Button, ButtonVariants as _},
    calendar::Date,
    chart::AreaChart,
    date_picker::{DatePicker, DatePickerEvent, DatePickerState},
    h_flex,
    input::{Input, InputEvent, InputState},
    scroll::ScrollableElement as _,
    switch::Switch,
    v_flex, v_virtual_list,
};
use smol::Timer;

use crate::{
    db::{
        AppTotal, CloseBehavior, DEFAULT_INTERVAL_MS, DashboardData, Database, ThemePreference,
        TimelineItem, WindowSize,
    },
    startup,
    tracker::TrackerHandle,
    tray,
};

const INTERVAL_PRESETS: [u64; 5] = [200, 500, 1_000, 3_000, 5_000];
const TIMELINE_ROW_HEIGHT: f32 = 58.0;
const HOUR_MS: i64 = 60 * 60 * 1_000;
const DAY_MS: i64 = 24 * HOUR_MS;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ViewMode {
    Overview,
    Timeline,
    Settings,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TimeFilter {
    Last24Hours,
    Today,
    ThisWeek,
    ThisMonth,
    Custom,
}

#[derive(Clone, Debug)]
struct TimeRange {
    start_ms: i64,
    end_ms: i64,
    label: String,
}

#[derive(Clone)]
struct AppAreaPoint {
    label: SharedString,
    values: Vec<f64>,
}

#[derive(Clone)]
struct AppGroup {
    process_name: String,
    icon_png: Option<Vec<u8>>,
    duration_ms: u64,
    percent: f32,
    processes: Vec<String>,
    is_other: bool,
}

#[derive(Clone)]
struct ChartBucket {
    start_ms: i64,
    end_ms: i64,
    label: SharedString,
}

pub struct Dashboard {
    database: Database,
    interval_ms: Arc<AtomicU64>,
    exit_requested: Arc<AtomicBool>,
    target_hwnd: Arc<AtomicIsize>,
    _tracker: TrackerHandle,
    data: DashboardData,
    time_filter: TimeFilter,
    time_range: TimeRange,
    custom_range: (NaiveDate, NaiveDate),
    range_picker: Entity<DatePickerState>,
    process_filter_input: Entity<InputState>,
    title_filter_input: Entity<InputState>,
    timeline_indices: Rc<Vec<usize>>,
    timeline_row_sizes: Rc<Vec<GpuiSize<Pixels>>>,
    timeline_scroll: VirtualListScrollHandle,
    window_width: f32,
    window_height: f32,
    last_saved_window_size: Option<WindowSize>,
    theme: ThemePreference,
    autostart_enabled: bool,
    silent_start: bool,
    close_behavior: CloseBehavior,
    mode: ViewMode,
    status: String,
    _subscriptions: Vec<Subscription>,
}

impl Dashboard {
    pub fn new(
        database: Database,
        interval_ms: Arc<AtomicU64>,
        exit_requested: Arc<AtomicBool>,
        target_hwnd: Arc<AtomicIsize>,
        tracker: TrackerHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let today = Local::now().date_naive();
        let time_filter = TimeFilter::Today;
        let custom_range = (today, today);
        let time_range = time_filter.range(custom_range);
        let settings = database.app_settings().unwrap_or_else(|error| {
            eprintln!("failed to load app settings: {error:#}");
            crate::db::AppSettings {
                theme: ThemePreference::Light,
                autostart_enabled: false,
                silent_start: false,
                close_behavior: CloseBehavior::HideToTray,
            }
        });
        let autostart_enabled = startup::is_enabled().unwrap_or(settings.autostart_enabled);
        let last_saved_window_size = database.get_window_size().ok().flatten();

        let range_picker = cx.new(|cx| {
            let mut picker = DatePickerState::range(window, cx).date_format("%Y-%m-%d");
            picker.set_date((today, today), window, cx);
            picker
        });
        let process_filter_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("进程")
                .clean_on_escape()
        });
        let title_filter_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("标题")
                .clean_on_escape()
        });

        let subscriptions = vec![
            cx.observe_window_bounds(window, |view, window, _| {
                view.remember_window_size(window);
            }),
            cx.subscribe_in(&range_picker, window, |view, _, event, window, cx| {
                let DatePickerEvent::Change(date) = event;
                if let Date::Range(Some(start), Some(end)) = date {
                    let (start, end) = ordered_dates(*start, *end);
                    view.time_filter = TimeFilter::Custom;
                    view.custom_range = (start, end);
                    view.sync_range_picker(window, cx);
                    view.refresh(cx);
                }
            }),
            cx.subscribe_in(&process_filter_input, window, |view, _, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    view.rebuild_timeline_filter(cx);
                }
            }),
            cx.subscribe_in(&title_filter_input, window, |view, _, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    view.rebuild_timeline_filter(cx);
                }
            }),
        ];

        let data = database
            .dashboard_range(
                time_range.start_ms,
                time_range.end_ms,
                time_range.label.clone(),
            )
            .unwrap_or_else(|error| {
                eprintln!("failed to load dashboard data: {error:#}");
                empty_dashboard(DEFAULT_INTERVAL_MS)
            });
        let timeline_indices = filtered_timeline_indices(
            &data.timeline,
            process_filter_input.read(cx).value().as_ref(),
            title_filter_input.read(cx).value().as_ref(),
        );

        let dashboard = Self {
            database,
            interval_ms,
            exit_requested,
            target_hwnd,
            _tracker: tracker,
            time_filter,
            time_range,
            custom_range,
            range_picker,
            process_filter_input,
            title_filter_input,
            timeline_row_sizes: timeline_row_sizes(timeline_indices.len()),
            timeline_indices: Rc::new(timeline_indices),
            timeline_scroll: VirtualListScrollHandle::new(),
            window_width: crate::db::DEFAULT_WINDOW_WIDTH as f32,
            window_height: crate::db::DEFAULT_WINDOW_HEIGHT as f32,
            last_saved_window_size,
            theme: settings.theme,
            autostart_enabled,
            silent_start: settings.silent_start,
            close_behavior: settings.close_behavior,
            data,
            mode: ViewMode::Overview,
            status: "运行中".to_owned(),
            _subscriptions: subscriptions,
        };
        dashboard.spawn_refresh(cx);
        dashboard
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
                        if view.exit_requested.load(Ordering::Relaxed) {
                            cx.quit();
                            return false;
                        }
                        view.refresh(cx);
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

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let time_range = self.time_filter.range(self.custom_range);
        match self.database.dashboard_range(
            time_range.start_ms,
            time_range.end_ms,
            time_range.label.clone(),
        ) {
            Ok(data) => {
                self.time_range = time_range;
                self.data = data;
                self.rebuild_timeline_filter(cx);
                self.status = "运行中".to_owned();
            }
            Err(error) => {
                self.status = format!("读取失败: {error}");
            }
        }
        cx.notify();
    }

    fn set_time_filter(&mut self, filter: TimeFilter, window: &mut Window, cx: &mut Context<Self>) {
        self.time_filter = filter;
        self.time_range = self.time_filter.range(self.custom_range);
        self.sync_range_picker(window, cx);
        self.refresh(cx);
    }

    fn sync_range_picker(&self, window: &mut Window, cx: &mut Context<Self>) {
        let (start, end) = self.time_filter.date_range_for_picker(self.custom_range);
        self.range_picker.update(cx, |picker, cx| {
            picker.set_date((start, end), window, cx);
        });
    }

    fn rebuild_timeline_filter(&mut self, cx: &mut Context<Self>) {
        let process_filter = self.process_filter_input.read(cx).value();
        let title_filter = self.title_filter_input.read(cx).value();
        let indices =
            filtered_timeline_indices(&self.data.timeline, &process_filter, &title_filter);
        self.timeline_row_sizes = timeline_row_sizes(indices.len());
        self.timeline_indices = Rc::new(indices);
        cx.notify();
    }

    fn remember_window_size(&mut self, window: &mut Window) {
        let size = window.window_bounds().get_bounds().size;
        let width = f32::from(size.width).round();
        let height = f32::from(size.height).round();

        if !width.is_finite() || !height.is_finite() {
            return;
        }

        let window_size = WindowSize::normalized(width.max(0.0) as u32, height.max(0.0) as u32);
        if self.last_saved_window_size == Some(window_size) {
            return;
        }

        match self.database.set_window_size(window_size) {
            Ok(()) => {
                self.last_saved_window_size = Some(window_size);
            }
            Err(error) => {
                eprintln!("failed to save window size: {error:#}");
            }
        }
    }

    fn set_interval(&mut self, interval_ms: u64, cx: &mut Context<Self>) {
        match self.database.set_interval_ms(interval_ms) {
            Ok(interval_ms) => {
                self.interval_ms.store(interval_ms, Ordering::Relaxed);
                self.data.interval_ms = interval_ms;
                self.status = format!("采样间隔已设为 {}", format_interval(interval_ms));
            }
            Err(error) => {
                self.status = format!("保存失败: {error}");
            }
        }
        cx.notify();
    }

    fn toggle_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let next_theme = self.theme.toggled();
        match self.database.set_theme_preference(next_theme) {
            Ok(()) => {
                self.theme = next_theme;
                Theme::change(
                    if next_theme.is_dark() {
                        ThemeMode::Dark
                    } else {
                        ThemeMode::Light
                    },
                    Some(window),
                    cx,
                );
            }
            Err(error) => {
                self.status = format!("保存失败: {error}");
            }
        }
        cx.notify();
    }

    fn set_autostart(&mut self, enabled: bool, cx: &mut Context<Self>) {
        match startup::set_enabled(enabled, self.silent_start)
            .and_then(|_| self.database.set_autostart_enabled(enabled))
        {
            Ok(()) => {
                self.autostart_enabled = enabled;
            }
            Err(error) => {
                self.status = format!("保存失败: {error}");
            }
        }
        cx.notify();
    }

    fn set_silent_start(&mut self, enabled: bool, cx: &mut Context<Self>) {
        match self
            .database
            .set_silent_start(enabled)
            .and_then(|_| startup::set_enabled(self.autostart_enabled, enabled))
        {
            Ok(()) => {
                self.silent_start = enabled;
            }
            Err(error) => {
                self.status = format!("保存失败: {error}");
            }
        }
        cx.notify();
    }

    fn set_close_behavior(&mut self, behavior: CloseBehavior, cx: &mut Context<Self>) {
        match self.database.set_close_behavior(behavior) {
            Ok(()) => {
                self.close_behavior = behavior;
            }
            Err(error) => {
                self.status = format!("保存失败: {error}");
            }
        }
        cx.notify();
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let target = self.target_hwnd.clone();

        h_flex()
            .w_full()
            .h(px(48.))
            .min_h(px(48.))
            .max_h(px(48.))
            .flex_none()
            .items_center()
            .gap_3()
            .overflow_hidden()
            .window_control_area(WindowControlArea::Drag)
            .border_b_1()
            .border_color(cx.theme().border.opacity(0.52))
            .bg(cx.theme().background)
            .pl_4()
            .pr_2()
            .child(
                h_flex()
                    .h_full()
                    .items_center()
                    .gap_2()
                    .flex_shrink_0()
                    .window_control_area(WindowControlArea::Drag)
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(self.render_app_logo(cx))
                            .child(
                                div()
                                    .text_base()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .child("I Did Today"),
                            ),
                    ),
            )
            .child(self.render_nav(cx))
            .child(self.render_time_filters(cx))
            .child(
                div()
                    .h_full()
                    .flex_1()
                    .window_control_area(WindowControlArea::Drag),
            )
            .child(
                h_flex()
                    .h_full()
                    .items_center()
                    .gap_1()
                    .flex_shrink_0()
                    .child(
                        Button::new("theme-toggle")
                            .ghost()
                            .compact()
                            .small()
                            .rounded(px(7.))
                            .icon(if self.theme.is_dark() {
                                IconName::Sun
                            } else {
                                IconName::Moon
                            })
                            .tooltip(if self.theme.is_dark() {
                                "切换到日间主题"
                            } else {
                                "切换到夜间主题"
                            })
                            .on_click(
                                cx.listener(|view, _, window, cx| view.toggle_theme(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("minimize")
                            .ghost()
                            .compact()
                            .small()
                            .rounded(px(7.))
                            .icon(IconName::WindowMinimize)
                            .tooltip("最小化")
                            .on_click(move |_, _, _| {
                                tray::minimize_window(target.load(Ordering::Relaxed));
                            }),
                    )
                    .child(
                        Button::new("close")
                            .ghost()
                            .compact()
                            .small()
                            .rounded(px(7.))
                            .icon(IconName::WindowClose)
                            .tooltip("关闭")
                            .on_click(cx.listener(|view, _, _, cx| view.handle_close_button(cx))),
                    ),
            )
    }

    fn handle_close_button(&mut self, cx: &mut Context<Self>) {
        match self.close_behavior {
            CloseBehavior::Minimize => {
                tray::minimize_window(self.target_hwnd.load(Ordering::Relaxed));
            }
            CloseBehavior::HideToTray => {
                tray::hide_window(self.target_hwnd.load(Ordering::Relaxed));
            }
            CloseBehavior::Exit => {
                self.exit_requested.store(true, Ordering::Relaxed);
                cx.quit();
            }
        }
    }

    fn render_app_logo(&self, cx: &mut Context<Self>) -> AnyElement {
        let image = Arc::new(Image::from_bytes(
            ImageFormat::Svg,
            include_bytes!("../assets/idt-icon.svg").to_vec(),
        ));

        div()
            .size_7()
            .flex_shrink_0()
            .rounded(px(7.))
            .border_1()
            .border_color(cx.theme().border.opacity(0.35))
            .overflow_hidden()
            .bg(cx.theme().background)
            .child(img(image).size_full().object_fit(ObjectFit::Contain))
            .into_any_element()
    }

    fn render_nav(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .gap_1()
            .p_1()
            .rounded(px(8.))
            .bg(cx.theme().muted.opacity(0.62))
            .child(self.render_nav_button(
                "overview-tab",
                "总览",
                IconName::LayoutDashboard,
                ViewMode::Overview,
                cx,
            ))
            .child(self.render_nav_button(
                "timeline-tab",
                "时间线",
                IconName::Calendar,
                ViewMode::Timeline,
                cx,
            ))
            .child(self.render_nav_button(
                "settings-tab",
                "设置",
                IconName::Settings2,
                ViewMode::Settings,
                cx,
            ))
    }

    fn render_nav_button(
        &self,
        id: &'static str,
        label: &'static str,
        icon: IconName,
        mode: ViewMode,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        Button::new(id)
            .label(label)
            .icon(icon)
            .compact()
            .small()
            .rounded(px(6.))
            .selected(self.mode == mode)
            .on_click(cx.listener(move |view, _, _, cx| {
                view.mode = mode;
                cx.notify();
            }))
    }

    fn render_time_filters(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .items_center()
            .gap_1()
            .flex_shrink_0()
            .window_control_area(WindowControlArea::Drag)
            .child(self.render_time_filter_button(
                "range-24h",
                "24小时",
                TimeFilter::Last24Hours,
                cx,
            ))
            .child(self.render_time_filter_button("range-today", "当天", TimeFilter::Today, cx))
            .child(self.render_time_filter_button("range-week", "本周", TimeFilter::ThisWeek, cx))
            .child(self.render_time_filter_button("range-month", "本月", TimeFilter::ThisMonth, cx))
            .child(
                div()
                    .w(px(210.))
                    .child(
                        DatePicker::new(&self.range_picker)
                            .small()
                            .appearance(true)
                            .number_of_months(2),
                    )
                    .when(self.time_filter == TimeFilter::Custom, |this| {
                        this.border_1()
                            .rounded(px(7.))
                            .border_color(cx.theme().primary.opacity(0.55))
                    }),
            )
    }

    fn render_time_filter_button(
        &self,
        id: &'static str,
        label: &'static str,
        filter: TimeFilter,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        Button::new(id)
            .label(label)
            .compact()
            .small()
            .rounded(px(6.))
            .selected(self.time_filter == filter)
            .on_click(cx.listener(move |view, _, window, cx| {
                view.set_time_filter(filter, window, cx);
            }))
    }

    fn render_overview(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let app_count = self
            .data
            .app_totals
            .iter()
            .filter(|total| total.duration_ms > 0)
            .count();

        v_flex()
            .size_full()
            .gap_2()
            .child(
                h_flex()
                    .pt_2()
                    .gap_3()
                    .child(self.render_metric("总时长", format_duration(self.data.total_ms), cx))
                    .child(self.render_metric("应用数量", app_count.to_string(), cx))
                    .child(self.render_metric("记录数", self.data.timeline.len().to_string(), cx)),
            )
            .child(
                h_flex()
                    .gap_4()
                    .flex_1()
                    .min_h(px(0.))
                    .pb_2()
                    .overflow_hidden()
                    .child(self.render_usage_chart(cx))
                    .child(self.render_app_totals(cx)),
            )
    }

    fn render_timeline(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex().size_full().child(self.render_timeline_grid(cx))
    }

    fn render_timeline_grid(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let item_sizes = self.timeline_row_sizes.clone();
        let item_indices = self.timeline_indices.clone();
        let body = if self.timeline_indices.is_empty() {
            div()
                .relative()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(cx.theme().muted_foreground)
                .child("暂无记录")
                .into_any_element()
        } else {
            div()
                .relative()
                .flex_1()
                .child(
                    v_virtual_list(
                        cx.entity().clone(),
                        "timeline-virtual-list",
                        item_sizes,
                        move |view, visible_range, _, cx| {
                            let total = item_indices.len();
                            visible_range
                                .filter_map(|row_ix| {
                                    let visible_ix = total.checked_sub(row_ix + 1)?;
                                    let item_ix = *item_indices.get(visible_ix)?;
                                    let item = view.data.timeline.get(item_ix)?.clone();
                                    Some(
                                        view.render_timeline_row(row_ix, &item, cx)
                                            .into_any_element(),
                                    )
                                })
                                .collect::<Vec<_>>()
                        },
                    )
                    .track_scroll(&self.timeline_scroll)
                    .flex_1(),
                )
                .vertical_scrollbar(&self.timeline_scroll)
                .into_any_element()
        };

        div()
            .flex_1()
            .rounded_xl()
            .border_1()
            .border_color(cx.theme().border.opacity(0.55))
            .bg(cx.theme().background)
            .overflow_hidden()
            .child(
                v_flex()
                    .size_full()
                    .child(self.render_timeline_columns(cx))
                    .child(body),
            )
            .into_any_element()
    }

    fn render_timeline_columns(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .w_full()
            .h(px(34.))
            .items_center()
            .border_b_1()
            .border_color(cx.theme().border.opacity(0.5))
            .bg(cx.theme().muted.opacity(0.32))
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(timeline_head_cell(
                "时间",
                px(self.timeline_time_column_width()),
            ))
            .child(
                div()
                    .w(px(208.))
                    .h_full()
                    .px_2()
                    .flex()
                    .items_center()
                    .child(
                        Input::new(&self.process_filter_input)
                            .small()
                            .appearance(false)
                            .prefix(
                                Icon::new(IconName::Search)
                                    .xsmall()
                                    .text_color(cx.theme().muted_foreground),
                            )
                            .cleanable(true),
                    ),
            )
            .child(
                div().flex_1().h_full().px_2().flex().items_center().child(
                    Input::new(&self.title_filter_input)
                        .small()
                        .appearance(false)
                        .prefix(
                            Icon::new(IconName::Search)
                                .xsmall()
                                .text_color(cx.theme().muted_foreground),
                        )
                        .cleanable(true),
                ),
            )
            .child(timeline_head_cell("窗口类", px(150.)))
            .child(timeline_head_cell("时长", px(86.)))
    }

    fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
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

        v_flex()
            .size_full()
            .gap_4()
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
                                    .child("采样间隔"),
                            )
                            .child(h_flex().gap_2().children(interval_buttons)),
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
                                    .child(self.database.archive_dir().display().to_string()),
                            ),
                    ),
            )
    }

    fn render_metric(
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

    fn render_usage_chart(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let app_groups = self.grouped_app_totals();
        let chart_data = app_area_points(&self.data.timeline, &app_groups, &self.time_range);
        let tick_margin = (chart_data.len() / 6).max(1);
        let mut chart = AreaChart::new(chart_data)
            .x(|point: &AppAreaPoint| point.label.clone())
            .tick_margin(tick_margin);

        for ix in 0..app_groups.len() {
            let color = chart_color(ix, cx);
            chart = chart
                .y(move |point: &AppAreaPoint| point.values.get(ix).copied().unwrap_or(0.0))
                .stroke(color)
                .fill(linear_gradient(
                    0.,
                    linear_color_stop(color.opacity(0.34), 1.),
                    linear_color_stop(cx.theme().background.opacity(0.05), 0.),
                ));
        }

        div()
            .flex_1()
            .h_full()
            .min_h(px(0.))
            .min_w(px(0.))
            .rounded_xl()
            .border_1()
            .border_color(cx.theme().border.opacity(0.55))
            .bg(cx.theme().background)
            .p_4()
            .overflow_hidden()
            .child(
                v_flex()
                    .size_full()
                    .min_h(px(0.))
                    .gap_3()
                    .child(
                        div()
                            .flex_none()
                            .text_lg()
                            .font_weight(gpui::FontWeight::BOLD)
                            .child("应用趋势"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h(px(0.))
                            .w_full()
                            .child(if app_groups.is_empty() {
                                div()
                                    .size_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("暂无记录")
                                    .into_any_element()
                            } else {
                                chart.into_any_element()
                            }),
                    ),
            )
    }

    fn render_app_totals(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let groups = self.grouped_app_totals();
        let rows = if groups.is_empty() {
            vec![
                div()
                    .p_4()
                    .text_color(cx.theme().muted_foreground)
                    .child("暂无记录")
                    .into_any_element(),
            ]
        } else {
            groups
                .iter()
                .enumerate()
                .map(|(ix, total)| self.render_app_group(ix, total, cx).into_any_element())
                .collect::<Vec<_>>()
        };

        div()
            .w(px(360.))
            .h_full()
            .min_h(px(0.))
            .rounded_xl()
            .border_1()
            .border_color(cx.theme().border.opacity(0.55))
            .bg(cx.theme().background)
            .p_4()
            .overflow_hidden()
            .child(
                v_flex()
                    .size_full()
                    .min_h(px(0.))
                    .gap_3()
                    .child(
                        div()
                            .flex_none()
                            .text_lg()
                            .font_weight(gpui::FontWeight::BOLD)
                            .child("应用分布"),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_h(px(0.))
                            .gap_2()
                            .children(rows)
                            .overflow_y_scrollbar(),
                    ),
            )
    }

    fn render_app_group(
        &self,
        ix: usize,
        total: &AppGroup,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let accent = process_accent(&total.process_name, cx);
        let bar_color = chart_color(ix, cx);

        v_flex()
            .min_h(px(58.))
            .gap_2()
            .pb_2()
            .child(
                h_flex()
                    .justify_between()
                    .gap_3()
                    .items_center()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .overflow_hidden()
                            .child(self.render_process_icon(
                                &total.process_name,
                                if total.is_other {
                                    None
                                } else {
                                    total.icon_png.as_deref()
                                },
                                accent,
                                cx,
                            ))
                            .child(
                                div()
                                    .text_sm()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(total.process_name.clone()),
                            ),
                    )
                    .child(
                        div()
                            .text_sm()
                            .flex_shrink_0()
                            .text_color(cx.theme().muted_foreground)
                            .child(format_duration(total.duration_ms)),
                    ),
            )
            .child(
                div()
                    .h(px(7.))
                    .w_full()
                    .rounded_full()
                    .bg(cx.theme().muted)
                    .overflow_hidden()
                    .child(
                        div()
                            .h_full()
                            .w(relative(total.percent.clamp(0.0, 1.0)))
                            .bg(bar_color),
                    ),
            )
    }

    fn render_timeline_row(
        &self,
        row_ix: usize,
        item: &TimelineItem,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let accent = process_accent(&item.process_name, cx);
        let show_date = !matches!(self.time_filter, TimeFilter::Today);
        let time_width = self.timeline_time_column_width();
        let row_bg = if row_ix % 2 == 0 {
            cx.theme().background
        } else {
            cx.theme().muted.opacity(0.18)
        };
        let title = if item.window_title.trim().is_empty() {
            "无标题".to_owned()
        } else {
            item.window_title.clone()
        };

        h_flex()
            .w_full()
            .h(px(TIMELINE_ROW_HEIGHT))
            .items_center()
            .border_b_1()
            .border_color(cx.theme().border.opacity(0.28))
            .bg(row_bg)
            .hover(|style| style.bg(cx.theme().muted.opacity(0.42)))
            .child(
                v_flex()
                    .w(px(time_width))
                    .h_full()
                    .justify_center()
                    .px_3()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(cx.theme().success)
                            .child(date_clock(item.started_at_ms, show_date)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(cx.theme().info)
                            .child(date_clock(item.ended_at_ms, show_date)),
                    ),
            )
            .child(
                h_flex()
                    .w(px(208.))
                    .h_full()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .child(self.render_process_icon(
                        &item.process_name,
                        item.icon_png.as_deref(),
                        accent,
                        cx,
                    ))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::BOLD)
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(item.process_name.clone()),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .px_3()
                    .flex()
                    .items_center()
                    .overflow_hidden()
                    .child(
                        div()
                            .text_sm()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(title),
                    ),
            )
            .child(
                div()
                    .w(px(150.))
                    .h_full()
                    .px_3()
                    .flex()
                    .items_center()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .overflow_hidden()
                    .child(
                        div()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(item.window_class.clone()),
                    ),
            )
            .child(
                div()
                    .w(px(86.))
                    .h_full()
                    .px_3()
                    .flex()
                    .items_center()
                    .justify_end()
                    .text_xs()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(accent)
                    .child(format_duration(item.duration_ms)),
            )
    }

    fn render_process_icon(
        &self,
        process_name: &str,
        icon_png: Option<&[u8]>,
        accent: Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let base = div()
            .size_6()
            .flex_shrink_0()
            .rounded(px(6.))
            .bg(accent.opacity(0.12))
            .border_1()
            .border_color(cx.theme().border.opacity(0.35))
            .overflow_hidden()
            .flex()
            .items_center()
            .justify_center();

        if let Some(icon_png) = icon_png {
            let image = Arc::new(Image::from_bytes(ImageFormat::Png, icon_png.to_vec()));
            return base
                .child(img(image).size_full().object_fit(ObjectFit::Contain))
                .into_any_element();
        }

        base.text_xs()
            .font_weight(gpui::FontWeight::BOLD)
            .text_color(accent)
            .child(process_initial(process_name))
            .into_any_element()
    }

    fn timeline_time_column_width(&self) -> f32 {
        if matches!(self.time_filter, TimeFilter::Today) {
            116.0
        } else {
            178.0
        }
    }

    fn app_display_count(&self) -> usize {
        let usable_width = (self.window_width - 460.0).max(360.0);
        let width_count = (usable_width / 180.0).floor() as usize;
        let available_height = (self.window_height - 286.0).max(210.0);
        let height_count = (available_height / 42.0).floor() as usize;

        height_count.max(width_count).clamp(3, 16)
    }

    fn grouped_app_totals(&self) -> Vec<AppGroup> {
        group_app_totals(
            &self.data.app_totals,
            self.data.total_ms,
            self.app_display_count(),
        )
    }
}

impl Render for Dashboard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport_size = window.viewport_size();
        self.window_width = viewport_size.width.as_f32();
        self.window_height = viewport_size.height.as_f32();
        let body = match self.mode {
            ViewMode::Overview => self.render_overview(cx).into_any_element(),
            ViewMode::Timeline => self.render_timeline(cx).into_any_element(),
            ViewMode::Settings => self.render_settings(cx).into_any_element(),
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
                        .child(div().flex_1().min_h(px(0.)).overflow_hidden().child(body)),
                ),
            )
    }
}

fn timeline_row_sizes(count: usize) -> Rc<Vec<GpuiSize<Pixels>>> {
    Rc::new(
        (0..count)
            .map(|_| size(px(1_160.), px(TIMELINE_ROW_HEIGHT)))
            .collect(),
    )
}

fn timeline_head_cell(label: &'static str, width: Pixels) -> gpui::Div {
    div()
        .w(width)
        .h_full()
        .px_3()
        .flex()
        .items_center()
        .font_weight(gpui::FontWeight::BOLD)
        .child(label)
}

fn close_behavior_index(behavior: CloseBehavior) -> usize {
    match behavior {
        CloseBehavior::Minimize => 0,
        CloseBehavior::HideToTray => 1,
        CloseBehavior::Exit => 2,
    }
}

impl TimeFilter {
    fn range(self, custom_range: (NaiveDate, NaiveDate)) -> TimeRange {
        let now = Local::now();
        let today = now.date_naive();

        match self {
            Self::Last24Hours => {
                let end_ms = now.timestamp_millis();
                let start_ms = end_ms.saturating_sub(DAY_MS);
                TimeRange {
                    start_ms,
                    end_ms,
                    label: "最近24小时".to_owned(),
                }
            }
            Self::Today => {
                let (start_ms, end_ms) = day_bounds(today);
                TimeRange {
                    start_ms,
                    end_ms,
                    label: today.format("%Y-%m-%d").to_string(),
                }
            }
            Self::ThisWeek => {
                let start =
                    today - ChronoDuration::days(today.weekday().num_days_from_monday() as i64);
                let end = start + ChronoDuration::days(7);
                TimeRange {
                    start_ms: local_midnight_ms(start),
                    end_ms: local_midnight_ms(end),
                    label: format!(
                        "{} - {}",
                        start.format("%Y-%m-%d"),
                        (end - ChronoDuration::days(1)).format("%Y-%m-%d")
                    ),
                }
            }
            Self::ThisMonth => {
                let start = NaiveDate::from_ymd_opt(today.year(), today.month(), 1)
                    .expect("month start should resolve");
                let end = if today.month() == 12 {
                    NaiveDate::from_ymd_opt(today.year() + 1, 1, 1)
                } else {
                    NaiveDate::from_ymd_opt(today.year(), today.month() + 1, 1)
                }
                .expect("next month start should resolve");
                TimeRange {
                    start_ms: local_midnight_ms(start),
                    end_ms: local_midnight_ms(end),
                    label: start.format("%Y-%m").to_string(),
                }
            }
            Self::Custom => {
                let (start, end) = ordered_dates(custom_range.0, custom_range.1);
                let range_end = end + ChronoDuration::days(1);
                TimeRange {
                    start_ms: local_midnight_ms(start),
                    end_ms: local_midnight_ms(range_end),
                    label: format!("{} - {}", start.format("%Y-%m-%d"), end.format("%Y-%m-%d")),
                }
            }
        }
    }

    fn date_range_for_picker(self, custom_range: (NaiveDate, NaiveDate)) -> (NaiveDate, NaiveDate) {
        let now = Local::now();
        let today = now.date_naive();

        match self {
            Self::Last24Hours => (today - ChronoDuration::days(1), today),
            Self::Today => (today, today),
            Self::ThisWeek => {
                let start =
                    today - ChronoDuration::days(today.weekday().num_days_from_monday() as i64);
                (start, start + ChronoDuration::days(6))
            }
            Self::ThisMonth => {
                let start = NaiveDate::from_ymd_opt(today.year(), today.month(), 1)
                    .expect("month start should resolve");
                let next_month = if today.month() == 12 {
                    NaiveDate::from_ymd_opt(today.year() + 1, 1, 1)
                } else {
                    NaiveDate::from_ymd_opt(today.year(), today.month() + 1, 1)
                }
                .expect("next month start should resolve");
                (start, next_month - ChronoDuration::days(1))
            }
            Self::Custom => ordered_dates(custom_range.0, custom_range.1),
        }
    }
}

fn ordered_dates(start: NaiveDate, end: NaiveDate) -> (NaiveDate, NaiveDate) {
    if start <= end {
        (start, end)
    } else {
        (end, start)
    }
}

fn day_bounds(date: NaiveDate) -> (i64, i64) {
    let start_ms = local_midnight_ms(date);
    let end_ms = local_midnight_ms(date + ChronoDuration::days(1));
    (start_ms, end_ms)
}

fn local_midnight_ms(date: NaiveDate) -> i64 {
    let start_naive = date
        .and_hms_opt(0, 0, 0)
        .expect("midnight should always be valid");
    Local
        .from_local_datetime(&start_naive)
        .earliest()
        .expect("local midnight should resolve")
        .timestamp_millis()
}

fn filtered_timeline_indices(
    timeline: &[TimelineItem],
    process_filter: &str,
    title_filter: &str,
) -> Vec<usize> {
    let process_filter = process_filter.trim().to_lowercase();
    let title_filter = title_filter.trim().to_lowercase();

    timeline
        .iter()
        .enumerate()
        .filter_map(|(ix, item)| {
            let process_matches = process_filter.is_empty()
                || item.process_name.to_lowercase().contains(&process_filter);
            let title_matches =
                title_filter.is_empty() || item.window_title.to_lowercase().contains(&title_filter);

            (process_matches && title_matches).then_some(ix)
        })
        .collect()
}

fn group_app_totals(app_totals: &[AppTotal], total_ms: u64, display_count: usize) -> Vec<AppGroup> {
    let display_count = display_count.max(1);
    let visible_count = app_totals.len().min(display_count);
    let mut groups = app_totals
        .iter()
        .take(visible_count)
        .map(|total| AppGroup {
            process_name: total.process_name.clone(),
            icon_png: total.icon_png.clone(),
            duration_ms: total.duration_ms,
            percent: total.percent,
            processes: vec![total.process_name.clone()],
            is_other: false,
        })
        .collect::<Vec<_>>();

    if app_totals.len() > visible_count {
        let mut other_ms = 0_u64;
        let mut processes = Vec::new();
        for total in app_totals.iter().skip(visible_count) {
            other_ms = other_ms.saturating_add(total.duration_ms);
            processes.push(total.process_name.clone());
        }
        if other_ms > 0 {
            groups.push(AppGroup {
                process_name: "其他".to_owned(),
                icon_png: None,
                duration_ms: other_ms,
                percent: if total_ms == 0 {
                    0.0
                } else {
                    other_ms as f32 / total_ms as f32
                },
                processes,
                is_other: true,
            });
        }
    }

    groups
}

fn app_area_points(
    timeline: &[TimelineItem],
    groups: &[AppGroup],
    time_range: &TimeRange,
) -> Vec<AppAreaPoint> {
    let buckets = chart_buckets(time_range);
    if buckets.is_empty() {
        return Vec::new();
    }

    let group_lookup = groups
        .iter()
        .enumerate()
        .flat_map(|(group_ix, group)| {
            group
                .processes
                .iter()
                .map(move |process| (process.clone(), group_ix))
        })
        .collect::<HashMap<_, _>>();

    let mut values = vec![vec![0_f64; groups.len()]; buckets.len()];
    for item in timeline {
        let Some(group_ix) = group_lookup.get(&item.process_name).copied() else {
            continue;
        };

        for (bucket_ix, bucket) in buckets.iter().enumerate() {
            let overlap_start = item.started_at_ms.max(bucket.start_ms);
            let overlap_end = item.ended_at_ms.min(bucket.end_ms);
            if overlap_end > overlap_start {
                values[bucket_ix][group_ix] +=
                    overlap_end.saturating_sub(overlap_start) as f64 / 60_000.0;
            }
        }
    }

    let values = smooth_area_values(&values);

    buckets
        .into_iter()
        .enumerate()
        .map(|(ix, bucket)| AppAreaPoint {
            label: bucket.label,
            values: values.get(ix).cloned().unwrap_or_default(),
        })
        .collect()
}

fn smooth_area_values(values: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let bucket_count = values.len();
    let series_count = values.first().map_or(0, Vec::len);
    if bucket_count < 3 || series_count == 0 {
        return values.to_vec();
    }

    let mut smoothed = vec![vec![0.0; series_count]; bucket_count];
    for series_ix in 0..series_count {
        let raw_sum = values
            .iter()
            .map(|bucket| bucket.get(series_ix).copied().unwrap_or(0.0))
            .sum::<f64>();
        if raw_sum <= f64::EPSILON {
            continue;
        }

        let peak = values
            .iter()
            .map(|bucket| bucket[series_ix])
            .fold(0.0_f64, f64::max);
        let floor = (peak * 0.006).clamp(0.002, 0.05);

        for bucket_ix in 0..bucket_count {
            let previous = if bucket_ix == 0 {
                values[bucket_ix][series_ix]
            } else {
                values[bucket_ix - 1][series_ix]
            };
            let current = values[bucket_ix][series_ix];
            let next = values
                .get(bucket_ix + 1)
                .map(|bucket| bucket[series_ix])
                .unwrap_or(current);

            smoothed[bucket_ix][series_ix] = current * 0.64 + (previous + next) * 0.18;
        }

        let first_pass = smoothed
            .iter()
            .map(|bucket| bucket[series_ix])
            .collect::<Vec<_>>();
        for bucket_ix in 0..bucket_count {
            let previous = if bucket_ix == 0 {
                first_pass[bucket_ix]
            } else {
                first_pass[bucket_ix - 1]
            };
            let current = first_pass[bucket_ix];
            let next = first_pass.get(bucket_ix + 1).copied().unwrap_or(current);

            smoothed[bucket_ix][series_ix] = (current * 0.7 + (previous + next) * 0.15).max(floor);
        }

        let smoothed_sum = smoothed.iter().map(|bucket| bucket[series_ix]).sum::<f64>();
        if smoothed_sum <= f64::EPSILON {
            continue;
        }

        let scale = raw_sum / smoothed_sum;
        for bucket in &mut smoothed {
            bucket[series_ix] = (bucket[series_ix] * scale).max(floor);
        }
    }

    smoothed
}

fn chart_buckets(time_range: &TimeRange) -> Vec<ChartBucket> {
    let duration_ms = time_range.end_ms.saturating_sub(time_range.start_ms);
    if duration_ms <= 0 {
        return Vec::new();
    }

    let bucket_count = if duration_ms <= DAY_MS {
        24
    } else {
        ((duration_ms + DAY_MS - 1) / DAY_MS).clamp(1, 31) as usize
    };
    let bucket_ms = ((duration_ms as f64 / bucket_count as f64).ceil() as i64).max(1);

    (0..bucket_count)
        .filter_map(|ix| {
            let start_ms = time_range
                .start_ms
                .saturating_add(bucket_ms.saturating_mul(ix as i64));
            if start_ms >= time_range.end_ms {
                return None;
            }
            let end_ms = start_ms.saturating_add(bucket_ms).min(time_range.end_ms);
            Some(ChartBucket {
                start_ms,
                end_ms,
                label: bucket_label(start_ms, duration_ms),
            })
        })
        .collect()
}

fn bucket_label(start_ms: i64, range_ms: i64) -> SharedString {
    Local
        .timestamp_millis_opt(start_ms)
        .single()
        .map(|time| {
            if range_ms <= DAY_MS {
                time.format("%H:%M").to_string().into()
            } else {
                time.format("%m-%d").to_string().into()
            }
        })
        .unwrap_or_else(|| "".into())
}

fn empty_dashboard(interval_ms: u64) -> DashboardData {
    DashboardData {
        total_ms: 0,
        interval_ms,
        app_totals: Vec::new(),
        timeline: Vec::new(),
    }
}

fn format_interval(interval_ms: u64) -> String {
    if interval_ms % 1_000 == 0 {
        format!("{}s", interval_ms / 1_000)
    } else if interval_ms % 100 == 0 {
        format!("{:.1}s", interval_ms as f64 / 1_000.0)
    } else {
        format!("{interval_ms}ms")
    }
}

fn process_initial(process_name: &str) -> String {
    process_name
        .trim()
        .chars()
        .find(|ch| ch.is_alphanumeric())
        .map(|ch| ch.to_uppercase().collect())
        .unwrap_or_else(|| "?".to_owned())
}

fn format_duration(duration_ms: u64) -> String {
    let total_seconds = duration_ms / 1_000;
    let hours = total_seconds / 3_600;
    let minutes = (total_seconds % 3_600) / 60;
    let seconds = total_seconds % 60;

    if hours > 0 {
        format!("{hours}小时{minutes:02}分")
    } else if minutes > 0 {
        format!("{minutes}分{seconds:02}秒")
    } else {
        format!("{seconds}秒")
    }
}

fn process_accent(process_name: &str, cx: &mut Context<Dashboard>) -> Hsla {
    let palette = [
        cx.theme().chart_1,
        cx.theme().chart_2,
        cx.theme().chart_3,
        cx.theme().chart_4,
        cx.theme().chart_5,
    ];
    let hash = process_name.bytes().fold(0_usize, |acc, byte| {
        acc.wrapping_mul(31).wrapping_add(byte as usize)
    });
    palette[hash % palette.len()]
}

fn chart_color(ix: usize, cx: &mut Context<Dashboard>) -> Hsla {
    let palette = [
        cx.theme().blue,
        cx.theme().green,
        cx.theme().magenta,
        cx.theme().yellow,
        cx.theme().cyan,
        cx.theme().red,
        cx.theme().warning,
        cx.theme().info,
    ];
    palette[ix % palette.len()]
}

fn date_clock(timestamp_ms: i64, show_date: bool) -> String {
    Local
        .timestamp_millis_opt(timestamp_ms)
        .single()
        .map(|time| {
            if show_date {
                time.format("%Y-%m-%d %H:%M:%S").to_string()
            } else {
                time.format("%H:%M:%S").to_string()
            }
        })
        .unwrap_or_else(|| {
            if show_date {
                "---- -- --:--:--".to_owned()
            } else {
                "--:--:--".to_owned()
            }
        })
}
