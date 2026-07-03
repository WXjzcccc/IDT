use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, Result};
use chrono::{Datelike, Duration as ChronoDuration, Local, NaiveDate, TimeZone};
use rusqlite::{Connection, OptionalExtension, params};

const TODO_DB_NAME: &str = "todos.sqlite3";
pub const TODO_WINDOW_MIN_WIDTH: u32 = 320;
pub const TODO_WINDOW_MIN_HEIGHT: u32 = 260;
const TODO_WINDOW_MAX_WIDTH: u32 = 1200;
const TODO_WINDOW_MAX_HEIGHT: u32 = 1400;

#[derive(Clone, Debug)]
pub struct TodoItem {
    pub id: i64,
    pub title: String,
    pub details: String,
    pub tags: Vec<TodoTag>,
    pub started_at_ms: i64,
    pub due_at_ms: Option<i64>,
    pub completed_at_ms: Option<i64>,
    pub subtasks: Vec<TodoSubtask>,
}

#[derive(Clone, Debug)]
pub struct TodoTag {
    pub id: i64,
    pub name: String,
    pub color: String,
}

#[derive(Clone, Debug)]
pub struct TodoSubtask {
    pub id: i64,
    pub title: String,
    pub completed: bool,
}

#[derive(Clone, Debug)]
pub struct TodoDraft {
    pub id: Option<i64>,
    pub title: String,
    pub details: String,
    pub tag_ids: Vec<i64>,
    pub started_at_ms: i64,
    pub due_at_ms: Option<i64>,
    pub completed_at_ms: Option<i64>,
    pub subtasks: Vec<TodoSubtaskDraft>,
}

#[derive(Clone, Debug)]
pub struct TodoSubtaskDraft {
    pub id: Option<i64>,
    pub title: String,
    pub completed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TodoWindowTheme {
    Light,
    Dark,
}

impl TodoWindowTheme {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    fn parse(value: &str) -> Self {
        if value.eq_ignore_ascii_case("dark") {
            Self::Dark
        } else {
            Self::Light
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            Self::Light => Self::Dark,
            Self::Dark => Self::Light,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TodoWindowSettings {
    pub theme: TodoWindowTheme,
    pub opacity_percent: u8,
    pub locked: bool,
    pub width: u32,
    pub height: u32,
    pub x: Option<i32>,
    pub y: Option<i32>,
}

impl Default for TodoWindowSettings {
    fn default() -> Self {
        Self {
            theme: TodoWindowTheme::Light,
            opacity_percent: 96,
            locked: false,
            width: 420,
            height: 520,
            x: None,
            y: None,
        }
    }
}

#[derive(Clone)]
pub struct TodoDatabase {
    path: Arc<PathBuf>,
}

impl TodoDatabase {
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
        let todo_path = if path.extension().is_some() {
            path.to_path_buf()
        } else {
            app_dir.join(TODO_DB_NAME)
        };

        fs::create_dir_all(&app_dir).context("unable to create IDT data directory")?;

        let database = Self {
            path: Arc::new(todo_path),
        };
        database.initialize()?;
        Ok(database)
    }

    pub fn now_ms() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or_default()
    }

    pub fn path(&self) -> &Path {
        self.path.as_ref()
    }

    pub fn load_month_items(&self, month: NaiveDate) -> Result<Vec<TodoItem>> {
        let (month_start_ms, month_end_ms) = month_bounds(month);
        let now_ms = Self::now_ms();
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT id, title, details, started_at_ms, due_at_ms, completed_at_ms
            FROM todo_items
            WHERE
                (started_at_ms < ?2 AND COALESCE(completed_at_ms, ?3) >= ?1)
                OR (due_at_ms IS NOT NULL AND due_at_ms >= ?1 AND due_at_ms < ?2)
            ORDER BY COALESCE(completed_at_ms, ?3) DESC, started_at_ms ASC, id ASC
            "#,
        )?;
        let mut items = stmt
            .query_map(params![month_start_ms, month_end_ms, now_ms], item_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        self.load_item_tags(&conn, &mut items)?;
        self.load_subtasks(&conn, &mut items)?;
        Ok(items)
    }

