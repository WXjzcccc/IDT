use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

use crate::{
    db::{Database, FocusInfo, MAX_INTERVAL_MS, MIN_INTERVAL_MS},
    focus,
};

pub struct TrackerHandle {
    stop: Arc<AtomicBool>,
}

impl TrackerHandle {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for TrackerHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn start(
    database: Database,
    interval_ms: Arc<AtomicU64>,
    cache_flush_interval_ms: Arc<AtomicU64>,
) -> TrackerHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();

    thread::Builder::new()
        .name("idt-activity-tracker".to_owned())
        .spawn(move || run_tracker(database, interval_ms, cache_flush_interval_ms, thread_stop))
        .expect("activity tracker thread should start");

    TrackerHandle { stop }
}

fn run_tracker(
    database: Database,
    interval_ms: Arc<AtomicU64>,
    cache_flush_interval_ms: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
) {
    let mut previous: Option<FocusInfo> = focus::current_foreground();
    let mut previous_at_ms = Database::now_ms();
    let mut last_flush_ms = previous_at_ms;

    while !stop.load(Ordering::Relaxed) {
        let sleep_ms = interval_ms
            .load(Ordering::Relaxed)
            .clamp(MIN_INTERVAL_MS, MAX_INTERVAL_MS);
        thread::sleep(Duration::from_millis(sleep_ms));

        let now_ms = Database::now_ms();
        let locked = focus::is_workstation_locked();
        let current = if locked {
            None
        } else {
            focus::current_foreground()
        };

        if !locked && let Some(info) = previous.as_ref() {
            if let Err(error) = database.append_usage(info, previous_at_ms, now_ms) {
                eprintln!("failed to cache activity sample: {error:#}");
            }
        }

        let flush_interval_ms = cache_flush_interval_ms.load(Ordering::Relaxed);
        if now_ms.saturating_sub(last_flush_ms) >= flush_interval_ms.min(i64::MAX as u64) as i64 {
            if let Err(error) = database.flush_usage_cache() {
                eprintln!("failed to flush activity cache: {error:#}");
            } else {
                last_flush_ms = now_ms;
            }
        }

        previous = current;
        previous_at_ms = now_ms;
    }

    if let Err(error) = database.flush_usage_cache() {
        eprintln!("failed to flush activity cache before tracker exit: {error:#}");
    }
}
