use super::*;

#[test]
fn timeline_count_and_page_return_matching_rows() {
    let dir = std::env::temp_dir().join(format!(
        "idt-db-test-{}-{}",
        std::process::id(),
        Database::now_ms()
    ));
    let database = Database::open(&dir).expect("test database should open");
    let base_ms = Database::now_ms().saturating_sub(60_000);

    let alpha = test_focus_info("alpha.exe", "Alpha Window");
    let beta = test_focus_info("beta.exe", "Beta Window");
    database
        .append_usage(&alpha, base_ms, base_ms + 10_000)
        .expect("alpha usage should insert");
    database
        .append_usage(&beta, base_ms + 20_000, base_ms + 30_000)
        .expect("beta usage should insert");

    let count = database
        .timeline_count(base_ms - 1_000, base_ms + 31_000, "", "")
        .expect("timeline count should query");
    assert_eq!(count, 2);

    let page = database
        .timeline_page(base_ms - 1_000, base_ms + 31_000, "", "", 0, 10)
        .expect("timeline page should query");
    assert_eq!(page.len(), 2);
    assert_eq!(page[0].process_name, "beta.exe");
    assert_eq!(page[1].process_name, "alpha.exe");

    let filtered = database
        .timeline_page(base_ms - 1_000, base_ms + 31_000, "alpha", "window", 0, 10)
        .expect("filtered timeline page should query");
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].process_name, "alpha.exe");

    drop(database);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn cached_usage_is_queryable_before_and_after_flush() {
    let dir = std::env::temp_dir().join(format!(
        "idt-db-cache-test-{}-{}",
        std::process::id(),
        Database::now_ms()
    ));
    let database = Database::open(&dir).expect("test database should open");
    let base_ms = Database::now_ms().saturating_sub(60_000);
    let alpha = test_focus_info("alpha.exe", "Alpha Window");

    database
        .append_usage(&alpha, base_ms, base_ms + 5_000)
        .expect("first alpha usage should cache");
    database
        .append_usage(&alpha, base_ms + 5_000, base_ms + 12_000)
        .expect("second alpha usage should extend cache");

    let cached_count = database
        .timeline_count(base_ms - 1_000, base_ms + 13_000, "", "")
        .expect("cached timeline count should query");
    assert_eq!(cached_count, 1);
    let cached_page = database
        .timeline_page(base_ms - 1_000, base_ms + 13_000, "", "", 0, 10)
        .expect("cached timeline page should query");
    assert_eq!(cached_page.len(), 1);
    assert_eq!(cached_page[0].duration_ms, 12_000);

    database
        .flush_usage_cache()
        .expect("cache should flush to disk");
    let flushed_page = database
        .timeline_page(base_ms - 1_000, base_ms + 13_000, "", "", 0, 10)
        .expect("flushed timeline page should query");
    assert_eq!(flushed_page.len(), 1);
    assert_eq!(flushed_page[0].duration_ms, 12_000);

    drop(database);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn cached_usage_merges_with_flushed_timeline_record() {
    let dir = std::env::temp_dir().join(format!(
        "idt-db-boundary-test-{}-{}",
        std::process::id(),
        Database::now_ms()
    ));
    let database = Database::open(&dir).expect("test database should open");
    let base_ms = Database::now_ms().saturating_sub(60_000);
    let alpha = test_focus_info("alpha.exe", "Alpha Window");

    database
        .append_usage(&alpha, base_ms, base_ms + 10_000)
        .expect("first alpha usage should cache");
    database
        .flush_usage_cache()
        .expect("first alpha usage should flush");
    database
        .append_usage(&alpha, base_ms + 10_000, base_ms + 15_000)
        .expect("continued alpha usage should cache");

    let count = database
        .timeline_count(base_ms - 1_000, base_ms + 16_000, "", "")
        .expect("timeline count should query");
    assert_eq!(count, 1);

    let page = database
        .timeline_page(base_ms - 1_000, base_ms + 16_000, "", "", 0, 10)
        .expect("timeline page should query");
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].started_at_ms, base_ms);
    assert_eq!(page[0].ended_at_ms, base_ms + 15_000);
    assert_eq!(page[0].duration_ms, 15_000);

    database
        .flush_usage_cache()
        .expect("continued alpha usage should flush");
    let flushed_page = database
        .timeline_page(base_ms - 1_000, base_ms + 16_000, "", "", 0, 10)
        .expect("flushed timeline page should query");
    assert_eq!(flushed_page.len(), 1);
    assert_eq!(flushed_page[0].duration_ms, 15_000);

    drop(database);
    let _ = std::fs::remove_dir_all(dir);
}

fn test_focus_info(process_name: &str, window_title: &str) -> FocusInfo {
    FocusInfo {
        process_id: 1,
        process_name: process_name.to_owned(),
        exe_path: String::new(),
        window_class: "TestWindow".to_owned(),
        window_title: window_title.to_owned(),
    }
}
