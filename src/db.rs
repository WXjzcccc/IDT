use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicI64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, Result};
use rusqlite::{Connection, OptionalExtension, params, params_from_iter, types::Value};

use crate::process_icon;

mod helpers;
#[cfg(test)]
mod tests;

use helpers::*;

pub const DEFAULT_INTERVAL_MS: u64 = 1_000;
pub const MIN_INTERVAL_MS: u64 = 200;
pub const MAX_INTERVAL_MS: u64 = 5_000;
pub const ALLOWED_INTERVAL_MS: [u64; 5] = [200, 500, 1_000, 3_000, 5_000];
pub const DEFAULT_CACHE_FLUSH_INTERVAL_MS: u64 = 10_000;
pub const ALLOWED_CACHE_FLUSH_INTERVAL_MS: [u64; 4] = [5_000, 10_000, 30_000, 60_000];
pub const DEFAULT_WINDOW_WIDTH: u32 = 1_120;
pub const DEFAULT_WINDOW_HEIGHT: u32 = 760;
pub const MIN_WINDOW_WIDTH: u32 = 880;
pub const MIN_WINDOW_HEIGHT: u32 = 620;
pub const MAX_WINDOW_WIDTH: u32 = 10_000;
pub const MAX_WINDOW_HEIGHT: u32 = 10_000;
const WEEK_MS: i64 = 7 * 24 * 60 * 60 * 1_000;
const ARCHIVE_CHECK_INTERVAL_MS: i64 = 60 * 60 * 1_000;
const APP_TOTAL_ICON_LIMIT: usize = 32;

const MAIN_DB_NAME: &str = "idt.sqlite3";
const ICON_DB_NAME: &str = "icons.sqlite3";
const ARCHIVE_DIR_NAME: &str = "archive";
const ACTIVITY_TABLE_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS activity_sessions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    started_at_ms INTEGER NOT NULL,
    ended_at_ms INTEGER NOT NULL,
    duration_ms INTEGER NOT NULL,
    process_id INTEGER NOT NULL,
    process_name TEXT NOT NULL,
    exe_path TEXT NOT NULL,
    window_class TEXT NOT NULL,
    window_title TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_activity_sessions_started_at
    ON activity_sessions(started_at_ms);

CREATE INDEX IF NOT EXISTS idx_activity_sessions_ended_at
    ON activity_sessions(ended_at_ms);

CREATE INDEX IF NOT EXISTS idx_activity_sessions_process
    ON activity_sessions(process_name, started_at_ms);

CREATE UNIQUE INDEX IF NOT EXISTS idx_activity_sessions_identity
    ON activity_sessions(
        started_at_ms,
        ended_at_ms,
        process_id,
        process_name,
        exe_path,
        window_class,
        window_title
    );
"#;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThemePreference {
    Light,
    Dark,
}

impl ThemePreference {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    pub fn is_dark(self) -> bool {
        matches!(self, Self::Dark)
    }

    pub fn toggled(self) -> Self {
        if self.is_dark() {
            Self::Light
        } else {
            Self::Dark
        }
    }

