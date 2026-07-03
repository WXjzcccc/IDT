use std::time::Duration;

use crate::{
    app_icon,
    todo_db::{
        TODO_WINDOW_MIN_HEIGHT, TODO_WINDOW_MIN_WIDTH, TodoDatabase, TodoDraft, TodoItem,
        TodoSubtaskDraft, TodoTag, date_from_ms, day_end_ms, first_day_of_month, local_midnight_ms,
        next_month, previous_month,
    },
    ui_controls::red_icon_button_variant,
    window_util,
};
use chrono::{Datelike, Duration as ChronoDuration, Local, NaiveDate};
use gpui::{
    Animation, AnimationExt, AnyElement, AppContext as _, Bounds, Context, Entity, Hsla,
    InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, Render,
    ScrollWheelEvent, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    TitlebarOptions, Window, WindowBackgroundAppearance, WindowBounds, WindowOptions, div, hsla,
    point, prelude::FluentBuilder as _, px, relative, size,
};
use gpui_component::{
    ActiveTheme, IconName, PixelsExt as _, Root, Sizable as _, button::Button,
    button::ButtonVariants as _, calendar::Date, checkbox::Checkbox,
    color_picker::ColorPickerState, date_picker::DatePicker, date_picker::DatePickerEvent,
    date_picker::DatePickerState, h_flex, input::Input, input::InputEvent, input::InputState,
    scroll::ScrollableElement as _, v_flex,
};

mod tags;
mod window;

use self::window::TodoWindow;

const WEEKDAYS: [&str; 7] = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"];
pub(crate) const TAGS_ICON_PATH: &str = "icons/tags.svg";
pub(crate) const PICTURE_IN_PICTURE_ICON_PATH: &str = "icons/picture-in-picture.svg";
pub(crate) const PIN_ICON_PATH: &str = "icons/pin.svg";
pub(crate) const PIN_OFF_ICON_PATH: &str = "icons/pin-off.svg";
pub(crate) const LOCK_ICON_PATH: &str = "icons/lock.svg";
pub(crate) const LOCK_OPEN_ICON_PATH: &str = "icons/lock-open.svg";
pub(crate) const MONITOR_STOP_ICON_PATH: &str = "icons/monitor-stop.svg";
const MAX_WEEK_LANES: usize = 8;
const CALENDAR_EMPTY_WEEK_HEIGHT: f32 = 44.0;
const CALENDAR_DAY_HEADER_HEIGHT: f32 = 22.0;
const CALENDAR_SEGMENT_TOP: f32 = 31.0;
const CALENDAR_SEGMENT_HEIGHT: f32 = 22.0;
const CALENDAR_SEGMENT_STEP: f32 = 24.0;
const CALENDAR_ROW_BOTTOM_PADDING: f32 = 4.0;

pub struct TodoPanel {
    database: TodoDatabase,
    month: NaiveDate,
    items: Vec<TodoItem>,
    tags: Vec<TodoTag>,
    day_details_date: Option<NaiveDate>,
    editor: Option<TodoEditor>,
    tag_manager_open: bool,
    editing_tag_id: Option<i64>,
    tag_color_index: usize,
    title_input: Entity<InputState>,
    details_input: Entity<InputState>,
    tag_name_input: Entity<InputState>,
    tag_color_picker: Entity<ColorPickerState>,
    subtask_input: Entity<InputState>,
    start_picker: Entity<DatePickerState>,
    due_picker: Entity<DatePickerState>,
    status: String,
    _subscriptions: Vec<Subscription>,
}

#[derive(Clone)]
struct TodoEditor {
    id: Option<i64>,
    started_date: NaiveDate,
    due_date: Option<NaiveDate>,
    completed_at_ms: Option<i64>,
    selected_tag_ids: Vec<i64>,
    subtasks: Vec<EditableSubtask>,
    error: Option<String>,
}

#[derive(Clone)]
struct EditableSubtask {
    id: Option<i64>,
    title: String,
    completed: bool,
}

#[derive(Clone)]
struct WeekTodoSegment {
    item: TodoItem,
    start_ix: usize,
    end_ix: usize,
    lane: usize,
}