    pub fn load_open_items(&self) -> Result<Vec<TodoItem>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT id, title, details, started_at_ms, due_at_ms, completed_at_ms
            FROM todo_items
            WHERE completed_at_ms IS NULL
            ORDER BY
                CASE WHEN due_at_ms IS NULL THEN 1 ELSE 0 END ASC,
                due_at_ms ASC,
                started_at_ms ASC,
                id ASC
            "#,
        )?;
        let mut items = stmt
            .query_map([], item_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        self.load_item_tags(&conn, &mut items)?;
        self.load_subtasks(&conn, &mut items)?;
        Ok(items)
    }

    pub fn load_item(&self, id: i64) -> Result<Option<TodoItem>> {
        let conn = self.connect()?;
        let mut item = conn
            .query_row(
                r#"
                SELECT id, title, details, started_at_ms, due_at_ms, completed_at_ms
                FROM todo_items
                WHERE id = ?1
                "#,
                params![id],
                item_from_row,
            )
            .optional()?;
        if let Some(item) = item.as_mut() {
            self.load_item_tags(&conn, std::slice::from_mut(item))?;
            self.load_subtasks(&conn, std::slice::from_mut(item))?;
        }
        Ok(item)
    }

    pub fn load_tags(&self) -> Result<Vec<TodoTag>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT id, name, color
            FROM todo_tags
            ORDER BY name COLLATE NOCASE ASC, id ASC
            "#,
        )?;
        stmt.query_map([], tag_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn save_tag(&self, name: &str, color: &str) -> Result<i64> {
        let name = name.trim();
        if name.is_empty() {
            anyhow::bail!("标签名不能为空");
        }
        let color = normalized_color(color);
        let now_ms = Self::now_ms();
        let conn = self.connect()?;
        conn.execute(
            r#"
            INSERT INTO todo_tags (name, color, created_at_ms, updated_at_ms)
            VALUES (?1, ?2, ?3, ?3)
            ON CONFLICT(name) DO UPDATE SET
                color = excluded.color,
                updated_at_ms = excluded.updated_at_ms
            "#,
            params![name, color, now_ms],
        )?;
        Ok(conn.query_row(
            "SELECT id FROM todo_tags WHERE name = ?1",
            params![name],
            |row| row.get(0),
        )?)
    }

    pub fn update_tag(&self, id: i64, name: &str, color: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            anyhow::bail!("标签名不能为空");
        }

        let color = normalized_color(color);
        let now_ms = Self::now_ms();
        let conn = self.connect()?;
        conn.execute(
            r#"
            UPDATE todo_tags
            SET name = ?1,
                color = ?2,
                updated_at_ms = ?3
            WHERE id = ?4
            "#,
            params![name, color, now_ms, id],
        )?;
        Ok(())
    }