    fn parse(value: &str) -> Self {
        if value.eq_ignore_ascii_case("dark") {
            Self::Dark
        } else {
            Self::Light
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CloseBehavior {
    Minimize,
    HideToTray,
    Exit,
}

impl CloseBehavior {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Minimize => "minimize",
            Self::HideToTray => "hide_to_tray",
            Self::Exit => "exit",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "minimize" => Self::Minimize,
            "exit" => Self::Exit,
            _ => Self::HideToTray,
        }
    }
}

#[derive(Clone, Debug)]
pub struct AppSettings {
    pub theme: ThemePreference,
    pub autostart_enabled: bool,
    pub silent_start: bool,
    pub close_behavior: CloseBehavior,
    pub cache_flush_interval_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WindowSize {
    pub width: u32,
    pub height: u32,
}

impl Default for WindowSize {
    fn default() -> Self {
        Self {
            width: DEFAULT_WINDOW_WIDTH,
            height: DEFAULT_WINDOW_HEIGHT,
        }
    }
}

impl WindowSize {
    pub fn normalized(width: u32, height: u32) -> Self {
        Self {
            width: width.clamp(MIN_WINDOW_WIDTH, MAX_WINDOW_WIDTH),
            height: height.clamp(MIN_WINDOW_HEIGHT, MAX_WINDOW_HEIGHT),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FocusInfo {
    pub process_id: u32,
    pub process_name: String,
    pub exe_path: String,
    pub window_class: String,
    pub window_title: String,
}

#[derive(Clone, Debug)]
pub struct TimelineItem {
    pub started_at_ms: i64,
    pub ended_at_ms: i64,
    pub duration_ms: u64,
    pub process_name: String,
    pub exe_path: String,
    pub icon_png: Option<Arc<[u8]>>,
    pub window_title: String,
    pub window_class: String,
}

#[derive(Clone, Debug)]
pub struct AppTotal {
    pub process_name: String,
    pub exe_path: String,
    pub icon_png: Option<Arc<[u8]>>,
    pub duration_ms: u64,
    pub percent: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct DashboardBucket {
    pub start_ms: i64,
    pub end_ms: i64,
}

#[derive(Clone, Debug)]
pub struct AppBucketTotal {
    pub bucket_ix: usize,
    pub process_name: String,
    pub duration_ms: u64,
}

#[derive(Clone, Debug)]
pub struct DashboardData {
    pub total_ms: u64,
    pub interval_ms: u64,
    pub record_count: usize,
    pub app_totals: Vec<AppTotal>,
    pub bucket_totals: Vec<AppBucketTotal>,
}

#[derive(Clone)]
pub struct Database {
    main_path: Arc<PathBuf>,
    icons_path: Arc<PathBuf>,
    archive_dir: Arc<PathBuf>,
    last_archive_check_ms: Arc<AtomicI64>,
    usage_cache: Arc<Mutex<UsageCache>>,
}

impl Database {
    pub fn open_default() -> Result<Self> {
        let base_dir = dirs_next::data_local_dir()
            .or_else(|| std::env::current_dir().ok())
            .context("unable to resolve a data directory")?;
        Self::open(base_dir.join("IDT"))
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let app_dir = if path.extension().is_some() {
            path.parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."))
        } else {
            path.to_path_buf()
        };
        let main_path = if path.extension().is_some() {
            path.to_path_buf()
        } else {
            app_dir.join(MAIN_DB_NAME)
        };
        let icons_path = app_dir.join(ICON_DB_NAME);
        let archive_dir = app_dir.join(ARCHIVE_DIR_NAME);

        fs::create_dir_all(&app_dir).context("unable to create IDT data directory")?;
        fs::create_dir_all(&archive_dir).context("unable to create IDT archive directory")?;

        let database = Self {
            main_path: Arc::new(main_path),
            icons_path: Arc::new(icons_path),
            archive_dir: Arc::new(archive_dir),
            last_archive_check_ms: Arc::new(AtomicI64::new(0)),
            usage_cache: Arc::new(Mutex::new(UsageCache::default())),
        };
        database.initialize()?;
        database.configure_connection_modes()?;
        Ok(database)
    }

    pub fn path(&self) -> &Path {
        self.main_path.as_ref()
    }

    pub fn icons_path(&self) -> &Path {
        self.icons_path.as_ref()
    }

    pub fn archive_dir(&self) -> &Path {
        self.archive_dir.as_ref()
    }

    pub fn now_ms() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or_default()
    }

    pub fn app_settings(&self) -> Result<AppSettings> {
        let conn = self.connect()?;
        Ok(AppSettings {
            theme: ThemePreference::parse(
                &setting_value(&conn, "theme_mode")?.unwrap_or_else(|| "light".to_owned()),
            ),
            autostart_enabled: parse_bool_setting(
                setting_value(&conn, "autostart_enabled")?.as_deref(),
            ),
            silent_start: parse_bool_setting(setting_value(&conn, "silent_start")?.as_deref()),
            close_behavior: CloseBehavior::parse(
                &setting_value(&conn, "close_behavior")?
                    .unwrap_or_else(|| CloseBehavior::HideToTray.as_str().to_owned()),
            ),
            cache_flush_interval_ms: normalize_cache_flush_interval(
                setting_value(&conn, "cache_flush_interval_ms")?
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(DEFAULT_CACHE_FLUSH_INTERVAL_MS),
            ),
        })
    }

    pub fn get_interval_ms(&self) -> Result<u64> {
        let conn = self.connect()?;
        let value = setting_value(&conn, "capture_interval_ms")?
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(DEFAULT_INTERVAL_MS);

        Ok(normalize_interval(value))
    }

    pub fn get_cache_flush_interval_ms(&self) -> Result<u64> {
        let conn = self.connect()?;
        let value = setting_value(&conn, "cache_flush_interval_ms")?
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(DEFAULT_CACHE_FLUSH_INTERVAL_MS);

        Ok(normalize_cache_flush_interval(value))
    }

    pub fn get_window_size(&self) -> Result<Option<WindowSize>> {
        let conn = self.connect()?;
        let width = setting_value(&conn, "window_width")?.and_then(parse_window_width);
        let height = setting_value(&conn, "window_height")?.and_then(parse_window_height);

        Ok(width
            .zip(height)
            .map(|(width, height)| WindowSize { width, height }))
    }

    pub fn set_interval_ms(&self, interval_ms: u64) -> Result<u64> {
        let interval_ms = normalize_interval(interval_ms);
        let conn = self.connect()?;
        set_setting(&conn, "capture_interval_ms", interval_ms.to_string())?;
        Ok(interval_ms)
    }

    pub fn set_cache_flush_interval_ms(&self, interval_ms: u64) -> Result<u64> {
        let interval_ms = normalize_cache_flush_interval(interval_ms);
        let conn = self.connect()?;
        set_setting(&conn, "cache_flush_interval_ms", interval_ms.to_string())?;
        Ok(interval_ms)
    }

    pub fn set_theme_preference(&self, theme: ThemePreference) -> Result<()> {
        let conn = self.connect()?;
        set_setting(&conn, "theme_mode", theme.as_str())
    }

    pub fn set_autostart_enabled(&self, enabled: bool) -> Result<()> {
        let conn = self.connect()?;
        set_setting(&conn, "autostart_enabled", bool_setting(enabled))
    }

    pub fn set_silent_start(&self, enabled: bool) -> Result<()> {
        let conn = self.connect()?;
        set_setting(&conn, "silent_start", bool_setting(enabled))
    }

    pub fn get_close_behavior(&self) -> Result<CloseBehavior> {
        Ok(self.app_settings()?.close_behavior)
    }

    pub fn set_close_behavior(&self, behavior: CloseBehavior) -> Result<()> {
        let conn = self.connect()?;
        set_setting(&conn, "close_behavior", behavior.as_str())
    }

    pub fn set_window_size(&self, window_size: WindowSize) -> Result<()> {
        let window_size = WindowSize::normalized(window_size.width, window_size.height);
        let conn = self.connect()?;
        set_setting(&conn, "window_width", window_size.width.to_string())?;
        set_setting(&conn, "window_height", window_size.height.to_string())
    }

    pub fn append_usage(
        &self,
        info: &FocusInfo,
        started_at_ms: i64,
        ended_at_ms: i64,
    ) -> Result<()> {
        if ended_at_ms <= started_at_ms {
            return Ok(());
        }

        let mut cache = self.lock_usage_cache();
        cache.append(info, started_at_ms, ended_at_ms);
        Ok(())
    }

    pub fn flush_usage_cache(&self) -> Result<()> {
        let mut cache = self.lock_usage_cache();
        if cache.sessions.is_empty() {
            return Ok(());
        }

        self.write_usage_sessions_to_disk(&cache.sessions)?;
        cache.sessions.clear();
        Ok(())
    }

    pub fn dashboard_range(
        &self,
        range_start_ms: i64,
        range_end_ms: i64,
        buckets: &[DashboardBucket],
    ) -> Result<DashboardData> {
        let interval_ms = self.get_interval_ms()?;
        let mut total_ms = 0_u64;
        let mut record_count = 0_usize;
        let mut app_accumulators = BTreeMap::<String, AppTotalAccumulator>::new();
        let mut bucket_accumulators = BTreeMap::<(usize, String), u64>::new();

        {
            let cache = self.lock_usage_cache();
            for conn in self.connect_activity_sources(range_start_ms, range_end_ms)? {
                let (conn_total_ms, conn_record_count) =
                    dashboard_summary_from_conn(&conn, range_start_ms, range_end_ms)?;
                total_ms = total_ms.saturating_add(conn_total_ms);
                record_count = record_count.saturating_add(conn_record_count);
                merge_app_totals_from_conn(
                    &conn,
                    range_start_ms,
                    range_end_ms,
                    &mut app_accumulators,
                )?;
                merge_bucket_totals_from_conn(&conn, buckets, &mut bucket_accumulators)?;
            }
            merge_cached_dashboard_data(
                &cache.sessions,
                range_start_ms,
                range_end_ms,
                buckets,
                &mut total_ms,
                &mut record_count,
                &mut app_accumulators,
                &mut bucket_accumulators,
            );
        }

        let mut app_totals = build_app_totals(app_accumulators, total_ms);
        let icon_count = app_totals.len().min(APP_TOTAL_ICON_LIMIT);
        self.apply_app_total_icons(&mut app_totals[..icon_count])?;
        let bucket_totals = bucket_accumulators
            .into_iter()
            .map(|((bucket_ix, process_name), duration_ms)| AppBucketTotal {
                bucket_ix,
                process_name,
                duration_ms,
            })
            .collect();

        Ok(DashboardData {
            total_ms,
            interval_ms,
            record_count,
            app_totals,
            bucket_totals,
        })
    }

    pub fn timeline_count(
        &self,
        range_start_ms: i64,
        range_end_ms: i64,
        process_filter: &str,
        title_filter: &str,
    ) -> Result<usize> {
        let filter = TimelineFilter::new(process_filter, title_filter);
        let mut count = 0_usize;
        {
            let cache = self.lock_usage_cache();
            let cached_boundary_item =
                first_cached_timeline_item(&cache.sessions, range_start_ms, range_end_ms, &filter);
            let mut merged_cache_disk_boundary = false;
            for conn in self.connect_activity_sources(range_start_ms, range_end_ms)? {
                count = count.saturating_add(timeline_count_from_conn(
                    &conn,
                    range_start_ms,
                    range_end_ms,
                    &filter,
                )?);
                if !merged_cache_disk_boundary && let Some(item) = cached_boundary_item.as_ref() {
                    merged_cache_disk_boundary = timeline_merge_candidate_exists_from_conn(
                        &conn,
                        item,
                        range_start_ms,
                        range_end_ms,
                    )?;
                }
            }
            count = count.saturating_add(cached_timeline_count(
                &cache.sessions,
                range_start_ms,
                range_end_ms,
                &filter,
            ));
            if merged_cache_disk_boundary {
                count = count.saturating_sub(1);
            }
        }
        Ok(count)
    }

    pub fn timeline_page(
        &self,
        range_start_ms: i64,
        range_end_ms: i64,
        process_filter: &str,
        title_filter: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<TimelineItem>> {
        if limit == 0 || range_end_ms <= range_start_ms {
            return Ok(Vec::new());
        }

        let filter = TimelineFilter::new(process_filter, title_filter);
        let items = {
            let cache = self.lock_usage_cache();
            let cached_items =
                cached_timeline_items(&cache.sessions, range_start_ms, range_end_ms, &filter);
            let db_limit = offset
                .saturating_add(limit)
                .saturating_add(cached_items.len())
                .saturating_add(usize::from(!cached_items.is_empty()))
                .max(limit);
            let archive_paths = self.archive_paths_in_range(range_start_ms, range_end_ms);
            let conn = self.connect()?;
            let mut source_names = vec!["main".to_owned()];

            for (ix, path) in archive_paths.iter().enumerate() {
                let alias = format!("archive_{ix}");
                conn.execute(
                    &format!("ATTACH DATABASE ?1 AS {alias}"),
                    params![path.display().to_string()],
                )?;
                source_names.push(alias);
            }

            let mut sql_params = Vec::<Value>::new();
            let selects = source_names
                .iter()
                .map(|source_name| {
                    timeline_select_sql(
                        source_name,
                        range_start_ms,
                        range_end_ms,
                        &filter,
                        &mut sql_params,
                    )
                })
                .collect::<Vec<_>>()
                .join(" UNION ALL ");

            sql_params.push(Value::Integer(db_limit.min(i64::MAX as usize) as i64));

            let sql = format!(
                r#"
                SELECT
                    started_at_ms,
                    ended_at_ms,
                    duration_ms,
                    process_name,
                    exe_path,
                    window_title,
                    window_class
                FROM ({selects})
                ORDER BY started_at_ms DESC, ended_at_ms DESC, sort_id DESC
                LIMIT ?
                "#
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(sql_params), |row| {
                let started_at_ms = row.get::<_, i64>(0)?;
                let ended_at_ms = row.get::<_, i64>(1)?;
                let clipped_start_ms = started_at_ms.max(range_start_ms);
                let clipped_end_ms = ended_at_ms.min(range_end_ms);
                Ok(TimelineItem {
                    started_at_ms: clipped_start_ms,
                    ended_at_ms: clipped_end_ms,
                    duration_ms: clipped_end_ms.saturating_sub(clipped_start_ms).max(0) as u64,
                    process_name: row.get(3)?,
                    exe_path: row.get(4)?,
                    window_title: row.get(5)?,
                    window_class: row.get(6)?,
                    icon_png: None,
                })
            })?;
            let mut items = rows
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(anyhow::Error::from)?;
            items.extend(cached_items);
            items
        };

        let mut items = merge_contiguous_timeline_items(items)
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect::<Vec<_>>();
        self.apply_icons(&mut items)?;
        Ok(items)
    }

    fn initialize(&self) -> Result<()> {
        let conn = self.connect()?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at_ms INTEGER NOT NULL
            );
            "#,
        )?;
        conn.execute_batch(ACTIVITY_TABLE_SQL)?;

        conn.execute(
            r#"
            INSERT OR IGNORE INTO settings (key, value, updated_at_ms)
            VALUES ('capture_interval_ms', ?1, ?2)
            "#,
            params![DEFAULT_INTERVAL_MS.to_string(), Self::now_ms()],
        )?;
        insert_default_setting(&conn, "theme_mode", ThemePreference::Light.as_str())?;
        insert_default_setting(&conn, "autostart_enabled", bool_setting(false))?;
        insert_default_setting(&conn, "silent_start", bool_setting(false))?;
        insert_default_setting(&conn, "close_behavior", CloseBehavior::HideToTray.as_str())?;
        insert_default_setting(
            &conn,
            "cache_flush_interval_ms",
            DEFAULT_CACHE_FLUSH_INTERVAL_MS.to_string(),
        )?;

        let icons_conn = self.connect_icons()?;
        initialize_icon_schema(&icons_conn)?;
        self.migrate_icons_from_main(&conn, &icons_conn)?;
        self.archive_old_sessions()?;

        Ok(())
    }

    fn connect(&self) -> Result<Connection> {
        connect_path(self.main_path.as_ref())
    }

    fn connect_icons(&self) -> Result<Connection> {
        connect_path(self.icons_path.as_ref())
    }

    fn connect_archive(&self, year: i32, month: u32) -> Result<Connection> {
        let path = self.archive_path(year, month);
        let conn = connect_path(&path)?;
        conn.execute_batch(ACTIVITY_TABLE_SQL)?;
        Ok(conn)
    }

    fn configure_connection_modes(&self) -> Result<()> {
        let conn = self.connect()?;
        configure_database_mode(&conn)?;

        let icons_conn = self.connect_icons()?;
        configure_database_mode(&icons_conn)?;

        Ok(())
    }

    fn lock_usage_cache(&self) -> std::sync::MutexGuard<'_, UsageCache> {
        self.usage_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn write_usage_sessions_to_disk(&self, sessions: &[SessionRecord]) -> Result<()> {
        if sessions.is_empty() {
            return Ok(());
        }

        let mut seen_processes = BTreeSet::<String>::new();
        for session in sessions {
            let key = process_key_from_parts(&session.process_name, &session.exe_path);
            if seen_processes.insert(key) {
                self.ensure_process_icon(&FocusInfo {
                    process_id: session.process_id,
                    process_name: session.process_name.clone(),
                    exe_path: session.exe_path.clone(),
                    window_class: session.window_class.clone(),
                    window_title: session.window_title.clone(),
                })?;
            }
        }

        let mut conn = self.connect()?;
        let tx = conn.transaction()?;
        for session in sessions {
            append_session_record(&tx, session)?;
        }
        tx.commit()?;
        self.maybe_archive_old_sessions();
        Ok(())
    }

    fn apply_icons(&self, sessions: &mut [TimelineItem]) -> Result<()> {
        if sessions.is_empty() {
            return Ok(());
        }

        let conn = self.connect_icons()?;
        let mut stmt = conn.prepare("SELECT icon_png FROM process_icons WHERE process_key = ?1")?;
        let mut icons = BTreeMap::<String, Option<Arc<[u8]>>>::new();

        for item in sessions.iter() {
            let key = process_key_from_parts(&item.process_name, &item.exe_path);
            if icons.contains_key(&key) {
                continue;
            }

            let icon_png = stmt
                .query_row(params![key], |row| row.get::<_, Option<Vec<u8>>>(0))
                .optional()?
                .flatten()
                .map(Arc::<[u8]>::from);
            icons.insert(
                process_key_from_parts(&item.process_name, &item.exe_path),
                icon_png,
            );
        }

        for item in sessions.iter_mut() {
            let key = process_key_from_parts(&item.process_name, &item.exe_path);
            item.icon_png = icons.get(&key).cloned().flatten();
        }

        Ok(())
    }

    fn apply_app_total_icons(&self, totals: &mut [AppTotal]) -> Result<()> {
        if totals.is_empty() {
            return Ok(());
        }

        let conn = self.connect_icons()?;
        let mut stmt = conn.prepare("SELECT icon_png FROM process_icons WHERE process_key = ?1")?;
        let mut icons = BTreeMap::<String, Option<Arc<[u8]>>>::new();

        for total in totals.iter() {
            let key = process_key_from_parts(&total.process_name, &total.exe_path);
            if icons.contains_key(&key) {
                continue;
            }

            let icon_png = stmt
                .query_row(params![key], |row| row.get::<_, Option<Vec<u8>>>(0))
                .optional()?
                .flatten()
                .map(Arc::<[u8]>::from);
            icons.insert(key, icon_png);
        }

        for total in totals.iter_mut() {
            let key = process_key_from_parts(&total.process_name, &total.exe_path);
            total.icon_png = icons.get(&key).cloned().flatten();
        }

        Ok(())
    }

    fn connect_activity_sources(
        &self,
        range_start_ms: i64,
        range_end_ms: i64,
    ) -> Result<Vec<Connection>> {
        let mut connections = vec![self.connect()?];
        for path in self.archive_paths_in_range(range_start_ms, range_end_ms) {
            connections.push(connect_path(&path)?);
        }
        Ok(connections)
    }

    fn archive_paths_in_range(&self, range_start_ms: i64, range_end_ms: i64) -> Vec<PathBuf> {
        months_in_range(range_start_ms, range_end_ms)
            .into_iter()
            .map(|(year, month)| self.archive_path(year, month))
            .filter(|path| path.exists())
            .collect()
    }

    fn ensure_process_icon(&self, info: &FocusInfo) -> Result<()> {
        let conn = self.connect_icons()?;
        let process_key = process_key(info);
        let now_ms = Self::now_ms();
        let existing = conn
            .query_row(
                r#"
                SELECT icon_png, last_icon_attempt_ms
                FROM process_icons
                WHERE process_key = ?1
                "#,
                params![process_key],
                |row| {
                    Ok(ProcessIconState {
                        icon_png: row.get(0)?,
                        last_icon_attempt_ms: row.get(1)?,
                    })
                },
            )
            .optional()?;

        let should_attempt_icon = !info.exe_path.trim().is_empty()
            && existing.as_ref().is_none_or(|state| {
                state.icon_png.is_none()
                    && state.last_icon_attempt_ms.is_none_or(|attempted_at| {
                        now_ms.saturating_sub(attempted_at) >= process_icon::icon_retry_after_ms()
                    })
            });

        let extracted_icon = if should_attempt_icon {
            process_icon::extract_png_from_exe(&info.exe_path).ok()
        } else {
            None
        };

        let icon_png = extracted_icon.as_ref().map(|icon| icon.png.as_slice());
        let icon_width = extracted_icon.as_ref().map(|icon| icon.width);
        let icon_height = extracted_icon.as_ref().map(|icon| icon.height);
        let icon_source = extracted_icon.as_ref().map(|_| "exe");
        let last_icon_attempt_ms = should_attempt_icon.then_some(now_ms);

        conn.execute(
            r#"
            INSERT INTO process_icons (
                process_key,
                process_name,
                exe_path,
                icon_png,
                icon_width,
                icon_height,
                icon_source,
                last_icon_attempt_ms,
                created_at_ms,
                last_seen_at_ms
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)
            ON CONFLICT(process_key) DO UPDATE SET
                process_name = excluded.process_name,
                exe_path = excluded.exe_path,
                icon_png = COALESCE(excluded.icon_png, process_icons.icon_png),
                icon_width = COALESCE(excluded.icon_width, process_icons.icon_width),
                icon_height = COALESCE(excluded.icon_height, process_icons.icon_height),
                icon_source = COALESCE(excluded.icon_source, process_icons.icon_source),
                last_icon_attempt_ms = COALESCE(
                    excluded.last_icon_attempt_ms,
                    process_icons.last_icon_attempt_ms
                ),
                last_seen_at_ms = excluded.last_seen_at_ms
            "#,
            params![
                process_key,
                info.process_name,
                info.exe_path,
                icon_png,
                icon_width,
                icon_height,
                icon_source,
                last_icon_attempt_ms,
                now_ms,
            ],
        )?;

        Ok(())
    }

    fn maybe_archive_old_sessions(&self) {
        let now_ms = Self::now_ms();
        let last = self.last_archive_check_ms.load(Ordering::Relaxed);
        if now_ms.saturating_sub(last) < ARCHIVE_CHECK_INTERVAL_MS {
            return;
        }

        if self
            .last_archive_check_ms
            .compare_exchange(last, now_ms, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            if let Err(error) = self.archive_old_sessions() {
                eprintln!("failed to archive old activity sessions: {error:#}");
            }
        }
    }

    fn archive_old_sessions(&self) -> Result<()> {
        let cutoff_ms = Self::now_ms().saturating_sub(WEEK_MS);
        let mut conn = self.connect()?;
        let old_sessions = load_archive_candidates(&conn, cutoff_ms)?;
        if old_sessions.is_empty() {
            return Ok(());
        }

        let mut by_month = BTreeMap::<(i32, u32), Vec<SessionRecord>>::new();
        for session in old_sessions {
            let (year, month) = archive_month(session.started_at_ms);
            by_month.entry((year, month)).or_default().push(session);
        }

        for ((year, month), sessions) in by_month {
            let mut archive = self.connect_archive(year, month)?;
            let tx = archive.transaction()?;
            for session in &sessions {
                insert_session_record(&tx, session)?;
            }
            tx.commit()?;
        }

        let tx = conn.transaction()?;
        for id in load_archive_ids(&tx, cutoff_ms)? {
            tx.execute("DELETE FROM activity_sessions WHERE id = ?1", params![id])?;
        }
        tx.commit()?;
        Ok(())
    }

    fn migrate_icons_from_main(&self, main: &Connection, icons: &Connection) -> Result<()> {
        let has_legacy_icons = main
            .query_row(
                r#"
                SELECT 1
                FROM sqlite_master
                WHERE type = 'table' AND name = 'process_icons'
                "#,
                [],
                |_| Ok(()),
            )
            .optional()?
            .is_some();

        if !has_legacy_icons {
            return Ok(());
        }

        let mut stmt = main.prepare(
            r#"
            SELECT
                process_key,
                process_name,
                exe_path,
                icon_png,
                icon_width,
                icon_height,
                icon_source,
                last_icon_attempt_ms,
                created_at_ms,
                last_seen_at_ms
            FROM process_icons
            "#,
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(ProcessIconRecord {
                process_key: row.get(0)?,
                process_name: row.get(1)?,
                exe_path: row.get(2)?,
                icon_png: row.get(3)?,
                icon_width: row.get(4)?,
                icon_height: row.get(5)?,
                icon_source: row.get(6)?,
                last_icon_attempt_ms: row.get(7)?,
                created_at_ms: row.get(8)?,
                last_seen_at_ms: row.get(9)?,
            })
        })?;

        for row in rows {
            let row = row?;
            icons.execute(
                r#"
                INSERT INTO process_icons (
                    process_key,
                    process_name,
                    exe_path,
                    icon_png,
                    icon_width,
                    icon_height,
                    icon_source,
                    last_icon_attempt_ms,
                    created_at_ms,
                    last_seen_at_ms
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                ON CONFLICT(process_key) DO UPDATE SET
                    process_name = excluded.process_name,
                    exe_path = excluded.exe_path,
                    icon_png = COALESCE(process_icons.icon_png, excluded.icon_png),
                    icon_width = COALESCE(process_icons.icon_width, excluded.icon_width),
                    icon_height = COALESCE(process_icons.icon_height, excluded.icon_height),
                    icon_source = COALESCE(process_icons.icon_source, excluded.icon_source),
                    last_icon_attempt_ms = COALESCE(
                        process_icons.last_icon_attempt_ms,
                        excluded.last_icon_attempt_ms
                    ),
                    last_seen_at_ms = MAX(process_icons.last_seen_at_ms, excluded.last_seen_at_ms)
                "#,
                params![
                    row.process_key,
                    row.process_name,
                    row.exe_path,
                    row.icon_png,
                    row.icon_width,
                    row.icon_height,
                    row.icon_source,
                    row.last_icon_attempt_ms,
                    row.created_at_ms,
                    row.last_seen_at_ms,
                ],
            )?;
        }

        Ok(())
    }

    fn archive_path(&self, year: i32, month: u32) -> PathBuf {
        self.archive_dir
            .join(format!("idt-{year:04}-{month:02}.sqlite3"))
    }
}

#[derive(Default)]
struct UsageCache {
    sessions: Vec<SessionRecord>,
}

impl UsageCache {
    fn append(&mut self, info: &FocusInfo, started_at_ms: i64, ended_at_ms: i64) {
        let duration_ms = ended_at_ms.saturating_sub(started_at_ms) as u64;
        if let Some(last) = self.sessions.last_mut() {
            if same_focus_info(last, info)
                && started_at_ms <= last.ended_at_ms.saturating_add(MAX_INTERVAL_MS as i64)
            {
                last.ended_at_ms = ended_at_ms;
                last.duration_ms =
                    last.ended_at_ms.saturating_sub(last.started_at_ms).max(0) as u64;
                last.process_id = info.process_id;
                return;
            }
        }

        self.sessions.push(SessionRecord {
            started_at_ms,
            ended_at_ms,
            duration_ms,
            process_id: info.process_id,
            process_name: info.process_name.clone(),
            exe_path: info.exe_path.clone(),
            window_class: info.window_class.clone(),
            window_title: info.window_title.clone(),
        });
    }
}

#[derive(Debug)]
struct LastSession {
    id: i64,
    ended_at_ms: i64,
    process_name: String,
    exe_path: String,
    window_class: String,
    window_title: String,
}

struct ProcessIconState {
    icon_png: Option<Vec<u8>>,
    last_icon_attempt_ms: Option<i64>,
}

#[derive(Clone, Debug)]
struct SessionRecord {
    started_at_ms: i64,
    ended_at_ms: i64,
    duration_ms: u64,
    process_id: u32,
    process_name: String,
    exe_path: String,
    window_class: String,
    window_title: String,
}

struct ProcessIconRecord {
    process_key: String,
    process_name: String,
    exe_path: String,
    icon_png: Option<Vec<u8>>,
    icon_width: Option<i32>,
    icon_height: Option<i32>,
    icon_source: Option<String>,
    last_icon_attempt_ms: Option<i64>,
    created_at_ms: i64,
    last_seen_at_ms: i64,
}

fn connect_path(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(std::time::Duration::from_secs(2))?;
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    Ok(conn)
}

fn configure_database_mode(conn: &Connection) -> Result<()> {
    conn.execute_batch("PRAGMA journal_mode = WAL;")?;
    Ok(())
}

fn initialize_icon_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS process_icons (
            process_key TEXT PRIMARY KEY,
            process_name TEXT NOT NULL,
            exe_path TEXT NOT NULL,
            icon_png BLOB,
            icon_width INTEGER,
            icon_height INTEGER,
            icon_source TEXT,
            last_icon_attempt_ms INTEGER,
            created_at_ms INTEGER NOT NULL,
            last_seen_at_ms INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_process_icons_process
            ON process_icons(process_name);
        "#,
    )?;
    Ok(())
}

fn insert_default_setting(conn: &Connection, key: &str, value: impl ToString) -> Result<()> {
    conn.execute(
        r#"
        INSERT OR IGNORE INTO settings (key, value, updated_at_ms)
        VALUES (?1, ?2, ?3)
        "#,
        params![key, value.to_string(), Database::now_ms()],
    )?;
    Ok(())
}

fn setting_value(conn: &Connection, key: &str) -> Result<Option<String>> {
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        params![key],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .map_err(Into::into)
}

fn set_setting(conn: &Connection, key: &str, value: impl ToString) -> Result<()> {
    conn.execute(
        r#"
        INSERT INTO settings (key, value, updated_at_ms)
        VALUES (?1, ?2, ?3)
        ON CONFLICT(key) DO UPDATE SET
            value = excluded.value,
            updated_at_ms = excluded.updated_at_ms
        "#,
        params![key, value.to_string(), Database::now_ms()],
    )?;
    Ok(())
}

fn bool_setting(value: bool) -> &'static str {
    if value { "1" } else { "0" }
}

fn parse_bool_setting(value: Option<&str>) -> bool {
    matches!(
        value
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn parse_window_width(value: String) -> Option<u32> {
    parse_window_dimension(value, MIN_WINDOW_WIDTH, MAX_WINDOW_WIDTH)
}

fn parse_window_height(value: String) -> Option<u32> {
    parse_window_dimension(value, MIN_WINDOW_HEIGHT, MAX_WINDOW_HEIGHT)
}

fn parse_window_dimension(value: String, min: u32, max: u32) -> Option<u32> {
    value
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|dimension| (min..=max).contains(dimension))
}

#[derive(Default)]
struct AppTotalAccumulator {
    duration_ms: u64,
    icon_exe_path: String,
    icon_exe_duration_ms: u64,
}

struct TimelineFilter {
    process_contains: Option<String>,
    title_contains: Option<String>,
    process_like: Option<String>,
    title_like: Option<String>,
}

impl TimelineFilter {
    fn new(process_filter: &str, title_filter: &str) -> Self {
        Self {
            process_contains: contains_filter(process_filter),
            title_contains: contains_filter(title_filter),
            process_like: like_filter(process_filter),
            title_like: like_filter(title_filter),
        }
    }
}

fn dashboard_summary_from_conn(
    conn: &Connection,
    range_start_ms: i64,
    range_end_ms: i64,
) -> Result<(u64, usize)> {
    let mut stmt = conn.prepare(
        r#"
        SELECT
            COALESCE(SUM(MAX(0, MIN(ended_at_ms, ?2) - MAX(started_at_ms, ?1))), 0),
            COUNT(*)
        FROM activity_sessions
        WHERE ended_at_ms > ?1 AND started_at_ms < ?2
        "#,
    )?;

    stmt.query_row(params![range_start_ms, range_end_ms], |row| {
        Ok((
            row.get::<_, i64>(0)?.max(0) as u64,
            row.get::<_, i64>(1)?.max(0) as usize,
        ))
    })
    .map_err(Into::into)
}

fn merge_app_totals_from_conn(
    conn: &Connection,
    range_start_ms: i64,
    range_end_ms: i64,
    totals: &mut BTreeMap<String, AppTotalAccumulator>,
) -> Result<()> {
    let mut stmt = conn.prepare(
        r#"
        SELECT
            process_name,
            exe_path,
            SUM(MAX(0, MIN(ended_at_ms, ?2) - MAX(started_at_ms, ?1))) AS clipped_duration_ms
        FROM activity_sessions
        WHERE ended_at_ms > ?1 AND started_at_ms < ?2
        GROUP BY process_name, exe_path
        HAVING clipped_duration_ms > 0
        "#,
    )?;

    let rows = stmt.query_map(params![range_start_ms, range_end_ms], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?.max(0) as u64,
        ))
    })?;