impl TodoPanel {
    pub fn new(database: TodoDatabase, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let today = Local::now().date_naive();
        let title_input = cx.new(|cx| InputState::new(window, cx).placeholder("标题"));
        let details_input = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .rows(5)
                .placeholder("内容")
        });
        let tag_name_input = cx.new(|cx| InputState::new(window, cx).placeholder("标签名"));
        let tag_color_picker =
            cx.new(|cx| ColorPickerState::new(window, cx).default_value(color_from_hex("#2563eb")));
        let subtask_input = cx.new(|cx| InputState::new(window, cx).placeholder("子项"));
        let start_picker = cx.new(|cx| {
            let mut picker = DatePickerState::new(window, cx).date_format("%Y-%m-%d");
            picker.set_date(today, window, cx);
            picker
        });
        let due_picker = cx.new(|cx| DatePickerState::new(window, cx).date_format("%Y-%m-%d"));

        let subscriptions = vec![
            cx.subscribe_in(&start_picker, window, |view, _, event, _, cx| {
                let DatePickerEvent::Change(date) = event;
                if let Date::Single(Some(date)) = date {
                    if let Some(editor) = view.editor.as_mut() {
                        editor.started_date = *date;
                    }
                    cx.notify();
                }
            }),
            cx.subscribe_in(&due_picker, window, |view, _, event, _, cx| {
                let DatePickerEvent::Change(date) = event;
                if let Date::Single(date) = date {
                    if let Some(editor) = view.editor.as_mut() {
                        editor.due_date = *date;
                    }
                    cx.notify();
                }
            }),
            cx.subscribe_in(&title_input, window, |view, _, event, _, cx| {
                if matches!(event, InputEvent::Change)
                    && let Some(editor) = view.editor.as_mut()
                {
                    editor.error = None;
                    cx.notify();
                }
            }),
        ];

        let mut panel = Self {
            database,
            month: first_day_of_month(today),
            items: Vec::new(),
            tags: Vec::new(),
            day_details_date: None,
            editor: None,
            tag_manager_open: false,
            editing_tag_id: None,
            tag_color_index: 0,
            title_input,
            details_input,
            tag_name_input,
            tag_color_picker,
            subtask_input,
            start_picker,
            due_picker,
            status: String::new(),
            _subscriptions: subscriptions,
        };
        panel.reload(cx);
        panel
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let items = self.database.load_month_items(self.month);
        let tags = self.database.load_tags();
        match (items, tags) {
            (Ok(items), Ok(tags)) => {
                self.items = items;
                self.tags = tags;
                self.status.clear();
            }
            (items, tags) => {
                self.items = items.unwrap_or_default();
                self.tags = tags.unwrap_or_default();
                self.status = "读取失败".to_owned();
            }
        }
        cx.notify();
    }

    pub(crate) fn month_label(&self) -> String {
        self.month.format("%Y年%m月").to_string()
    }

    pub(crate) fn previous_month(&mut self, cx: &mut Context<Self>) {
        self.month = previous_month(self.month);
        self.reload(cx);
    }

    pub(crate) fn next_month(&mut self, cx: &mut Context<Self>) {
        self.month = next_month(self.month);
        self.reload(cx);
    }

    pub(crate) fn current_month(&mut self, cx: &mut Context<Self>) {
        self.month = first_day_of_month(Local::now().date_naive());
        self.reload(cx);
    }

    pub(crate) fn start_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let today = Local::now().date_naive();
        self.day_details_date = None;
        self.editor = Some(TodoEditor {
            id: None,
            started_date: today,
            due_date: None,
            completed_at_ms: None,
            selected_tag_ids: Vec::new(),
            subtasks: Vec::new(),
            error: None,
        });
        self.set_editor_inputs("", "", today, None, window, cx);
        cx.notify();
    }

    fn open_existing(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        self.day_details_date = None;
        match self.database.load_item(id) {
            Ok(Some(item)) => {
                let started_date = date_from_ms(item.started_at_ms);
                let due_date = item.due_at_ms.map(date_from_ms);
                self.editor = Some(TodoEditor {
                    id: Some(item.id),
                    started_date,
                    due_date,
                    completed_at_ms: item.completed_at_ms,
                    selected_tag_ids: item.tags.iter().map(|tag| tag.id).collect(),
                    subtasks: item
                        .subtasks
                        .iter()
                        .map(|subtask| EditableSubtask {
                            id: Some(subtask.id),
                            title: subtask.title.clone(),
                            completed: subtask.completed,
                        })
                        .collect(),
                    error: None,
                });
                self.set_editor_inputs(
                    &item.title,
                    &item.details,
                    started_date,
                    due_date,
                    window,
                    cx,
                );
            }
            Ok(None) => {
                self.status = "待办不存在".to_owned();
            }
            Err(error) => {
                self.status = format!("读取失败: {error}");
            }
        }
        cx.notify();
    }

    fn set_editor_inputs(
        &mut self,
        title: &str,
        details: &str,
        started_date: NaiveDate,
        due_date: Option<NaiveDate>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = title.to_owned();
        let details = details.to_owned();
        self.title_input
            .update(cx, |input, cx| input.set_value(title, window, cx));
        self.details_input
            .update(cx, |input, cx| input.set_value(details, window, cx));
        self.subtask_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.start_picker
            .update(cx, |picker, cx| picker.set_date(started_date, window, cx));
        self.due_picker.update(cx, |picker, cx| {
            picker.set_date(Date::Single(due_date), window, cx)
        });
    }

    fn close_editor(&mut self, cx: &mut Context<Self>) {
        self.editor = None;
        cx.notify();
    }

    fn add_subtask(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let title = self.subtask_input.read(cx).value().trim().to_owned();
        if title.is_empty() {
            return;
        }
        if let Some(editor) = self.editor.as_mut() {
            editor.subtasks.push(EditableSubtask {
                id: None,
                title,
                completed: false,
            });
        }
        self.subtask_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        cx.notify();
    }

    fn toggle_editor_subtask(&mut self, index: usize, completed: bool, cx: &mut Context<Self>) {
        if let Some(editor) = self.editor.as_mut()
            && let Some(subtask) = editor.subtasks.get_mut(index)
        {
            subtask.completed = completed;
        }
        cx.notify();
    }

    fn clear_due_date(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.editor.as_mut() {
            editor.due_date = None;
        }
        self.due_picker.update(cx, |picker, cx| {
            picker.set_date(Date::Single(None), window, cx)
        });
        cx.notify();
    }

    fn save_editor(&mut self, close_after_save: bool, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.clone() else {
            return;
        };
        let title = self.title_input.read(cx).value().to_string();
        if title.trim().is_empty() {
            if let Some(editor) = self.editor.as_mut() {
                editor.error = Some("标题不能为空".to_owned());
            }
            cx.notify();
            return;
        }

        let draft = TodoDraft {
            id: editor.id,
            title,
            details: self.details_input.read(cx).value().to_string(),
            tag_ids: editor.selected_tag_ids.clone(),
            started_at_ms: local_midnight_ms(editor.started_date),
            due_at_ms: editor.due_date.map(day_end_ms),
            completed_at_ms: editor.completed_at_ms,
            subtasks: editor
                .subtasks
                .iter()
                .map(|subtask| TodoSubtaskDraft {
                    id: subtask.id,
                    title: subtask.title.clone(),
                    completed: subtask.completed,
                })
                .collect(),
        };

        match self.database.save_item(&draft) {
            Ok(_) => {
                if close_after_save {
                    self.editor = None;
                }
                self.reload(cx);
            }
            Err(error) => {
                if let Some(editor) = self.editor.as_mut() {
                    editor.error = Some(format!("保存失败: {error}"));
                }
                cx.notify();
            }
        }
    }

    fn set_editor_completed(&mut self, completed: bool, cx: &mut Context<Self>) {
        if let Some(editor) = self.editor.as_mut() {
            editor.completed_at_ms = completed.then(TodoDatabase::now_ms);
        }
        self.save_editor(true, cx);
    }

    pub(crate) fn open_standalone(&mut self, cx: &mut Context<Self>) {
        let settings = self.database.todo_window_settings().unwrap_or_default();
        let window_size = size(px(settings.width as f32), px(settings.height as f32));
        let bounds = if let (Some(x), Some(y)) = (settings.x, settings.y) {
            Bounds::new(point(px(x as f32), px(y as f32)), window_size)
        } else {
            Bounds::centered(None, window_size, cx)
        };
        let database = self.database.clone();
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(
                px(TODO_WINDOW_MIN_WIDTH as f32),
                px(TODO_WINDOW_MIN_HEIGHT as f32),
            )),
            titlebar: Some(TitlebarOptions {
                title: Some(SharedString::from("I Did Today")),
                appears_transparent: true,
                ..Default::default()
            }),
            window_background: WindowBackgroundAppearance::Transparent,
            focus: true,
            show: true,
            ..Default::default()
        };

        if let Err(error) = cx.open_window(options, |window, cx| {
            window.set_window_title("I Did Today");
            if let Some(hwnd) = window_util::hwnd_from_window(window) {
                app_icon::apply_window_icons(hwnd);
                window_util::disable_maximize(hwnd);
                window_util::set_window_resize_enabled(hwnd, !settings.locked);
                window_util::set_window_opacity(hwnd, settings.opacity_percent);
            }
            let view = cx.new(|cx| TodoWindow::new(database, settings, window, cx));
            let close_view = view.clone();
            window.on_window_should_close(cx, move |window, cx| {
                close_view.update(cx, |view, _| view.prepare_close(window));
                true
            });
            cx.new(|cx| Root::new(view, window, cx))
        }) {
            self.status = format!("窗口打开失败: {error}");
            cx.notify();
        }
    }

    fn render_calendar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let first = first_day_of_month(self.month);
        let grid_start =
            first - ChronoDuration::days(first.weekday().num_days_from_sunday() as i64);
        let weeks = (0..6)
            .map(|week| self.render_week(grid_start, week, cx).into_any_element())
            .collect::<Vec<_>>();

        v_flex()
            .size_full()
            .min_h(px(0.))
            .on_scroll_wheel(cx.listener(|view, event: &ScrollWheelEvent, _, cx| {
                let delta = event.delta.pixel_delta(px(24.)).y.as_f32();
                if delta > 0.0 {
                    view.previous_month(cx);
                } else if delta < 0.0 {
                    view.next_month(cx);
                }
                cx.stop_propagation();
            }))
            .child(
                h_flex()
                    .h(px(34.))
                    .flex_none()
                    .border_b_1()
                    .border_color(cx.theme().border.opacity(0.45))
                    .children(WEEKDAYS.iter().map(|day| {
                        div()
                            .flex_1()
                            .h_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_sm()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(cx.theme().primary)
                            .child(*day)
                    })),
            )
            .children(weeks)
    }

    fn render_week(
        &self,
        grid_start: NaiveDate,
        week: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let week_start = grid_start + ChronoDuration::days((week * 7) as i64);
        let segments = self.week_todo_segments(week_start);
        let lane_count = segments
            .iter()
            .map(|segment| segment.lane + 1)
            .max()
            .unwrap_or_default();
        let has_week_overflow = self.week_has_overflow(week_start);
        let row_height = calendar_week_height(lane_count, has_week_overflow);
        let cells = (0..7)
            .map(|day| {
                let date = week_start + ChronoDuration::days(day as i64);
                self.render_day_cell(date, cx)
            })
            .collect::<Vec<_>>();
        let segments = segments
            .into_iter()
            .map(|segment| self.render_calendar_segment(segment, cx))
            .collect::<Vec<_>>();
        let overflow_markers = (0..7)
            .filter_map(|day| {
                let date = week_start + ChronoDuration::days(day as i64);
                let overflow_count = self.day_overflow_segments(date).len();
                (overflow_count > 0)
                    .then(|| self.render_day_overflow_more(date, day, overflow_count, cx))
            })
            .collect::<Vec<_>>();

        div()
            .relative()
            .flex_auto()
            .flex_basis(px(row_height))
            .min_h(px(row_height))
            .border_b_1()
            .border_color(cx.theme().border.opacity(0.28))
            .child(h_flex().size_full().children(cells))
            .children(segments)
            .children(overflow_markers)
    }

    fn render_day_cell(&self, date: NaiveDate, cx: &mut Context<Self>) -> AnyElement {
        let today = Local::now().date_naive();
        let in_month = date.month() == self.month.month();
        let overflow_segments = self.day_overflow_segments(date);
        let overflow_count = overflow_segments.len();
        let due_count = self
            .items
            .iter()
            .filter(|item| item.due_at_ms.map(date_from_ms) == Some(date) && !item.is_completed())
            .count();

        v_flex()
            .relative()
            .flex_1()
            .h_full()
            .min_w(px(0.))
            .border_r_1()
            .border_color(cx.theme().border.opacity(0.28))
            .bg(if date == today {
                cx.theme().primary.opacity(0.07)
            } else {
                cx.theme().background
            })
            .p_1()
            .gap_1()
            .overflow_hidden()
            .child(
                h_flex()
                    .h(px(CALENDAR_DAY_HEADER_HEIGHT))
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .child(div().flex_1().min_w(px(0.)).h_full().flex().items_center())
                    .child(
                        div()
                            .h_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .px_1()
                            .text_sm()
                            .text_center()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(if in_month {
                                cx.theme().foreground
                            } else {
                                cx.theme().muted_foreground.opacity(0.55)
                            })
                            .child(format!("{}日", date.day())),
                    )
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w(px(0.))
                            .h_full()
                            .justify_end()
                            .gap_1()
                            .items_center()
                            .when(overflow_count > 0, |this| {
                                this.child(
                                    div()
                                        .px_1()
                                        .rounded(px(4.))
                                        .text_xs()
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .bg(cx.theme().primary.opacity(0.12))
                                        .text_color(cx.theme().primary)
                                        .child(format!("+{overflow_count}")),
                                )
                            })
                            .when(due_count > 0, |this| {
                                this.child(
                                    div()
                                        .px_1()
                                        .rounded(px(4.))
                                        .text_xs()
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .bg(cx.theme().warning.opacity(0.14))
                                        .text_color(cx.theme().warning)
                                        .child(format!("D{due_count}")),
                                )
                            }),
                    ),
            )
            .child(div().flex_1().min_h(px(0.)))
            .into_any_element()
    }

    fn week_todo_segments(&self, week_start: NaiveDate) -> Vec<WeekTodoSegment> {
        self.week_todo_segments_with_limit(week_start, Some(MAX_WEEK_LANES))
    }

    fn week_todo_segments_with_limit(
        &self,
        week_start: NaiveDate,
        lane_limit: Option<usize>,
    ) -> Vec<WeekTodoSegment> {
        let now_ms = TodoDatabase::now_ms();
        let week_end = week_start + ChronoDuration::days(6);
        let mut spans = self
            .items
            .iter()
            .filter_map(|item| {
                let (start, end) = todo_date_span(item, now_ms);
                if end < week_start || start > week_end {
                    return None;
                }
                let clipped_start = start.max(week_start);
                let clipped_end = end.min(week_end);
                let start_ix = (clipped_start - week_start).num_days().max(0) as usize;
                let end_ix = (clipped_end - week_start).num_days().clamp(0, 6) as usize;
                Some((item.clone(), start_ix, end_ix))
            })
            .collect::<Vec<_>>();

        spans.sort_by(|a, b| {
            a.1.cmp(&b.1)
                .then_with(|| b.2.saturating_sub(b.1).cmp(&a.2.saturating_sub(a.1)))
                .then_with(|| a.0.id.cmp(&b.0.id))
        });

        let mut lane_ends = Vec::<usize>::new();
        let mut segments = Vec::new();
        for (item, start_ix, end_ix) in spans {
            let lane = lane_ends
                .iter()
                .position(|last_end| *last_end < start_ix)
                .unwrap_or(lane_ends.len());
            if lane_limit.is_some_and(|limit| lane >= limit) {
                continue;
            }
            if lane == lane_ends.len() {
                lane_ends.push(end_ix);
            } else {
                lane_ends[lane] = end_ix;
            }
            segments.push(WeekTodoSegment {
                item,
                start_ix,
                end_ix,
                lane,
            });
        }
        segments
    }

    fn day_overflow_segments(&self, date: NaiveDate) -> Vec<WeekTodoSegment> {
        let week_start = date - ChronoDuration::days(date.weekday().num_days_from_sunday() as i64);
        let day_ix = (date - week_start).num_days().clamp(0, 6) as usize;
        self.week_todo_segments_with_limit(week_start, None)
            .into_iter()
            .filter(|segment| {
                segment.lane >= MAX_WEEK_LANES
                    && segment.start_ix <= day_ix
                    && segment.end_ix >= day_ix
            })
            .collect()
    }

    fn week_has_overflow(&self, week_start: NaiveDate) -> bool {
        self.week_todo_segments_with_limit(week_start, None)
            .into_iter()
            .any(|segment| segment.lane >= MAX_WEEK_LANES)
    }

    fn open_day_details(&mut self, date: NaiveDate, cx: &mut Context<Self>) {
        self.day_details_date = Some(date);
        cx.notify();
    }

    fn close_day_details(&mut self, cx: &mut Context<Self>) {
        self.day_details_date = None;
        cx.notify();
    }

    fn items_for_date(&self, date: NaiveDate) -> Vec<TodoItem> {
        let now_ms = TodoDatabase::now_ms();
        let mut items = self
            .items
            .iter()
            .filter(|item| {
                let (start, end) = todo_date_span(item, now_ms);
                (start <= date && date <= end) || item.due_at_ms.map(date_from_ms) == Some(date)
            })
            .cloned()
            .collect::<Vec<_>>();

        items.sort_by(|a, b| {
            let (a_start, a_end) = todo_date_span(a, now_ms);
            let (b_start, b_end) = todo_date_span(b, now_ms);
            a_start
                .cmp(&b_start)
                .then_with(|| a_end.cmp(&b_end))
                .then_with(|| a.id.cmp(&b.id))
        });
        items
    }

    fn render_day_overflow_more(
        &self,
        date: NaiveDate,
        day_ix: usize,
        overflow_count: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let left = day_ix as f32 / 7.0;

        Button::new(SharedString::from(format!("todo-day-overflow-more-{date}")))
            .ghost()
            .compact()
            .absolute()
            .top(px(
                CALENDAR_SEGMENT_TOP + MAX_WEEK_LANES as f32 * CALENDAR_SEGMENT_STEP
            ))
            .left(relative(left))
            .w(relative(1.0 / 7.0))
            .h(px(CALENDAR_SEGMENT_HEIGHT))
            .px_2()
            .flex()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .rounded(px(5.))
            .border_1()
            .border_color(cx.theme().border.opacity(0.48))
            .bg(cx.theme().muted.opacity(0.62))
            .text_color(cx.theme().muted_foreground)
            .text_xs()
            .font_weight(gpui::FontWeight::BOLD)
            .tooltip(format!("{}项", overflow_count))
            .on_click(cx.listener(move |view, _, _, cx| view.open_day_details(date, cx)))
            .child("...")
            .into_any_element()
    }

    fn render_calendar_segment(
        &self,
        segment: WeekTodoSegment,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let overdue = is_overdue(&segment.item);
        let accent = todo_item_accent(&segment.item, cx);
        let id = segment.item.id;
        let left = segment.start_ix as f32 / 7.0;
        let width = (segment.end_ix.saturating_sub(segment.start_ix) + 1) as f32 / 7.0;

        Button::new(SharedString::from(format!(
            "todo-segment-{}-{}-{}",
            segment.item.id, segment.start_ix, segment.lane
        )))
        .ghost()
        .compact()
        .absolute()
        .top(px(31.0 + segment.lane as f32 * 24.0))
        .left(relative(left))
        .w(relative(width))
        .h(px(22.))
        .mx(px(0.))
        .px_2()
        .flex()
        .items_center()
        .overflow_hidden()
        .border_1()
        .border_color(if overdue {
            cx.theme().warning.opacity(0.72)
        } else {
            accent.opacity(0.42)
        })
        .bg(if segment.item.is_completed() {
            cx.theme().muted.opacity(0.58)
        } else {
            accent.opacity(0.16)
        })
        .text_color(if segment.item.is_completed() {
            cx.theme().muted_foreground
        } else {
            accent
        })
        .text_xs()
        .font_weight(gpui::FontWeight::BOLD)
        .rounded(px(6.))
        .hover(|style| style.bg(accent.opacity(0.24)))
        .on_click(cx.listener(move |view, _, window, cx| {
            view.open_existing(id, window, cx);
        }))
        .child(
            div()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .child(segment.item.title.clone()),
        )
        .with_animation(
            SharedString::from(format!(
                "todo-segment-anim-{}-{}-{}",
                segment.item.id, segment.start_ix, segment.lane
            )),
            Animation::new(Duration::from_millis(220)).with_easing(gpui::ease_out_quint()),
            |this, delta| this.opacity((0.54 + delta * 0.46).min(1.0)),
        )
        .into_any_element()
    }

    fn render_editor_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.editor
            .as_ref()
            .map(|editor| self.render_editor(editor, cx).into_any_element())
    }

    fn render_day_details_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let date = self.day_details_date?;
        let rows = self
            .items_for_date(date)
            .into_iter()
            .map(|item| self.render_day_detail_item(item, cx))
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
                        .w(px(560.))
                        .h(px(520.))
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
                                        .child(format!("{}月{}日", date.month(), date.day())),
                                )
                                .child(
                                    Button::new("todo-day-details-close")
                                        .custom(red_icon_button_variant(cx))
                                        .compact()
                                        .small()
                                        .rounded(px(7.))
                                        .icon(IconName::Close)
                                        .tooltip("关闭")
                                        .on_click(
                                            cx.listener(|view, _, _, cx| {
                                                view.close_day_details(cx)
                                            }),
                                        ),
                                ),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_h(px(0.))
                                .gap_2()
                                .p_4()
                                .overflow_y_scrollbar()
                                .children(rows),
                        ),
                )
                .with_animation(
                    "todo-day-details-overlay",
                    Animation::new(Duration::from_millis(180)).with_easing(gpui::ease_out_quint()),
                    |this, delta| this.opacity((0.28 + delta * 0.72).min(1.0)),
                )
                .into_any_element(),
        )
    }

    fn render_day_detail_item(&self, item: TodoItem, cx: &mut Context<Self>) -> AnyElement {
        let id = item.id;
        let accent = todo_item_accent(&item, cx);
        let due = item.due_at_ms.map(format_date_ms);
        let (done, total) = item.subtask_counts();
        let metadata = h_flex()
            .gap_2()
            .items_center()
            .flex_wrap()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(format_duration(
                item.started_at_ms,
                item.effective_end_ms(TodoDatabase::now_ms()),
            ))
            .when(total > 0, |this| this.child(format!("{done}/{total}")))
            .when_some(due, |this, due| this.child(due));

        h_flex()
            .id(SharedString::from(format!("todo-day-detail-row-{id}")))
            .w_full()
            .min_h(px(66.))
            .items_start()
            .gap_3()
            .rounded(px(8.))
            .border_1()
            .border_color(cx.theme().border.opacity(0.48))
            .bg(cx.theme().muted.opacity(0.24))
            .px_3()
            .py_2()
            .cursor_pointer()
            .hover(|style| style.bg(accent.opacity(0.12)))
            .on_click(cx.listener(move |view, _, window, cx| view.open_existing(id, window, cx)))
            .child(
                div()
                    .mt(px(5.))
                    .size_3()
                    .rounded_full()
                    .bg(accent)
                    .flex_none(),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(0.))
                    .gap_1p5()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::BOLD)
                            .line_height(px(18.))
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(item.title.clone()),
                    )
                    .child(metadata)
                    .when(!item.tags.is_empty(), |this| {
                        this.child(h_flex().gap_1p5().flex_wrap().children(
                            item.tags.iter().take(4).map(|tag| {
                                let color = color_from_hex(&tag.color);
                                div()
                                    .rounded(px(4.))
                                    .bg(color.opacity(0.12))
                                    .text_color(color)
                                    .text_xs()
                                    .px_1p5()
                                    .py_0p5()
                                    .child(tag.name.clone())
                            }),
                        ))
                    }),
            )
            .into_any_element()
    }

    fn render_editor(&self, editor: &TodoEditor, cx: &mut Context<Self>) -> impl IntoElement {
        let completed = editor.completed_at_ms.is_some();
        let title = if editor.id.is_some() {
            "编辑待办"
        } else {
            "新建待办"
        };

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
                                    .child(title),
                            )
                            .child(
                                Button::new("todo-editor-close")
                                    .custom(red_icon_button_variant(cx))
                                    .compact()
                                    .small()
                                    .rounded(px(7.))
                                    .icon(IconName::Close)
                                    .tooltip("关闭")
                                    .on_click(cx.listener(|view, _, _, cx| view.close_editor(cx))),
                            ),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_h(px(0.))
                            .overflow_y_scrollbar()
                            .gap_3()
                            .p_4()
                            .child(form_row("标题", Input::new(&self.title_input).small(), cx))
                            .child(form_row(
                                "内容",
                                Input::new(&self.details_input).h(px(138.)).small(),
                                cx,
                            ))
                            .child(self.render_editor_tags(editor, cx))
                            .child(
                                h_flex()
                                    .gap_3()
                                    .items_start()
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .gap_2()
                                            .child(form_label("开始", cx))
                                            .child(
                                                DatePicker::new(&self.start_picker)
                                                    .small()
                                                    .number_of_months(1),
                                            ),
                                    )
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .gap_2()
                                            .child(form_label("截止", cx))
                                            .child(
                                                h_flex()
                                                    .gap_2()
                                                    .child(
                                                        div().flex_1().child(
                                                            DatePicker::new(&self.due_picker)
                                                                .small()
                                                                .number_of_months(1),
                                                        ),
                                                    )
                                                    .child(
                                                        Button::new("todo-clear-due")
                                                            .ghost()
                                                            .small()
                                                            .compact()
                                                            .rounded(px(7.))
                                                            .icon(IconName::Close)
                                                            .tooltip("清除截止")
                                                            .on_click(cx.listener(
                                                                |view, _, window, cx| {
                                                                    view.clear_due_date(window, cx)
                                                                },
                                                            )),
                                                    ),
                                            ),
                                    ),
                            )
                            .child(self.render_editor_subtasks(editor, cx))
                            .when_some(editor.error.clone(), |this, error| {
                                this.child(
                                    div()
                                        .rounded(px(6.))
                                        .border_1()
                                        .border_color(cx.theme().red.opacity(0.45))
                                        .bg(cx.theme().red.opacity(0.08))
                                        .text_color(cx.theme().red)
                                        .text_sm()
                                        .px_3()
                                        .py_2()
                                        .child(error),
                                )
                            }),
                    )
                    .child(
                        h_flex()
                            .h(px(58.))
                            .flex_none()
                            .items_center()
                            .justify_between()
                            .px_4()
                            .border_t_1()
                            .border_color(cx.theme().border.opacity(0.45))
                            .child(
                                Button::new("todo-editor-complete")
                                    .small()
                                    .rounded(px(7.))
                                    .icon(if completed {
                                        IconName::Undo2
                                    } else {
                                        IconName::CircleCheck
                                    })
                                    .label(if completed { "重新打开" } else { "完成" })
                                    .on_click(cx.listener(move |view, _, _, cx| {
                                        view.set_editor_completed(!completed, cx)
                                    })),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        Button::new("todo-editor-cancel")
                                            .ghost()
                                            .small()
                                            .rounded(px(7.))
                                            .label("取消")
                                            .on_click(
                                                cx.listener(|view, _, _, cx| view.close_editor(cx)),
                                            ),
                                    )
                                    .child(
                                        Button::new("todo-editor-save")
                                            .small()
                                            .rounded(px(7.))
                                            .icon(IconName::Check)
                                            .label("保存")
                                            .on_click(cx.listener(|view, _, _, cx| {
                                                view.save_editor(true, cx)
                                            })),
                                    ),
                            ),
                    ),
            )
            .with_animation(
                "todo-editor-overlay",
                Animation::new(Duration::from_millis(180)).with_easing(gpui::ease_out_quint()),
                |this, delta| this.opacity((0.28 + delta * 0.72).min(1.0)),
            )
    }

    fn render_editor_subtasks(
        &self,
        editor: &TodoEditor,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let rows = editor
            .subtasks
            .iter()
            .enumerate()
            .map(|(ix, subtask)| {
                let title = subtask.title.clone();
                h_flex()
                    .w_full()
                    .items_center()
                    .gap_2()
                    .rounded(px(7.))
                    .border_1()
                    .border_color(cx.theme().border.opacity(0.48))
                    .bg(cx.theme().muted.opacity(0.26))
                    .px_3()
                    .py_2()
                    .child(
                        Checkbox::new(("editor-subtask", ix))
                            .checked(subtask.completed)
                            .on_click(cx.listener(move |view, checked, _, cx| {
                                view.toggle_editor_subtask(ix, *checked, cx)
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_sm()
                            .when(subtask.completed, |this| {
                                this.text_color(cx.theme().muted_foreground)
                            })
                            .child(title),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>();

        v_flex()
            .gap_2()
            .child(form_label("子项", cx))
            .children(rows)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .child(Input::new(&self.subtask_input).small()),
                    )
                    .child(
                        Button::new("todo-add-subtask")
                            .small()
                            .compact()
                            .rounded(px(7.))
                            .icon(IconName::Plus)
                            .tooltip("添加子项")
                            .on_click(
                                cx.listener(|view, _, window, cx| view.add_subtask(window, cx)),
                            ),
                    ),
            )
    }
}

impl Render for TodoPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .relative()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                v_flex()
                    .size_full()
                    .child(div().flex_1().min_h(px(0.)).child(self.render_calendar(cx)))
                    .when(!self.status.is_empty(), |this| {
                        this.child(
                            div()
                                .absolute()
                                .left_3()
                                .bottom_3()
                                .rounded(px(7.))
                                .bg(cx.theme().background)
                                .border_1()
                                .border_color(cx.theme().border.opacity(0.5))
                                .text_sm()
                                .px_3()
                                .py_2()
                                .child(self.status.clone()),
                        )
                    }),
            )
            .children(self.render_editor_overlay(cx))
            .children(self.render_day_details_overlay(cx))
            .children(self.render_tag_manager_overlay(cx))
    }
}