    pub fn delete_tag(&self, id: i64) -> Result<()> {
        let mut conn = self.connect()?;
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM todo_item_tags WHERE tag_id = ?1", params![id])?;
        tx.execute("DELETE FROM todo_tags WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(())
    }

    pub fn todo_window_settings(&self) -> Result<TodoWindowSettings> {
        let conn = self.connect()?;
        let theme = setting_value(&conn, "todo_window_theme")?
            .map(|value| TodoWindowTheme::parse(&value))
            .unwrap_or(TodoWindowSettings::default().theme);
        let opacity_percent = setting_value(&conn, "todo_window_opacity")?
            .and_then(|value| value.parse::<u8>().ok())
            .unwrap_or(TodoWindowSettings::default().opacity_percent)
            .clamp(40, 100);
        let locked = parse_bool_setting(setting_value(&conn, "todo_window_locked")?.as_deref());
        let width = setting_value(&conn, "todo_window_width")?
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(TodoWindowSettings::default().width)
            .clamp(TODO_WINDOW_MIN_WIDTH, TODO_WINDOW_MAX_WIDTH);
        let height = setting_value(&conn, "todo_window_height")?
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(TodoWindowSettings::default().height)
            .clamp(TODO_WINDOW_MIN_HEIGHT, TODO_WINDOW_MAX_HEIGHT);
        let x = setting_value(&conn, "todo_window_x")?.and_then(parse_window_position);
        let y = setting_value(&conn, "todo_window_y")?.and_then(parse_window_position);

        Ok(TodoWindowSettings {
            theme,
            opacity_percent,
            locked,
            width,
            height,
            x,
            y,
        })
    }

    pub fn set_todo_window_settings(&self, settings: &TodoWindowSettings) -> Result<()> {
        let conn = self.connect()?;
        set_setting(&conn, "todo_window_theme", settings.theme.as_str())?;
        set_setting(
            &conn,
            "todo_window_opacity",
            settings.opacity_percent.clamp(40, 100),
        )?;
        set_setting(&conn, "todo_window_locked", bool_setting(settings.locked))?;
        set_setting(
            &conn,
            "todo_window_width",
            settings
                .width
                .clamp(TODO_WINDOW_MIN_WIDTH, TODO_WINDOW_MAX_WIDTH),
        )?;
        set_setting(
            &conn,
            "todo_window_height",
            settings
                .height
                .clamp(TODO_WINDOW_MIN_HEIGHT, TODO_WINDOW_MAX_HEIGHT),
        )?;
        if let Some(x) = settings.x {
            set_setting(&conn, "todo_window_x", x)?;
        }
        if let Some(y) = settings.y {
            set_setting(&conn, "todo_window_y", y)?;
        }
        Ok(())
    }

    pub fn save_item(&self, draft: &TodoDraft) -> Result<i64> {
        let title = draft.title.trim();
        if title.is_empty() {
            anyhow::bail!("标题不能为空");
        }

        let now_ms = Self::now_ms();
        let mut conn = self.connect()?;
        let tx = conn.transaction()?;

        let item_id = if let Some(id) = draft.id {
            tx.execute(
                r#"
                UPDATE todo_items
                SET title = ?1,
                    details = ?2,
                    tags = ?3,
                    started_at_ms = ?4,
                    due_at_ms = ?5,
                    completed_at_ms = ?6,
                    updated_at_ms = ?7
                WHERE id = ?8
                "#,
                params![
                    title,
                    draft.details.trim(),
                    "",
                    draft.started_at_ms,
                    draft.due_at_ms,
                    draft.completed_at_ms,
                    now_ms,
                    id
                ],
            )?;
            id
        } else {
            tx.execute(
                r#"
                INSERT INTO todo_items (
                    title,
                    details,
                    tags,
                    started_at_ms,
                    due_at_ms,
                    completed_at_ms,
                    created_at_ms,
                    updated_at_ms
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
                "#,
                params![
                    title,
                    draft.details.trim(),
                    "",
                    draft.started_at_ms,
                    draft.due_at_ms,
                    draft.completed_at_ms,
                    now_ms
                ],
            )?;
            tx.last_insert_rowid()
        };

        tx.execute(
            "DELETE FROM todo_item_tags WHERE item_id = ?1",
            params![item_id],
        )?;
        for (ix, tag_id) in draft.tag_ids.iter().copied().enumerate() {
            tx.execute(
                r#"
                INSERT OR IGNORE INTO todo_item_tags (item_id, tag_id, position)
                VALUES (?1, ?2, ?3)
                "#,
                params![item_id, tag_id, ix as i64],
            )?;
        }

        for (ix, subtask) in draft
            .subtasks
            .iter()
            .filter(|subtask| !subtask.title.trim().is_empty())
            .enumerate()
        {
            if let Some(id) = subtask.id {
                tx.execute(
                    r#"
                    UPDATE todo_subtasks
                    SET title = ?1,
                        completed = ?2,
                        position = ?3,
                        updated_at_ms = ?4
                    WHERE id = ?5 AND item_id = ?6
                    "#,
                    params![
                        subtask.title.trim(),
                        bool_int(subtask.completed),
                        ix as i64,
                        now_ms,
                        id,
                        item_id
                    ],
                )?;
            } else {
                tx.execute(
                    r#"
                    INSERT INTO todo_subtasks (
                        item_id,
                        title,
                        completed,
                        position,
                        created_at_ms,
                        updated_at_ms
                    )
                    VALUES (?1, ?2, ?3, ?4, ?5, ?5)
                    "#,
                    params![
                        item_id,
                        subtask.title.trim(),
                        bool_int(subtask.completed),
                        ix as i64,
                        now_ms
                    ],
                )?;
            }
        }

        tx.commit()?;
        Ok(item_id)
    }

    pub fn set_item_completed(&self, id: i64, completed: bool) -> Result<()> {
        let conn = self.connect()?;
        let now_ms = Self::now_ms();
        let completed_at_ms = completed.then_some(now_ms);
        conn.execute(
            r#"
            UPDATE todo_items
            SET completed_at_ms = ?1, updated_at_ms = ?2
            WHERE id = ?3
            "#,
            params![completed_at_ms, now_ms, id],
        )?;
        Ok(())
    }

    pub fn set_subtask_completed(&self, id: i64, completed: bool) -> Result<()> {
        let now_ms = Self::now_ms();
        let mut conn = self.connect()?;
        let tx = conn.transaction()?;
        tx.execute(
            r#"
            UPDATE todo_subtasks
            SET completed = ?1, updated_at_ms = ?2
            WHERE id = ?3
            "#,
            params![bool_int(completed), now_ms, id],
        )?;
        tx.execute(
            r#"
            UPDATE todo_items
            SET updated_at_ms = ?1
            WHERE id = (
                SELECT item_id
                FROM todo_subtasks
                WHERE id = ?2
            )
            "#,
            params![now_ms, id],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn initialize(&self) -> Result<()> {
        let conn = self.connect()?;
        conn.execute_batch(
            r#"
            PRAGMA foreign_keys = ON;

            CREATE TABLE IF NOT EXISTS todo_items (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                title TEXT NOT NULL,
                details TEXT NOT NULL DEFAULT '',
                tags TEXT NOT NULL DEFAULT '',
                started_at_ms INTEGER NOT NULL,
                due_at_ms INTEGER,
                completed_at_ms INTEGER,
                created_at_ms INTEGER NOT NULL,
                updated_at_ms INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_todo_items_started
                ON todo_items(started_at_ms);

            CREATE INDEX IF NOT EXISTS idx_todo_items_due
                ON todo_items(due_at_ms);

            CREATE INDEX IF NOT EXISTS idx_todo_items_completed
                ON todo_items(completed_at_ms);

            CREATE TABLE IF NOT EXISTS todo_tags (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                color TEXT NOT NULL,
                created_at_ms INTEGER NOT NULL,
                updated_at_ms INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS todo_item_tags (
                item_id INTEGER NOT NULL,
                tag_id INTEGER NOT NULL,
                position INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(item_id, tag_id),
                FOREIGN KEY(item_id) REFERENCES todo_items(id) ON DELETE CASCADE,
                FOREIGN KEY(tag_id) REFERENCES todo_tags(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_todo_item_tags_tag
                ON todo_item_tags(tag_id, item_id);

            CREATE TABLE IF NOT EXISTS todo_subtasks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                item_id INTEGER NOT NULL,
                title TEXT NOT NULL,
                completed INTEGER NOT NULL DEFAULT 0,
                position INTEGER NOT NULL DEFAULT 0,
                created_at_ms INTEGER NOT NULL,
                updated_at_ms INTEGER NOT NULL,
                FOREIGN KEY(item_id) REFERENCES todo_items(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_todo_subtasks_item
                ON todo_subtasks(item_id, position, id);

            CREATE TABLE IF NOT EXISTS todo_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            "#,
        )?;
        self.migrate_text_tags(&conn)?;
        Ok(())
    }

    fn connect(&self) -> Result<Connection> {
        let conn = Connection::open(self.path.as_ref())?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        Ok(conn)
    }

    fn load_subtasks(&self, conn: &Connection, items: &mut [TodoItem]) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }

        let mut stmt = conn.prepare(
            r#"
            SELECT id, title, completed
            FROM todo_subtasks
            WHERE item_id = ?1
            ORDER BY position ASC, id ASC
            "#,
        )?;

        for item in items {
            let subtasks = stmt
                .query_map(params![item.id], |row| {
                    Ok(TodoSubtask {
                        id: row.get(0)?,
                        title: row.get(1)?,
                        completed: row.get::<_, i64>(2)? != 0,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            item.subtasks = subtasks;
        }

        Ok(())
    }

    fn load_item_tags(&self, conn: &Connection, items: &mut [TodoItem]) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }

        let mut stmt = conn.prepare(
            r#"
            SELECT t.id, t.name, t.color
            FROM todo_item_tags it
            JOIN todo_tags t ON t.id = it.tag_id
            WHERE it.item_id = ?1
            ORDER BY it.position ASC, t.name COLLATE NOCASE ASC
            "#,
        )?;

        for item in items {
            let tags = stmt
                .query_map(params![item.id], tag_from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            item.tags = tags;
        }

        Ok(())
    }

    fn migrate_text_tags(&self, conn: &Connection) -> Result<()> {
        let now_ms = Self::now_ms();
        let mut stmt = conn.prepare("SELECT id, tags FROM todo_items WHERE tags <> ''")?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        for (item_id, value) in rows {
            for (ix, tag_name) in parse_tags(&value).into_iter().enumerate() {
                let color = default_tag_color(ix);
                conn.execute(
                    r#"
                    INSERT OR IGNORE INTO todo_tags (name, color, created_at_ms, updated_at_ms)
                    VALUES (?1, ?2, ?3, ?3)
                    "#,
                    params![tag_name, color, now_ms],
                )?;
                let tag_id = conn.query_row(
                    "SELECT id FROM todo_tags WHERE name = ?1",
                    params![tag_name],
                    |row| row.get::<_, i64>(0),
                )?;
                conn.execute(
                    r#"
                    INSERT OR IGNORE INTO todo_item_tags (item_id, tag_id, position)
                    VALUES (?1, ?2, ?3)
                    "#,
                    params![item_id, tag_id, ix as i64],
                )?;
            }
            conn.execute(
                "UPDATE todo_items SET tags = '' WHERE id = ?1",
                params![item_id],
            )?;
        }

        Ok(())
    }
}

impl TodoItem {
    pub fn is_completed(&self) -> bool {
        self.completed_at_ms.is_some()
    }

    pub fn effective_end_ms(&self, now_ms: i64) -> i64 {
        self.completed_at_ms
            .unwrap_or(now_ms)
            .max(self.started_at_ms)
    }

    pub fn subtask_counts(&self) -> (usize, usize) {
        let total = self.subtasks.len();
        let done = self
            .subtasks
            .iter()
            .filter(|subtask| subtask.completed)
            .count();
        (done, total)
    }
}

pub fn parse_tags(value: &str) -> Vec<String> {
    value
        .split([',', '，', ';', '；', ' '])
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .map(ToOwned::to_owned)
        .fold(Vec::<String>::new(), |mut tags, tag| {
            if !tags.iter().any(|current| current == &tag) {
                tags.push(tag);
            }
            tags
        })
}

pub fn first_day_of_month(date: NaiveDate) -> NaiveDate {
    NaiveDate::from_ymd_opt(date.year(), date.month(), 1).expect("valid month start")
}

pub fn next_month(date: NaiveDate) -> NaiveDate {
    let first = first_day_of_month(date);
    let (year, month) = if first.month() == 12 {
        (first.year() + 1, 1)
    } else {
        (first.year(), first.month() + 1)
    };
    NaiveDate::from_ymd_opt(year, month, 1).expect("valid next month")
}

pub fn previous_month(date: NaiveDate) -> NaiveDate {
    let first = first_day_of_month(date);
    let (year, month) = if first.month() == 1 {
        (first.year() - 1, 12)
    } else {
        (first.year(), first.month() - 1)
    };
    NaiveDate::from_ymd_opt(year, month, 1).expect("valid previous month")
}

pub fn date_from_ms(timestamp_ms: i64) -> NaiveDate {
    Local
        .timestamp_millis_opt(timestamp_ms)
        .single()
        .map(|time| time.date_naive())
        .unwrap_or_else(|| Local::now().date_naive())
}

pub fn local_midnight_ms(date: NaiveDate) -> i64 {
    let start = date
        .and_hms_opt(0, 0, 0)
        .expect("midnight should always resolve");
    Local
        .from_local_datetime(&start)
        .earliest()
        .expect("local midnight should resolve")
        .timestamp_millis()
}

pub fn day_end_ms(date: NaiveDate) -> i64 {
    local_midnight_ms(date + ChronoDuration::days(1)).saturating_sub(1)
}

fn month_bounds(month: NaiveDate) -> (i64, i64) {
    let start = first_day_of_month(month);
    (
        local_midnight_ms(start),
        local_midnight_ms(next_month(start)),
    )
}

fn item_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TodoItem> {
    Ok(TodoItem {
        id: row.get(0)?,
        title: row.get(1)?,
        details: row.get(2)?,
        tags: Vec::new(),
        started_at_ms: row.get(3)?,
        due_at_ms: row.get(4)?,
        completed_at_ms: row.get(5)?,
        subtasks: Vec::new(),
    })
}

fn tag_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TodoTag> {
    Ok(TodoTag {
        id: row.get(0)?,
        name: row.get(1)?,
        color: row.get(2)?,
    })
}

fn setting_value(conn: &Connection, key: &str) -> Result<Option<String>> {
    conn.query_row(
        "SELECT value FROM todo_settings WHERE key = ?1",
        params![key],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

fn set_setting(conn: &Connection, key: &str, value: impl ToString) -> Result<()> {
    conn.execute(
        r#"
        INSERT INTO todo_settings (key, value)
        VALUES (?1, ?2)
        ON CONFLICT(key) DO UPDATE SET value = excluded.value
        "#,
        params![key, value.to_string()],
    )?;
    Ok(())
}

fn parse_window_position(value: String) -> Option<i32> {
    let position = value.parse::<i32>().ok()?;
    (position.abs() <= 100_000).then_some(position)
}

fn normalized_color(color: &str) -> String {
    let trimmed = color.trim();
    if trimmed.len() == 7
        && trimmed.starts_with('#')
        && trimmed[1..].chars().all(|ch| ch.is_ascii_hexdigit())
    {
        trimmed.to_owned()
    } else {
        default_tag_color(0).to_owned()
    }
}

fn default_tag_color(ix: usize) -> &'static str {
    const COLORS: [&str; 8] = [
        "#2563eb", "#059669", "#d97706", "#dc2626", "#7c3aed", "#0891b2", "#be185d", "#4f46e5",
    ];
    COLORS[ix % COLORS.len()]
}

fn bool_int(value: bool) -> i64 {
    if value { 1 } else { 0 }
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