    for row in rows {
        let (process_name, exe_path, duration_ms) = row?;
        let entry = totals.entry(process_name).or_default();
        entry.duration_ms = entry.duration_ms.saturating_add(duration_ms);
        if duration_ms > entry.icon_exe_duration_ms {
            entry.icon_exe_path = exe_path;
            entry.icon_exe_duration_ms = duration_ms;
        }
    }

    Ok(())
}

fn merge_bucket_totals_from_conn(
    conn: &Connection,
    buckets: &[DashboardBucket],
    totals: &mut BTreeMap<(usize, String), u64>,
) -> Result<()> {
    let mut stmt = conn.prepare(
        r#"
        SELECT
            process_name,
            SUM(MAX(0, MIN(ended_at_ms, ?2) - MAX(started_at_ms, ?1))) AS clipped_duration_ms
        FROM activity_sessions
        WHERE ended_at_ms > ?1 AND started_at_ms < ?2
        GROUP BY process_name
        HAVING clipped_duration_ms > 0
        "#,
    )?;

    for (bucket_ix, bucket) in buckets.iter().enumerate() {
        let rows = stmt.query_map(params![bucket.start_ms, bucket.end_ms], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?.max(0) as u64,
            ))
        })?;

        for row in rows {
            let (process_name, duration_ms) = row?;
            let entry = totals.entry((bucket_ix, process_name)).or_default();
            *entry = entry.saturating_add(duration_ms);
        }
    }

    Ok(())
}

