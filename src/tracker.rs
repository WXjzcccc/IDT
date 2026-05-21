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

pub fn start(database: Database, interval_ms: Arc<AtomicU64>) -> TrackerHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();

    thread::Builder::new()
        .name("idt-activity-tracker".to_owned())
        .spawn(move || run_tracker(database, interval_ms, thread_stop))
        .expect("activity tracker thread should start");

    TrackerHandle { stop }
}

fn run_tracker(database: Database, interval_ms: Arc<AtomicU64>, stop: Arc<AtomicBool>) {
    let mut previous: Option<FocusInfo> = focus::current_foreground();
    let mut previous_at_ms = Database::now_ms();

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
                eprintln!("failed to store activity sample: {error:#}");
            }
        }

        previous = current;
        previous_at_ms = now_ms;
    }
}