fn form_label(label: &'static str, cx: &mut Context<TodoPanel>) -> impl IntoElement {
    div()
        .text_sm()
        .font_weight(gpui::FontWeight::BOLD)
        .text_color(cx.theme().muted_foreground)
        .child(label)
}

fn form_row(
    label: &'static str,
    field: impl IntoElement,
    cx: &mut Context<TodoPanel>,
) -> impl IntoElement {
    v_flex().gap_2().child(form_label(label, cx)).child(field)
}

fn todo_date_span(item: &TodoItem, now_ms: i64) -> (NaiveDate, NaiveDate) {
    let start = date_from_ms(item.started_at_ms);
    let end = date_from_ms(item.effective_end_ms(now_ms));
    if end < start {
        (start, start)
    } else {
        (start, end)
    }
}

fn calendar_segment_bottom(lane_count: usize) -> f32 {
    if lane_count == 0 {
        0.0
    } else {
        CALENDAR_SEGMENT_TOP
            + (lane_count.saturating_sub(1) as f32 * CALENDAR_SEGMENT_STEP)
            + CALENDAR_SEGMENT_HEIGHT
    }
}

fn calendar_week_height(lane_count: usize, has_overflow: bool) -> f32 {
    let visible_lane_count = if has_overflow {
        lane_count.max(MAX_WEEK_LANES + 1)
    } else {
        lane_count
    };
    calendar_segment_bottom(visible_lane_count)
        .max(CALENDAR_EMPTY_WEEK_HEIGHT)
        .ceil()
        + CALENDAR_ROW_BOTTOM_PADDING
}