fn timeline_count_from_conn(
    conn: &Connection,
    range_start_ms: i64,
    range_end_ms: i64,
    filter: &TimelineFilter,
) -> Result<usize> {
    let mut sql = String::from(
        r#"
        SELECT COUNT(*)
        FROM activity_sessions
        WHERE ended_at_ms > ? AND started_at_ms < ?
        "#,
    );
    let mut sql_params = vec![Value::Integer(range_start_ms), Value::Integer(range_end_ms)];
    append_filter_sql(&mut sql, filter, &mut sql_params);

    let mut stmt = conn.prepare(&sql)?;
    stmt.query_row(params_from_iter(sql_params), |row| {
        Ok(row.get::<_, i64>(0)?.max(0) as usize)
    })
    .map_err(Into::into)
}

fn timeline_merge_candidate_exists_from_conn(
    conn: &Connection,
    item: &TimelineItem,
    range_start_ms: i64,
    range_end_ms: i64,
) -> Result<bool> {
    let mut stmt = conn.prepare(
        r#"
        SELECT 1
        FROM activity_sessions
        WHERE ended_at_ms > ?1
            AND started_at_ms < ?2
            AND process_name = ?3
            AND exe_path = ?4
            AND window_title = ?5
            AND window_class = ?6
            AND ?7 <= MIN(ended_at_ms, ?2)
            AND MAX(started_at_ms, ?1) <= ?8
        LIMIT 1
        "#,
    )?;

    let exists = stmt
        .query_row(
            params![
                range_start_ms,
                range_end_ms,
                item.process_name,
                item.exe_path,
                item.window_title,
                item.window_class,
                item.started_at_ms,
                item.ended_at_ms,
            ],
            |_| Ok(()),
        )
        .optional()?
        .is_some();

    Ok(exists)
}

