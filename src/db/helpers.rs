use std::collections::BTreeMap;

use anyhow::Result;
use chrono::{Datelike, Local, NaiveDate, TimeZone};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use super::{
    ALLOWED_CACHE_FLUSH_INTERVAL_MS, ALLOWED_INTERVAL_MS, AppTotalAccumulator,
    DEFAULT_CACHE_FLUSH_INTERVAL_MS, DEFAULT_INTERVAL_MS, DashboardBucket, FocusInfo, LastSession,
    MAX_INTERVAL_MS, SessionRecord, TimelineFilter, TimelineItem,
};

pub(super) fn load_archive_candidates(
    conn: &Connection,
    cutoff_ms: i64,
) -> Result<Vec<SessionRecord>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT
            id,
            started_at_ms,
            ended_at_ms,
            duration_ms,
            process_id,
            process_name,
            exe_path,
            window_class,
            window_title
        FROM activity_sessions
        WHERE ended_at_ms < ?1
        ORDER BY started_at_ms ASC, id ASC
        "#,
    )?;

    let rows = stmt.query_map(params![cutoff_ms], |row| {
        Ok(SessionRecord {
            started_at_ms: row.get(1)?,
            ended_at_ms: row.get(2)?,
            duration_ms: row.get::<_, i64>(3)?.max(0) as u64,
            process_id: row.get(4)?,
            process_name: row.get(5)?,
            exe_path: row.get(6)?,
            window_class: row.get(7)?,
            window_title: row.get(8)?,
        })
    })?;

    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub(super) fn load_archive_ids(tx: &Transaction<'_>, cutoff_ms: i64) -> Result<Vec<i64>> {
    let mut stmt = tx.prepare(
        r#"
        SELECT id
        FROM activity_sessions
        WHERE ended_at_ms < ?1
        ORDER BY id ASC
        "#,
    )?;
    let rows = stmt.query_map(params![cutoff_ms], |row| row.get::<_, i64>(0))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub(super) fn append_session_record(tx: &Transaction<'_>, session: &SessionRecord) -> Result<()> {
    let last = tx
        .query_row(
            r#"
            SELECT id, ended_at_ms, process_name, exe_path, window_class, window_title
            FROM activity_sessions
            ORDER BY ended_at_ms DESC
            LIMIT 1
            "#,
            [],
            |row| {
                Ok(LastSession {
                    id: row.get(0)?,
                    ended_at_ms: row.get(1)?,
                    process_name: row.get(2)?,
                    exe_path: row.get(3)?,
                    window_class: row.get(4)?,
                    window_title: row.get(5)?,
                })
            },
        )
        .optional()?;

    let can_extend = last.as_ref().is_some_and(|last| {
        same_session_record(last, session)
            && session.started_at_ms <= last.ended_at_ms.saturating_add(MAX_INTERVAL_MS as i64)
    });

    if let Some(last) = last.filter(|_| can_extend) {
        tx.execute(
            r#"
            UPDATE activity_sessions
            SET ended_at_ms = ?1,
                duration_ms = MAX(0, ?1 - started_at_ms),
                process_id = ?2
            WHERE id = ?3
            "#,
            params![session.ended_at_ms, session.process_id, last.id],
        )?;
        return Ok(());
    }

    insert_session_record(tx, session)
}

pub(super) fn same_focus_info(session: &SessionRecord, info: &FocusInfo) -> bool {
    session.process_name == info.process_name
        && session.exe_path == info.exe_path
        && session.window_class == info.window_class
        && session.window_title == info.window_title
}

fn same_session_record(last: &LastSession, session: &SessionRecord) -> bool {
    last.process_name == session.process_name
        && last.exe_path == session.exe_path
        && last.window_class == session.window_class
        && last.window_title == session.window_title
}

pub(super) fn merge_cached_dashboard_data(
    sessions: &[SessionRecord],
    range_start_ms: i64,
    range_end_ms: i64,
    buckets: &[DashboardBucket],
    total_ms: &mut u64,
    record_count: &mut usize,
    app_accumulators: &mut BTreeMap<String, AppTotalAccumulator>,
    bucket_accumulators: &mut BTreeMap<(usize, String), u64>,
) {
    for session in sessions {
        let duration_ms = clipped_session_duration_ms(session, range_start_ms, range_end_ms);
        if duration_ms == 0 {
            continue;
        }

        *total_ms = total_ms.saturating_add(duration_ms);
        *record_count = record_count.saturating_add(1);

        let entry = app_accumulators
            .entry(session.process_name.clone())
            .or_default();
        entry.duration_ms = entry.duration_ms.saturating_add(duration_ms);
        if duration_ms > entry.icon_exe_duration_ms {
            entry.icon_exe_path = session.exe_path.clone();
            entry.icon_exe_duration_ms = duration_ms;
        }

        for (bucket_ix, bucket) in buckets.iter().enumerate() {
            let bucket_duration =
                clipped_session_duration_ms(session, bucket.start_ms, bucket.end_ms);
            if bucket_duration > 0 {
                let entry = bucket_accumulators
                    .entry((bucket_ix, session.process_name.clone()))
                    .or_default();
                *entry = entry.saturating_add(bucket_duration);
            }
        }
    }
}

pub(super) fn cached_timeline_count(
    sessions: &[SessionRecord],
    range_start_ms: i64,
    range_end_ms: i64,
    filter: &TimelineFilter,
) -> usize {
    sessions
        .iter()
        .filter(|session| {
            clipped_session_duration_ms(session, range_start_ms, range_end_ms) > 0
                && session_matches_filter(session, filter)
        })
        .count()
}

pub(super) fn cached_timeline_items(
    sessions: &[SessionRecord],
    range_start_ms: i64,
    range_end_ms: i64,
    filter: &TimelineFilter,
) -> Vec<TimelineItem> {
    sessions
        .iter()
        .filter(|session| {
            clipped_session_duration_ms(session, range_start_ms, range_end_ms) > 0
                && session_matches_filter(session, filter)
        })
        .map(|session| timeline_item_from_session(session, range_start_ms, range_end_ms))
        .collect()
}

pub(super) fn first_cached_timeline_item(
    sessions: &[SessionRecord],
    range_start_ms: i64,
    range_end_ms: i64,
    filter: &TimelineFilter,
) -> Option<TimelineItem> {
    let session = sessions.first()?;
    if clipped_session_duration_ms(session, range_start_ms, range_end_ms) > 0
        && session_matches_filter(session, filter)
    {
        Some(timeline_item_from_session(
            session,
            range_start_ms,
            range_end_ms,
        ))
    } else {
        None
    }
}

fn session_matches_filter(session: &SessionRecord, filter: &TimelineFilter) -> bool {
    if let Some(process_filter) = filter.process_contains.as_ref() {
        if !session.process_name.to_lowercase().contains(process_filter) {
            return false;
        }
    }

    if let Some(title_filter) = filter.title_contains.as_ref() {
        if !session.window_title.to_lowercase().contains(title_filter) {
            return false;
        }
    }

    true
}

fn clipped_session_duration_ms(
    session: &SessionRecord,
    range_start_ms: i64,
    range_end_ms: i64,
) -> u64 {
    if session.ended_at_ms <= range_start_ms || session.started_at_ms >= range_end_ms {
        return 0;
    }

    session
        .ended_at_ms
        .min(range_end_ms)
        .saturating_sub(session.started_at_ms.max(range_start_ms))
        .max(0) as u64
}

fn timeline_item_from_session(
    session: &SessionRecord,
    range_start_ms: i64,
    range_end_ms: i64,
) -> TimelineItem {
    let clipped_start_ms = session.started_at_ms.max(range_start_ms);
    let clipped_end_ms = session.ended_at_ms.min(range_end_ms);
    TimelineItem {
        started_at_ms: clipped_start_ms,
        ended_at_ms: clipped_end_ms,
        duration_ms: clipped_end_ms.saturating_sub(clipped_start_ms).max(0) as u64,
        process_name: session.process_name.clone(),
        exe_path: session.exe_path.clone(),
        icon_png: None,
        window_title: session.window_title.clone(),
        window_class: session.window_class.clone(),
    }
}

fn sort_timeline_items(items: &mut [TimelineItem]) {
    items.sort_by(|a, b| {
        b.started_at_ms
            .cmp(&a.started_at_ms)
            .then_with(|| b.ended_at_ms.cmp(&a.ended_at_ms))
            .then_with(|| b.duration_ms.cmp(&a.duration_ms))
            .then_with(|| a.process_name.cmp(&b.process_name))
    });
}

pub(super) fn merge_contiguous_timeline_items(mut items: Vec<TimelineItem>) -> Vec<TimelineItem> {
    if items.len() <= 1 {
        sort_timeline_items(&mut items);
        return items;
    }

    items.sort_by(|a, b| {
        a.started_at_ms
            .cmp(&b.started_at_ms)
            .then_with(|| a.ended_at_ms.cmp(&b.ended_at_ms))
            .then_with(|| a.process_name.cmp(&b.process_name))
    });

    let mut merged = Vec::<TimelineItem>::with_capacity(items.len());
    for item in items {
        if let Some(last) = merged.last_mut()
            && same_timeline_item(last, &item)
            && item.started_at_ms <= last.ended_at_ms
        {
            last.started_at_ms = last.started_at_ms.min(item.started_at_ms);
            last.ended_at_ms = last.ended_at_ms.max(item.ended_at_ms);
            last.duration_ms = last.ended_at_ms.saturating_sub(last.started_at_ms).max(0) as u64;
            if last.icon_png.is_none() {
                last.icon_png = item.icon_png;
            }
            continue;
        }

        merged.push(item);
    }

    sort_timeline_items(&mut merged);
    merged
}

fn same_timeline_item(a: &TimelineItem, b: &TimelineItem) -> bool {
    a.process_name == b.process_name
        && a.exe_path == b.exe_path
        && a.window_class == b.window_class
        && a.window_title == b.window_title
}

pub(super) fn insert_session_record(tx: &Transaction<'_>, session: &SessionRecord) -> Result<()> {
    tx.execute(
        r#"
        INSERT OR IGNORE INTO activity_sessions (
            started_at_ms,
            ended_at_ms,
            duration_ms,
            process_id,
            process_name,
            exe_path,
            window_class,
            window_title
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
        "#,
        params![
            session.started_at_ms,
            session.ended_at_ms,
            session.duration_ms,
            session.process_id,
            session.process_name,
            session.exe_path,
            session.window_class,
            session.window_title,
        ],
    )?;
    Ok(())
}

pub(super) fn normalize_interval(interval_ms: u64) -> u64 {
    if ALLOWED_INTERVAL_MS.contains(&interval_ms) {
        interval_ms
    } else {
        DEFAULT_INTERVAL_MS
    }
}

pub(super) fn normalize_cache_flush_interval(interval_ms: u64) -> u64 {
    if ALLOWED_CACHE_FLUSH_INTERVAL_MS.contains(&interval_ms) {
        interval_ms
    } else {
        DEFAULT_CACHE_FLUSH_INTERVAL_MS
    }
}

pub(super) fn process_key(info: &FocusInfo) -> String {
    if info.exe_path.trim().is_empty() {
        info.process_name.clone()
    } else {
        info.exe_path.clone()
    }
}

pub(super) fn process_key_from_parts(process_name: &str, exe_path: &str) -> String {
    if exe_path.trim().is_empty() {
        process_name.to_owned()
    } else {
        exe_path.to_owned()
    }
}

pub(super) fn archive_month(timestamp_ms: i64) -> (i32, u32) {
    Local
        .timestamp_millis_opt(timestamp_ms)
        .single()
        .map(|date_time| (date_time.year(), date_time.month()))
        .unwrap_or((1970, 1))
}

pub(super) fn months_in_range(range_start_ms: i64, range_end_ms: i64) -> Vec<(i32, u32)> {
    if range_end_ms <= range_start_ms {
        return Vec::new();
    }

    let start = Local
        .timestamp_millis_opt(range_start_ms)
        .single()
        .map(|date_time| date_time.date_naive())
        .unwrap_or_else(|| NaiveDate::from_ymd_opt(1970, 1, 1).expect("valid fallback date"));
    let end = Local
        .timestamp_millis_opt(range_end_ms.saturating_sub(1))
        .single()
        .map(|date_time| date_time.date_naive())
        .unwrap_or(start);

    let mut cursor =
        NaiveDate::from_ymd_opt(start.year(), start.month(), 1).expect("valid month start");
    let end_month = NaiveDate::from_ymd_opt(end.year(), end.month(), 1).expect("valid month start");
    let mut months = Vec::new();

    while cursor <= end_month {
        months.push((cursor.year(), cursor.month()));
        cursor = next_month(cursor);
    }

    months
}

fn next_month(date: NaiveDate) -> NaiveDate {
    let (year, month) = if date.month() == 12 {
        (date.year() + 1, 1)
    } else {
        (date.year(), date.month() + 1)
    };
    NaiveDate::from_ymd_opt(year, month, 1).expect("valid next month")
}