fn is_overdue(item: &TodoItem) -> bool {
    let Some(due_at_ms) = item.due_at_ms else {
        return false;
    };
    item.completed_at_ms.is_none() && due_at_ms < local_midnight_ms(Local::now().date_naive())
}

fn todo_accent<T>(id: i64, cx: &mut Context<T>) -> Hsla {
    let palette = [
        cx.theme().blue,
        cx.theme().green,
        cx.theme().cyan,
        cx.theme().magenta,
        cx.theme().yellow,
    ];
    palette[id.unsigned_abs() as usize % palette.len()]
}

fn todo_item_accent<T>(item: &TodoItem, cx: &mut Context<T>) -> Hsla {
    item.tags
        .first()
        .map(|tag| color_from_hex(&tag.color))
        .unwrap_or_else(|| todo_accent(item.id, cx))
}

fn color_from_hex(value: &str) -> Hsla {
    let value = value.trim().trim_start_matches('#');
    if value.len() != 6 {
        return hsla(215.0 / 360.0, 0.82, 0.52, 1.0);
    }

    let Ok(rgb) = u32::from_str_radix(value, 16) else {
        return hsla(215.0 / 360.0, 0.82, 0.52, 1.0);
    };
    let r = ((rgb >> 16) & 0xff) as f32 / 255.0;
    let g = ((rgb >> 8) & 0xff) as f32 / 255.0;
    let b = (rgb & 0xff) as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let lightness = (max + min) / 2.0;
    let delta = max - min;
    if delta <= f32::EPSILON {
        return hsla(0.0, 0.0, lightness, 1.0);
    }

    let saturation = if lightness > 0.5 {
        delta / (2.0 - max - min)
    } else {
        delta / (max + min)
    };
    let hue = if (max - r).abs() <= f32::EPSILON {
        ((g - b) / delta + if g < b { 6.0 } else { 0.0 }) / 6.0
    } else if (max - g).abs() <= f32::EPSILON {
        ((b - r) / delta + 2.0) / 6.0
    } else {
        ((r - g) / delta + 4.0) / 6.0
    };
    hsla(hue, saturation, lightness, 1.0)
}

fn format_duration(start_ms: i64, end_ms: i64) -> String {
    let total_seconds = end_ms.saturating_sub(start_ms).max(0) / 1_000;
    let days = total_seconds / 86_400;
    let hours = (total_seconds % 86_400) / 3_600;
    let minutes = (total_seconds % 3_600) / 60;

    if days > 0 {
        format!("{days}天{hours}小时")
    } else if hours > 0 {
        format!("{hours}小时{minutes}分")
    } else {
        format!("{minutes}分")
    }
}

fn format_date_ms(timestamp_ms: i64) -> String {
    date_from_ms(timestamp_ms).format("%m-%d").to_string()
}
