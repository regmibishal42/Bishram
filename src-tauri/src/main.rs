#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::Serialize;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{
    Emitter, Manager,
    menu::{MenuBuilder, MenuItemBuilder},
    tray::TrayIconBuilder,
    WebviewUrl, WebviewWindowBuilder,
};

const WORK_DEFAULT: u64 = 25 * 60;
const BREAK_DEFAULT: u64 = 5 * 60;
const IDLE_THRESHOLD: u64 = 180;

struct TimerState {
    work_duration: u64,
    break_duration: u64,
    remaining: u64,
    is_working: bool,
    is_running: bool,
    is_paused: bool,
    strict_mode: bool,
    idle_threshold: u64,
    cycles_completed: u32,
}

impl Default for TimerState {
    fn default() -> Self {
        Self {
            work_duration: WORK_DEFAULT,
            break_duration: BREAK_DEFAULT,
            remaining: WORK_DEFAULT,
            is_working: true,
            is_running: false,
            is_paused: false,
            strict_mode: false,
            idle_threshold: IDLE_THRESHOLD,
            cycles_completed: 0,
        }
    }
}

#[derive(Serialize, Clone)]
struct TimerSnapshot {
    remaining: u64,
    is_working: bool,
    is_running: bool,
    is_paused: bool,
    strict_mode: bool,
    cycles_completed: u32,
    work_duration: u64,
    break_duration: u64,
}

// ── Idle Detection ──────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn get_idle_seconds() -> f64 {
    type CGEventSourceStateID = i32;
    type CGEventType = u32;
    extern "C" {
        fn CGEventSourceSecondsSinceLastEventType(
            stateID: CGEventSourceStateID,
            eventType: CGEventType,
        ) -> f64;
    }
    const COMBINED: i32 = 0;
    const ANY_INPUT: u32 = 0xFFFFFFFF;
    unsafe { CGEventSourceSecondsSinceLastEventType(COMBINED, ANY_INPUT) }
}

#[cfg(target_os = "linux")]
fn get_idle_seconds() -> f64 {
    let output = std::process::Command::new("dbus-send")
        .args([
            "--session",
            "--dest=org.freedesktop.ScreenSaver",
            "--type=method_call",
            "--print-reply",
            "--reply-timeout=2000",
            "/org/freedesktop/ScreenSaver",
            "org.freedesktop.ScreenSaver.GetActiveTime",
        ])
        .output();
    if let Ok(output) = output {
        if output.status.success() {
            if let Ok(s) = String::from_utf8(output.stdout) {
                for word in s.split_whitespace() {
                    if let Ok(secs) = word.parse::<u32>() {
                        return secs as f64;
                    }
                }
            }
        }
    }

    let output = std::process::Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.gnome.Mutter.IdleMonitor",
            "--object-path",
            "/org/gnome/Mutter/IdleMonitor/Core",
            "--method",
            "org.gnome.Mutter.IdleMonitor.GetIdletime",
        ])
        .output();
    if let Ok(output) = output {
        if output.status.success() {
            if let Ok(s) = String::from_utf8(output.stdout) {
                let cleaned: String = s.chars().filter(|c| c.is_digit(10) || *c == '-').collect();
                if let Ok(us) = cleaned.parse::<i64>() {
                    return us as f64 / 1_000_000.0;
                }
            }
        }
    }
    0.0
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn get_idle_seconds() -> f64 {
    0.0
}

// ── Notifications ───────────────────────────────────────────────────────────

fn send_notification(title: &str, body: &str) {
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("notify-send")
            .args(["-a", "Bishram", title, body])
            .output();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("osascript")
            .args([
                "-e",
                &format!(
                    r#"display notification "{}" with title "{}""#,
                    body, title
                ),
            ])
            .output();
    }
}

// ── Tauri Commands ──────────────────────────────────────────────────────────

