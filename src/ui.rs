use std::{
    ops::Range,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering},
    },
    time::Duration,
};

use chrono::{Local, NaiveDate};
use gpui::{
    AnyElement, AppContext as _, Context, Entity, Hsla, Image, ImageFormat,
    InteractiveElement as _, IntoElement, ObjectFit, ParentElement as _, SharedString, Styled as _,
    StyledImage as _, Subscription, UniformListScrollHandle, Window, WindowControlArea, div, img,
    linear_color_stop, linear_gradient, prelude::FluentBuilder as _, px, relative, uniform_list,
};
use gpui_component::{
    ActiveTheme, Icon, IconName, Selectable as _, Sizable as _, Theme, ThemeMode,
    button::{Button, ButtonVariants as _},
    calendar::Date,
    chart::AreaChart,
    date_picker::{DatePicker, DatePickerEvent, DatePickerState},
    h_flex,
    input::{Input, InputEvent, InputState},
    scroll::ScrollableElement as _,
    v_flex,
};
use smol::Timer;

use crate::{
    db::{
        CloseBehavior, DEFAULT_INTERVAL_MS, DashboardData, Database, ThemePreference, TimelineItem,
        WindowSize,
    },
    startup,
    todo_db::TodoDatabase,
    todo_ui::TodoPanel,
    tracker::TrackerHandle,
    tray,
    ui_controls::red_icon_button_variant,
};

mod helpers;
mod render;
mod settings;
mod todo_header;

use helpers::*;

