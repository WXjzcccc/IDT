#![cfg_attr(not(target_os = "windows"), allow(dead_code))]
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

#[cfg(not(target_os = "windows"))]
compile_error!("IDT is Windows-only.");

#[cfg(target_os = "windows")]
mod app_assets;
#[cfg(target_os = "windows")]
mod app_icon;
#[cfg(target_os = "windows")]
mod db;
#[cfg(target_os = "windows")]
mod focus;
#[cfg(target_os = "windows")]
mod process_icon;
#[cfg(target_os = "windows")]
mod single_instance;
#[cfg(target_os = "windows")]
mod startup;
#[cfg(target_os = "windows")]
mod todo_db;
#[cfg(target_os = "windows")]
mod todo_ui;
#[cfg(target_os = "windows")]
mod tracker;
#[cfg(target_os = "windows")]
mod tray;
#[cfg(target_os = "windows")]
mod ui;
#[cfg(target_os = "windows")]
mod window_util;

#[cfg(target_os = "windows")]
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering},
};

#[cfg(target_os = "windows")]
use anyhow::Result;
#[cfg(target_os = "windows")]
use gpui::{
    App, AppContext as _, Application, Bounds, SharedString, TitlebarOptions,
    WindowBackgroundAppearance, WindowBounds, WindowOptions, px, size,
};
#[cfg(target_os = "windows")]
use gpui_component::{Root, Theme, ThemeMode};

#[cfg(target_os = "windows")]
fn main() {
    if let Err(error) = run() {
        eprintln!("IDT failed to start: {error:#}");
    }
}

#[cfg(target_os = "windows")]
fn run() -> Result<()> {
    let silent_launch = startup::is_silent_launch();
    let Some(_single_instance) = single_instance::acquire_or_activate(!silent_launch)? else {
        return Ok(());
    };

    let database = db::Database::open_default()?;
    let todo_database = todo_db::TodoDatabase::open_default()?;
    let app_settings = database.app_settings()?;
    let window_size = database.get_window_size()?.unwrap_or_default();
    let interval = database.get_interval_ms()?;
    let cache_flush_interval = database.get_cache_flush_interval_ms()?;
    let interval_ms = Arc::new(AtomicU64::new(interval));
    let cache_flush_interval_ms = Arc::new(AtomicU64::new(cache_flush_interval));
    let exit_requested = Arc::new(AtomicBool::new(false));
    let target_hwnd = Arc::new(AtomicIsize::new(0));
    let tracker = tracker::start(
        database.clone(),
        interval_ms.clone(),
        cache_flush_interval_ms.clone(),
    );
    if startup::is_enabled().unwrap_or(false) {
        if let Err(error) = startup::set_enabled(true, app_settings.silent_start) {
            eprintln!("failed to sync startup command: {error:#}");
        }
    }

    let app = Application::new().with_assets(app_assets::Assets);
    app.run(move |cx: &mut App| {
        gpui_component::init(cx);
        gpui_component::set_locale("zh-CN");
        Theme::change(
            if app_settings.theme.is_dark() {
                ThemeMode::Dark
            } else {
                ThemeMode::Light
            },
            None,
            cx,
        );

        let bounds = Bounds::centered(
            None,
            size(px(window_size.width as f32), px(window_size.height as f32)),
            cx,
        );
        let window_options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(
                px(db::MIN_WINDOW_WIDTH as f32),
                px(db::MIN_WINDOW_HEIGHT as f32),
            )),
            titlebar: Some(TitlebarOptions {
                title: Some(SharedString::from("I Did Today")),
                appears_transparent: true,
                ..Default::default()
            }),
            window_background: WindowBackgroundAppearance::Transparent,
            focus: !silent_launch,
            show: !silent_launch,
            ..Default::default()
        };

        let close_hwnd = target_hwnd.clone();
        let close_exit = exit_requested.clone();
        let close_database = database.clone();
        let tray_hwnd = target_hwnd.clone();
        let tray_exit = exit_requested.clone();

        cx.open_window(window_options, |window, cx| {
            window.set_window_title("I Did Today");
            if let Some(hwnd) = window_util::hwnd_from_window(window) {
                target_hwnd.store(hwnd, Ordering::Relaxed);
                app_icon::apply_window_icons(hwnd);
                if silent_launch {
                    window_util::resize_window(window, window_size.width, window_size.height);
                    tray::hide_window(hwnd);
                }
            }

            tray::start(tray_hwnd, tray_exit);

            let dashboard = cx.new(|cx| {
                ui::Dashboard::new(
                    database,
                    interval_ms,
                    cache_flush_interval_ms,
                    exit_requested,
                    target_hwnd,
                    tracker,
                    todo_database,
                    silent_launch,
                    window,
                    cx,
                )
            });
            let close_dashboard = dashboard.clone();

            window.on_window_should_close(cx, move |window, cx| {
                close_dashboard.update(cx, |dashboard, _| {
                    dashboard.persist_window_size(window);
                });
                match close_database
                    .get_close_behavior()
                    .unwrap_or(db::CloseBehavior::HideToTray)
                {
                    db::CloseBehavior::Minimize => {
                        tray::minimize_window(close_hwnd.load(Ordering::Relaxed));
                        false
                    }
                    db::CloseBehavior::HideToTray => {
                        close_dashboard.update(cx, |dashboard, cx| {
                            dashboard.release_view_data(cx);
                        });
                        tray::hide_window(close_hwnd.load(Ordering::Relaxed));
                        false
                    }
                    db::CloseBehavior::Exit => {
                        close_exit.store(true, Ordering::Relaxed);
                        true
                    }
                }
            });

            cx.new(|cx| Root::new(dashboard, window, cx))
        })
        .expect("failed to open IDT window");
    });

    Ok(())
}