fn timeline_select_sql(
    source_name: &str,
    range_start_ms: i64,
    range_end_ms: i64,
    filter: &TimelineFilter,
    sql_params: &mut Vec<Value>,
) -> String {
    sql_params.push(Value::Integer(range_start_ms));
    sql_params.push(Value::Integer(range_end_ms));

    let mut sql = format!(
        r#"
        SELECT
            id AS sort_id,
            started_at_ms,
            ended_at_ms,
            duration_ms,
            process_name,
            exe_path,
            window_title,
            window_class
        FROM {source_name}.activity_sessions
        WHERE ended_at_ms > ? AND started_at_ms < ?
        "#
    );
    append_filter_sql(&mut sql, filter, sql_params);
    sql
}

fn append_filter_sql(sql: &mut String, filter: &TimelineFilter, sql_params: &mut Vec<Value>) {
    if let Some(process_like) = filter.process_like.as_ref() {
        sql.push_str(" AND LOWER(process_name) LIKE ? ESCAPE '\\'");
        sql_params.push(Value::Text(process_like.clone()));
    }
    if let Some(title_like) = filter.title_like.as_ref() {
        sql.push_str(" AND LOWER(window_title) LIKE ? ESCAPE '\\'");
        sql_params.push(Value::Text(title_like.clone()));
    }
}