#[tauri::command]
fn get_state(state: tauri::State<Mutex<TimerState>>) -> Result<TimerSnapshot, String> {
    let s = state.lock().map_err(|e| e.to_string())?;
    Ok(TimerSnapshot {
        remaining: s.remaining,
        is_working: s.is_working,
        is_running: s.is_running,
        is_paused: s.is_paused,
        strict_mode: s.strict_mode,
        cycles_completed: s.cycles_completed,
        work_duration: s.work_duration,
        break_duration: s.break_duration,
    })
}

#[tauri::command]
fn toggle_timer(state: tauri::State<Mutex<TimerState>>) -> Result<(), String> {
    let mut s = state.lock().map_err(|e| e.to_string())?;
    s.is_running = !s.is_running;
    if s.is_running {
        s.is_paused = false;
    }
    Ok(())
}

#[tauri::command]
fn skip_break(
    app: tauri::AppHandle,
    state: tauri::State<Mutex<TimerState>>,
) -> Result<(), String> {
    {
        let s = state.lock().map_err(|e| e.to_string())?;
        if s.strict_mode {
            return Err("Cannot skip break in strict mode".into());
        }
    }
    let mut s = state.lock().map_err(|e| e.to_string())?;
    s.is_working = true;
    s.remaining = s.work_duration;
    s.is_running = true;
    s.is_paused = false;
    close_overlay(&app);
    let _ = app.emit("hide-overlay", true);
    Ok(())
}

#[tauri::command]
fn force_break(
    app: tauri::AppHandle,
    state: tauri::State<Mutex<TimerState>>,
) -> Result<(), String> {
    {
        let mut s = state.lock().map_err(|e| e.to_string())?;
        s.is_working = false;
        s.remaining = s.break_duration;
        s.is_running = true;
        s.is_paused = false;
    }
    let _ = app.emit("force-break", true);
    let _ = app.emit("show-overlay", true);
    let _ = app.emit("timer-tick", 0);
    create_overlay(&app);
    send_notification("Bishram", "Forced break time. Step away and breathe.");
    Ok(())
}

#[tauri::command]
fn update_settings(
    state: tauri::State<Mutex<TimerState>>,
    work_duration: u64,
    break_duration: u64,
    strict_mode: bool,
) -> Result<(), String> {
    let mut s = state.lock().map_err(|e| e.to_string())?;
    s.work_duration = work_duration.min(3600).max(60);
    s.break_duration = break_duration.min(3600).max(10);
    s.strict_mode = strict_mode;
    Ok(())
}

#[tauri::command]
fn reset_timer(state: tauri::State<Mutex<TimerState>>) -> Result<(), String> {
    let mut s = state.lock().map_err(|e| e.to_string())?;
    s.remaining = s.work_duration;
    s.is_working = true;
    s.is_running = false;
    s.is_paused = false;
    Ok(())
}

// ── Window Management ───────────────────────────────────────────────────────

fn create_overlay(app: &tauri::AppHandle) {
    if app.get_webview_window("overlay").is_some() {
        return;
    }
    if let Ok(window) = WebviewWindowBuilder::new(
        app,
        "overlay",
        WebviewUrl::App(PathBuf::from("index.html")),
    )
    .title("")
    .decorations(false)
    .transparent(true)
    .fullscreen(true)
    .always_on_top(true)
    .skip_taskbar(true)
    .build()
    {
        let _ = window.set_focus();
    }
}

fn close_overlay(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("overlay") {
        let _ = window.close();
    }
}

// ── Background Timer Loop ──────────────────────────────────────────────────

