use std::collections::HashMap;

use chrono::{Datelike, Duration as ChronoDuration, Local, NaiveDate, TimeZone};
use gpui::{Context, Hsla, ParentElement as _, Pixels, SharedString, Styled as _, div};
use gpui_component::ActiveTheme;

use crate::db::{AppTotal, CloseBehavior, DashboardBucket, DashboardData};

use super::{AppAreaPoint, AppGroup, ChartBucket, DAY_MS, Dashboard, TimeFilter, TimeRange};

pub(super) fn timeline_head_cell(label: &'static str, width: Pixels) -> gpui::Div {
    div()
        .w(width)
        .h_full()
        .px_3()
        .flex()
        .items_center()
        .font_weight(gpui::FontWeight::BOLD)
        .child(label)
}

pub(super) fn close_behavior_index(behavior: CloseBehavior) -> usize {
    match behavior {
        CloseBehavior::Minimize => 0,
        CloseBehavior::HideToTray => 1,
        CloseBehavior::Exit => 2,
    }
}

impl TimeFilter {
    pub(super) fn range(self, custom_range: (NaiveDate, NaiveDate)) -> TimeRange {
        let now = Local::now();
        let today = now.date_naive();

        match self {
            Self::Last24Hours => {
                let end_ms = now.timestamp_millis();
                let start_ms = end_ms.saturating_sub(DAY_MS);
                TimeRange { start_ms, end_ms }
            }
            Self::Today => {
                let (start_ms, end_ms) = day_bounds(today);
                TimeRange { start_ms, end_ms }
            }
            Self::ThisWeek => {
                let start =
                    today - ChronoDuration::days(today.weekday().num_days_from_monday() as i64);
                let end = start + ChronoDuration::days(7);
                TimeRange {
                    start_ms: local_midnight_ms(start),
                    end_ms: local_midnight_ms(end),
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
                }
            }
            Self::Custom => {
                let (start, end) = ordered_dates(custom_range.0, custom_range.1);
                let range_end = end + ChronoDuration::days(1);
                TimeRange {
                    start_ms: local_midnight_ms(start),
                    end_ms: local_midnight_ms(range_end),
                }
            }
        }
    }

    pub(super) fn date_range_for_picker(
        self,
        custom_range: (NaiveDate, NaiveDate),
    ) -> (NaiveDate, NaiveDate) {
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

pub(super) fn ordered_dates(start: NaiveDate, end: NaiveDate) -> (NaiveDate, NaiveDate) {
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

pub(super) fn local_midnight_ms(date: NaiveDate) -> i64 {
    let start_naive = date
        .and_hms_opt(0, 0, 0)
        .expect("midnight should always be valid");
    Local
        .from_local_datetime(&start_naive)
        .earliest()
        .expect("local midnight should resolve")
        .timestamp_millis()
}

pub(super) fn group_app_totals(
    app_totals: &[AppTotal],
    total_ms: u64,
    display_count: usize,
) -> Vec<AppGroup> {
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

pub(super) fn app_area_points(
    data: &DashboardData,
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
    for total in &data.bucket_totals {
        let Some(group_ix) = group_lookup.get(&total.process_name).copied() else {
            continue;
        };
        if let Some(bucket) = values.get_mut(total.bucket_ix) {
            bucket[group_ix] += total.duration_ms as f64 / 60_000.0;
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

pub(super) fn dashboard_buckets(time_range: &TimeRange) -> Vec<DashboardBucket> {
    chart_buckets(time_range)
        .into_iter()
        .map(|bucket| DashboardBucket {
            start_ms: bucket.start_ms,
            end_ms: bucket.end_ms,
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

pub(super) fn empty_dashboard(interval_ms: u64) -> DashboardData {
    DashboardData {
        total_ms: 0,
        interval_ms,
        record_count: 0,
        app_totals: Vec::new(),
        bucket_totals: Vec::new(),
    }
}

pub(super) fn format_interval(interval_ms: u64) -> String {
    if interval_ms % 1_000 == 0 {
        format!("{}s", interval_ms / 1_000)
    } else if interval_ms % 100 == 0 {
        format!("{:.1}s", interval_ms as f64 / 1_000.0)
    } else {
        format!("{interval_ms}ms")
    }
}

pub(super) fn process_initial(process_name: &str) -> String {
    process_name
        .trim()
        .chars()
        .find(|ch| ch.is_alphanumeric())
        .map(|ch| ch.to_uppercase().collect())
        .unwrap_or_else(|| "?".to_owned())
}

pub(super) fn format_duration(duration_ms: u64) -> String {
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

pub(super) fn process_accent(process_name: &str, cx: &mut Context<Dashboard>) -> Hsla {
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

pub(super) fn chart_color(ix: usize, cx: &mut Context<Dashboard>) -> Hsla {
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

pub(super) fn date_clock(timestamp_ms: i64, show_date: bool) -> String {
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