const INTERVAL_PRESETS: [u64; 5] = [200, 500, 1_000, 3_000, 5_000];
const CACHE_FLUSH_PRESETS: [u64; 4] = [5_000, 10_000, 30_000, 60_000];
const TIMELINE_ROW_HEIGHT: f32 = 58.0;
const TIMELINE_PAGE_SIZE: usize = 1000;
const HOUR_MS: i64 = 60 * 60 * 1_000;
const DAY_MS: i64 = 24 * HOUR_MS;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ViewMode {
    Overview,
    Timeline,
    Settings,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ShellMode {
    Activity,
    Todo,
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
}

#[derive(Clone)]
struct AppAreaPoint {
    label: SharedString,
    values: Vec<f64>,
}

#[derive(Clone)]
struct AppGroup {
    process_name: String,
    icon_png: Option<Arc<[u8]>>,
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

#[derive(Default)]
struct TimelineCache {
    start: usize,
    items: Vec<TimelineItem>,
    process_filter: String,
    title_filter: String,
}

pub struct Dashboard {
    database: Database,
    interval_ms: Arc<AtomicU64>,
    cache_flush_interval_ms: Arc<AtomicU64>,
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
    timeline_count: usize,
    timeline_cache: TimelineCache,
    timeline_scroll: UniformListScrollHandle,
    window_width: f32,
    window_height: f32,
    sleeping_to_tray: bool,
    data_loaded: bool,
    last_saved_window_size: Option<WindowSize>,
    theme: ThemePreference,
    autostart_enabled: bool,
    silent_start: bool,
    close_behavior: CloseBehavior,
    cache_flush_interval_value_ms: u64,
    todo_database_path: PathBuf,
    mode: ViewMode,
    shell_mode: ShellMode,
    todo_panel: Entity<TodoPanel>,
    status: String,
    _subscriptions: Vec<Subscription>,
}

impl Dashboard {
    pub fn new(
        database: Database,
        interval_ms: Arc<AtomicU64>,
        cache_flush_interval_ms: Arc<AtomicU64>,
        exit_requested: Arc<AtomicBool>,
        target_hwnd: Arc<AtomicIsize>,
        tracker: TrackerHandle,
        todo_database: TodoDatabase,
        start_hidden: bool,
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
                cache_flush_interval_ms: crate::db::DEFAULT_CACHE_FLUSH_INTERVAL_MS,
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
        let todo_database_path = todo_database.path().to_path_buf();
        let todo_panel = cx.new(|cx| TodoPanel::new(todo_database, window, cx));

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
                    view.refresh_from_interaction(cx);
                }
            }),
            cx.subscribe_in(&process_filter_input, window, |view, _, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    view.refresh_timeline_count(cx);
                }
            }),
            cx.subscribe_in(&title_filter_input, window, |view, _, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    view.refresh_timeline_count(cx);
                }
            }),
        ];

        let (data, timeline_count) = if start_hidden {
            (
                empty_dashboard(database.get_interval_ms().unwrap_or(DEFAULT_INTERVAL_MS)),
                0,
            )
        } else {
            let data = database
                .dashboard_range(
                    time_range.start_ms,
                    time_range.end_ms,
                    &dashboard_buckets(&time_range),
                )
                .unwrap_or_else(|error| {
                    eprintln!("failed to load dashboard data: {error:#}");
                    empty_dashboard(DEFAULT_INTERVAL_MS)
                });
            let timeline_count = database
                .timeline_count(
                    time_range.start_ms,
                    time_range.end_ms,
                    process_filter_input.read(cx).value().as_ref(),
                    title_filter_input.read(cx).value().as_ref(),
                )
                .unwrap_or_else(|error| {
                    eprintln!("failed to count timeline data: {error:#}");
                    0
                });
            (data, timeline_count)
        };

        let dashboard = Self {
            database,
            interval_ms,
            cache_flush_interval_ms,
            exit_requested,
            target_hwnd,
            _tracker: tracker,
            time_filter,
            time_range,
            custom_range,
            range_picker,
            process_filter_input,
            title_filter_input,
            timeline_count,
            timeline_cache: TimelineCache::default(),
            timeline_scroll: UniformListScrollHandle::new(),
            window_width: crate::db::DEFAULT_WINDOW_WIDTH as f32,
            window_height: crate::db::DEFAULT_WINDOW_HEIGHT as f32,
            sleeping_to_tray: start_hidden,
            data_loaded: !start_hidden,
            last_saved_window_size,
            theme: settings.theme,
            autostart_enabled,
            silent_start: settings.silent_start,
            close_behavior: settings.close_behavior,
            cache_flush_interval_value_ms: settings.cache_flush_interval_ms,
            todo_database_path,
            data,
            mode: ViewMode::Overview,
            shell_mode: ShellMode::Activity,
            todo_panel,
            status: if start_hidden {
                "后台运行中".to_owned()
            } else {
                "运行中".to_owned()
            },
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
                        view.refresh_visible(cx);
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

    fn refresh_visible(&mut self, cx: &mut Context<Self>) {
        let show_requested = tray::take_show_requested();
        if self.sleeping_to_tray {
            let visible = tray::is_window_visible(self.target_hwnd.load(Ordering::Relaxed));
            if !show_requested && !visible {
                return;
            }
            self.sleeping_to_tray = false;
        }

        if self.mode == ViewMode::Settings {
            return;
        }

        self.refresh(cx);
    }

    fn refresh_from_interaction(&mut self, cx: &mut Context<Self>) {
        self.sleeping_to_tray = false;
        self.refresh(cx);
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let time_range = self.time_filter.range(self.custom_range);
        match self.database.dashboard_range(
            time_range.start_ms,
            time_range.end_ms,
            &dashboard_buckets(&time_range),
        ) {
            Ok(data) => {
                self.time_range = time_range;
                self.data = data;
                self.data_loaded = true;
                if self.update_timeline_count(cx).is_ok() {
                    self.status = "运行中".to_owned();
                }
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
        self.refresh_from_interaction(cx);
    }

    fn sync_range_picker(&self, window: &mut Window, cx: &mut Context<Self>) {
        let (start, end) = self.time_filter.date_range_for_picker(self.custom_range);
        self.range_picker.update(cx, |picker, cx| {
            picker.set_date((start, end), window, cx);
        });
    }

    fn refresh_timeline_count(&mut self, cx: &mut Context<Self>) {
        self.sleeping_to_tray = false;
        if !self.data_loaded {
            self.refresh(cx);
            return;
        }

        let _ = self.update_timeline_count(cx);
        cx.notify();
    }

    fn update_timeline_count(&mut self, cx: &mut Context<Self>) -> Result<(), ()> {
        let process_filter = self.process_filter_input.read(cx).value();
        let title_filter = self.title_filter_input.read(cx).value();
        match self.database.timeline_count(
            self.time_range.start_ms,
            self.time_range.end_ms,
            &process_filter,
            &title_filter,
        ) {
            Ok(count) => {
                self.timeline_count = count;
                self.timeline_cache = TimelineCache::default();
                if self.mode == ViewMode::Timeline {
                    self.load_timeline_cache_for_current_filter(0, cx);
                }
            }
            Err(error) => {
                self.timeline_count = 0;
                self.timeline_cache = TimelineCache::default();
                self.status = format!("读取失败: {error}");
                return Err(());
            }
        }
        Ok(())
    }

    fn preload_timeline_first_page(&mut self, cx: &mut Context<Self>) {
        if self.timeline_count == 0 {
            self.timeline_cache = TimelineCache::default();
            cx.notify();
            return;
        }

        let process_filter = self.process_filter_input.read(cx).value().to_string();
        let title_filter = self.title_filter_input.read(cx).value().to_string();
        if !self.timeline_cache_contains(0, &process_filter, &title_filter) {
            self.load_timeline_cache_with_filters(0, process_filter, title_filter, cx);
            return;
        }

        cx.notify();
    }

    fn load_timeline_cache_for_current_filter(&mut self, offset: usize, cx: &mut Context<Self>) {
        let process_filter = self.process_filter_input.read(cx).value().to_string();
        let title_filter = self.title_filter_input.read(cx).value().to_string();
        self.load_timeline_cache_with_filters(offset, process_filter, title_filter, cx);
    }

    pub fn release_view_data(&mut self, cx: &mut Context<Self>) {
        self.sleeping_to_tray = true;
        if !self.data_loaded && self.timeline_cache.items.is_empty() {
            self.status = "后台运行中".to_owned();
            return;
        }

        let interval_ms = self.data.interval_ms;
        self.data = empty_dashboard(interval_ms);
        self.timeline_count = 0;
        self.timeline_cache = TimelineCache::default();
        self.data_loaded = false;
        self.status = "后台运行中".to_owned();
        tray::trim_working_set();
        cx.notify();
    }

    pub fn persist_window_size(&mut self, window: &mut Window) {
        self.remember_window_size(window);
    }

    fn remember_window_size(&mut self, window: &mut Window) {
        let size = window.viewport_size();
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

    fn set_cache_flush_interval(&mut self, interval_ms: u64, cx: &mut Context<Self>) {
        match self.database.set_cache_flush_interval_ms(interval_ms) {
            Ok(interval_ms) => {
                self.cache_flush_interval_ms
                    .store(interval_ms, Ordering::Relaxed);
                self.cache_flush_interval_value_ms = interval_ms;
                self.status = format!("缓存写入周期已设为 {}", format_interval(interval_ms));
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

    fn toggle_shell_mode(&mut self, cx: &mut Context<Self>) {
        self.shell_mode = match self.shell_mode {
            ShellMode::Activity => ShellMode::Todo,
            ShellMode::Todo => ShellMode::Activity,
        };
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
                Button::new("shell-mode-switch")
                    .ghost()
                    .compact()
                    .rounded(px(9.))
                    .h(px(38.))
                    .px_2()
                    .tooltip("切换功能")
                    .on_click(cx.listener(|view, _, _, cx| view.toggle_shell_mode(cx)))
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
            .child(
                h_flex()
                    .h_full()
                    .flex_1()
                    .min_w(px(0.))
                    .items_center()
                    .gap_3()
                    .overflow_hidden()
                    .window_control_area(WindowControlArea::Drag)
                    .when(self.shell_mode == ShellMode::Activity, |this| {
                        this.child(self.render_nav(cx))
                            .child(self.render_time_filters(cx))
                    })
                    .when(self.shell_mode == ShellMode::Todo, |this| {
                        this.child(self.render_todo_header_controls(cx))
                    }),
            )
            .child(
                h_flex()
                    .h_full()
                    .w(px(104.))
                    .min_w(px(104.))
                    .max_w(px(104.))
                    .justify_end()
                    .items_center()
                    .gap_1()
                    .flex_shrink_0()
                    .child(
                        Button::new("theme-toggle")
                            .ghost()
                            .compact()
                            .small()
                            .w(px(30.))
                            .h(px(30.))
                            .min_w(px(30.))
                            .min_h(px(30.))
                            .flex_none()
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
                            .w(px(30.))
                            .h(px(30.))
                            .min_w(px(30.))
                            .min_h(px(30.))
                            .flex_none()
                            .rounded(px(7.))
                            .icon(IconName::WindowMinimize)
                            .tooltip("最小化")
                            .on_click(move |_, _, _| {
                                tray::minimize_window(target.load(Ordering::Relaxed));
                            }),
                    )
                    .child(
                        Button::new("close")
                            .custom(red_icon_button_variant(cx))
                            .compact()
                            .small()
                            .w(px(30.))
                            .h(px(30.))
                            .min_w(px(30.))
                            .min_h(px(30.))
                            .flex_none()
                            .rounded(px(7.))
                            .icon(IconName::WindowClose)
                            .tooltip("关闭")
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.handle_close_button(window, cx)
                            })),
                    ),
            )
    }

    fn handle_close_button(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.remember_window_size(window);
        match self.close_behavior {
            CloseBehavior::Minimize => {
                tray::minimize_window(self.target_hwnd.load(Ordering::Relaxed));
            }
            CloseBehavior::HideToTray => {
                self.release_view_data(cx);
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
                if !view.data_loaded && mode != ViewMode::Settings {
                    view.refresh_from_interaction(cx);
                    return;
                }
                if mode == ViewMode::Timeline {
                    view.preload_timeline_first_page(cx);
                    return;
                }
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
                    .child(self.render_metric("记录数", self.data.record_count.to_string(), cx)),
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
        let item_count = self.timeline_count;
        let body = if item_count == 0 {
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
                .min_h(px(0.))
                .child(
                    uniform_list(
                        "timeline-virtual-list",
                        item_count,
                        cx.processor(move |view, visible_range: Range<usize>, _, cx| {
                            visible_range
                                .map(|row_ix| {
                                    match view.timeline_item_for_row(row_ix, item_count, cx) {
                                        Some(item) => view
                                            .render_timeline_row(row_ix, &item, cx)
                                            .into_any_element(),
                                        None => view.render_loading_timeline_row(row_ix, cx),
                                    }
                                })
                                .collect::<Vec<_>>()
                        }),
                    )
                    .size_full()
                    .track_scroll(self.timeline_scroll.clone()),
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

    fn render_usage_chart(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let app_groups = self.grouped_app_totals();
        let chart_data = app_area_points(&self.data, &app_groups, &self.time_range);
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

    fn render_loading_timeline_row(&self, row_ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let row_bg = if row_ix % 2 == 0 {
            cx.theme().background
        } else {
            cx.theme().muted.opacity(0.18)
        };

        h_flex()
            .w_full()
            .h(px(TIMELINE_ROW_HEIGHT))
            .items_center()
            .border_b_1()
            .border_color(cx.theme().border.opacity(0.28))
            .bg(row_bg)
            .px_3()
            .text_color(cx.theme().muted_foreground)
            .child("加载中")
            .into_any_element()
    }

    fn timeline_item_for_row(
        &mut self,
        row_ix: usize,
        total_count: usize,
        cx: &mut Context<Self>,
    ) -> Option<TimelineItem> {
        if row_ix >= total_count {
            return None;
        }
        let offset = row_ix;
        let process_filter = self.process_filter_input.read(cx).value().to_string();
        let title_filter = self.title_filter_input.read(cx).value().to_string();
        if !self.timeline_cache_contains(offset, &process_filter, &title_filter) {
            self.load_timeline_cache_with_filters(offset, process_filter, title_filter, cx);
        }

        self.timeline_cache
            .items
            .get(offset.saturating_sub(self.timeline_cache.start))
            .cloned()
    }

    fn timeline_cache_contains(
        &self,
        offset: usize,
        process_filter: &str,
        title_filter: &str,
    ) -> bool {
        offset >= self.timeline_cache.start
            && offset
                < self
                    .timeline_cache
                    .start
                    .saturating_add(self.timeline_cache.items.len())
            && self.timeline_cache.process_filter == process_filter
            && self.timeline_cache.title_filter == title_filter
    }

    fn load_timeline_cache_with_filters(
        &mut self,
        offset: usize,
        process_filter: String,
        title_filter: String,
        cx: &mut Context<Self>,
    ) {
        let start = offset.saturating_sub(TIMELINE_PAGE_SIZE / 4);
        let limit = TIMELINE_PAGE_SIZE.min(self.timeline_count.saturating_sub(start));

        match self.database.timeline_page(
            self.time_range.start_ms,
            self.time_range.end_ms,
            &process_filter,
            &title_filter,
            start,
            limit,
        ) {
            Ok(items) => {
                self.timeline_cache = TimelineCache {
                    start,
                    items,
                    process_filter,
                    title_filter,
                };
            }
            Err(error) => {
                self.timeline_cache = TimelineCache::default();
                self.status = format!("读取失败: {error}");
            }
        }

        cx.notify();
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