async fn timer_loop(app: tauri::AppHandle) {
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    loop {
        interval.tick().await;
        let action = {
            let state = app.state::<Mutex<TimerState>>();
            let mut s = match state.lock() {
                Ok(s) => s,
                Err(_) => continue,
            };
            if !s.is_running {
                continue;
            }
            if s.is_paused {
                continue;
            }
            let idle = get_idle_seconds();
            if idle > s.idle_threshold as f64 && s.is_working {
                s.is_paused = true;
                send_notification("Bishram", "Detected idle — timer paused.");
                let _ = app.emit("idle-paused", true);
                continue;
            }
            if s.remaining > 0 {
                s.remaining -= 1;
                if s.remaining % 5 == 0 || s.remaining <= 10 {
                    let _ = app.emit("timer-tick", s.remaining);
                }
                continue;
            }
            if s.is_working {
                s.is_working = false;
                s.remaining = s.break_duration;
                TimerAction::WorkComplete
            } else {
                s.is_working = true;
                s.remaining = s.work_duration;
                s.cycles_completed += 1;
                TimerAction::BreakComplete
            }
        };
        match action {
            TimerAction::WorkComplete => {
                send_notification("Bishram", "Focus session complete — time for a mindful break.");
                create_overlay(&app);
                let _ = app.emit("show-overlay", true);
                let _ = app.emit("timer-tick", 0);
            }
            TimerAction::BreakComplete => {
                send_notification("Bishram", "Break over — let's return to focus.");
                close_overlay(&app);
                let _ = app.emit("hide-overlay", true);
                let _ = app.emit("timer-tick", 0);
            }
        }
    }
}

enum TimerAction {
    WorkComplete,
    BreakComplete,
}

// ── Tray Setup ──────────────────────────────────────────────────────────────

fn setup_tray(app: &tauri::AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let toggle = MenuItemBuilder::with_id("toggle", "Start Focus").build(app)?;
    let toggle_clone = toggle.clone();
    let force = MenuItemBuilder::with_id("force", "Force Break Now").build(app)?;
    let skip = MenuItemBuilder::with_id("skip", "Skip Next Break").build(app)?;
    let sep = tauri::menu::PredefinedMenuItem::separator(app)?;
    let quit = MenuItemBuilder::with_id("quit", "Quit Bishram").build(app)?;

    let menu = MenuBuilder::new(app)
        .items(&[&toggle, &force, &skip, &sep, &quit])
        .build()?;

    let icon_data = std::fs::read(
        std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default())
            .join("icons")
            .join("32x32.png"),
    )
    .unwrap_or_else(|_| Vec::from(include_bytes!("../icons/32x32.png").as_slice()));

    let icon = tauri::image::Image::from_bytes(&icon_data)?;

    TrayIconBuilder::with_id("main")
        .icon(icon)
        .tooltip("Bishram")
        .menu(&menu)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "toggle" => {
                let state = app.state::<Mutex<TimerState>>();
                let mut s = state.lock().unwrap();
                s.is_running = !s.is_running;
                if s.is_running {
                    s.is_paused = false;
                }
                let label = if s.is_running { "Pause" } else { "Start Focus" };
                let _ = toggle_clone.set_text(label);
                if s.is_running && !s.is_working {
                    drop(s);
                    create_overlay(app);
                } else if !s.is_running {
                    drop(s);
                    close_overlay(app);
                }
            }
            "force" => {
                let state = app.state::<Mutex<TimerState>>();
                let mut s = state.lock().unwrap();
                s.is_working = false;
                s.remaining = s.break_duration;
                s.is_running = true;
                s.is_paused = false;
                drop(s);
                create_overlay(app);
                let _ = app.emit("force-break", true);
                let _ = app.emit("show-overlay", true);
                send_notification("Bishram", "Forced break. Step away and breathe.");
            }
            "skip" => {
                if app.get_webview_window("overlay").is_some() {
                    let state = app.state::<Mutex<TimerState>>();
                    let mut s = state.lock().unwrap();
                    if !s.strict_mode {
                        s.is_working = true;
                        s.remaining = s.work_duration;
                        s.is_running = true;
                        s.is_paused = false;
                        drop(s);
                        close_overlay(app);
                        let _ = app.emit("hide-overlay", true);
                    }
                }
            }
            "quit" => {
                app.exit(0);
            }
            _ => {}
        })
        .build(app)?;

    Ok(())
}

// ── Entrypoint ──────────────────────────────────────────────────────────────

fn main() {
    tauri::Builder::default()
        .manage(Mutex::new(TimerState::default()))
        .invoke_handler(tauri::generate_handler![
            get_state,
            toggle_timer,
            skip_break,
            force_break,
            update_settings,
            reset_timer,
        ])
        .setup(|app| {
            setup_tray(app.handle())?;
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                timer_loop(handle).await;
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Bishram");
}