fn contains_filter(value: &str) -> Option<String> {
    let value = value.trim().to_lowercase();
    if value.is_empty() { None } else { Some(value) }
}

fn like_filter(value: &str) -> Option<String> {
    let value = value.trim().to_lowercase();
    if value.is_empty() {
        return None;
    }

    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('%');
    for ch in value.chars() {
        match ch {
            '%' | '_' | '\\' => {
                escaped.push('\\');
                escaped.push(ch);
            }
            _ => escaped.push(ch),
        }
    }
    escaped.push('%');
    Some(escaped)
}

fn build_app_totals(totals: BTreeMap<String, AppTotalAccumulator>, total_ms: u64) -> Vec<AppTotal> {
    let mut totals = totals
        .into_iter()
        .map(|(process_name, accumulator)| {
            let percent = if total_ms == 0 {
                0.0
            } else {
                accumulator.duration_ms as f32 / total_ms as f32
            };
            AppTotal {
                process_name,
                exe_path: accumulator.icon_exe_path,
                icon_png: None,
                duration_ms: accumulator.duration_ms,
                percent,
            }
        })
        .collect::<Vec<_>>();

    totals.sort_by(|a, b| {
        b.duration_ms
            .cmp(&a.duration_ms)
            .then_with(|| a.process_name.cmp(&b.process_name))
    });
    totals
}
