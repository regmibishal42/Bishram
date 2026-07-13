#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{
    menu::{MenuBuilder, MenuItem, MenuItemBuilder},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent, Wry,
};

const WORK_DEFAULT: u64 = 25 * 60;
const BREAK_DEFAULT: u64 = 2 * 60;
const IDLE_THRESHOLD: u64 = 180;
const SNOOZE_SECONDS: u64 = 120;
const MAX_HISTORY: usize = 500;

struct TimerState {
    work_duration: u64,
    break_duration: u64,
    remaining: u64,
    is_working: bool,
    is_running: bool,
    is_paused: bool,
    paused_by_idle: bool,
    strict_mode: bool,
    sound_enabled: bool,
    launch_at_login: bool,
    idle_threshold: u64,
    is_snoozing: bool,
    snoozed_break_remaining: u64,
    cycle_history: Vec<u64>,
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
            paused_by_idle: false,
            strict_mode: false,
            sound_enabled: true,
            launch_at_login: false,
            idle_threshold: IDLE_THRESHOLD,
            is_snoozing: false,
            snoozed_break_remaining: 0,
            cycle_history: Vec::new(),
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
    sound_enabled: bool,
    launch_at_login: bool,
    is_snoozing: bool,
    cycles_completed: u32,
    work_duration: u64,
    break_duration: u64,
    cycle_history: Vec<u64>,
}

#[derive(Serialize, Deserialize, Clone)]
struct PersistedState {
    work_duration: u64,
    break_duration: u64,
    strict_mode: bool,
    sound_enabled: bool,
    launch_at_login: bool,
    cycle_history: Vec<u64>,
}

impl Default for PersistedState {
    fn default() -> Self {
        Self {
            work_duration: WORK_DEFAULT,
            break_duration: BREAK_DEFAULT,
            strict_mode: false,
            sound_enabled: true,
            launch_at_login: false,
            cycle_history: Vec::new(),
        }
    }
}

struct TrayHandles {
    toggle: MenuItem<Wry>,
    skip: MenuItem<Wry>,
}

fn fmt_time(secs: u64) -> String {
    let m = secs / 60;
    let s = secs % 60;
    format!("{:02}:{:02}", m, s)
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ── Persistence ──────────────────────────────────────────────────────────────

fn state_file_path(app: &tauri::AppHandle) -> Option<PathBuf> {
    let dir = app.path().app_data_dir().ok()?;
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("bishram-state.json"))
}

fn load_persisted(app: &tauri::AppHandle) -> PersistedState {
    state_file_path(app)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_persisted(app: &tauri::AppHandle, s: &TimerState) {
    let persisted = PersistedState {
        work_duration: s.work_duration,
        break_duration: s.break_duration,
        strict_mode: s.strict_mode,
        sound_enabled: s.sound_enabled,
        launch_at_login: s.launch_at_login,
        cycle_history: s.cycle_history.clone(),
    };
    if let Some(path) = state_file_path(app) {
        if let Ok(json) = serde_json::to_string_pretty(&persisted) {
            let _ = std::fs::write(path, json);
        }
    }
}

fn push_cycle_completion(s: &mut TimerState) {
    s.cycle_history.push(now_unix());
    if s.cycle_history.len() > MAX_HISTORY {
        let excess = s.cycle_history.len() - MAX_HISTORY;
        s.cycle_history.drain(0..excess);
    }
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

// ── Launch at Login ──────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn set_autostart(enabled: bool) -> std::io::Result<()> {
    let home = std::env::var("HOME")
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::NotFound, "HOME not set"))?;
    let agents_dir = PathBuf::from(&home).join("Library/LaunchAgents");
    std::fs::create_dir_all(&agents_dir)?;
    let plist_path = agents_dir.join("com.bishram.app.plist");

    if enabled {
        let exe = std::env::current_exe()?;
        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.bishram.app</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
</dict>
</plist>
"#,
            exe.display()
        );
        std::fs::write(&plist_path, plist)?;
        let _ = std::process::Command::new("launchctl")
            .args(["load", "-w", &plist_path.to_string_lossy()])
            .output();
    } else {
        let _ = std::process::Command::new("launchctl")
            .args(["unload", "-w", &plist_path.to_string_lossy()])
            .output();
        let _ = std::fs::remove_file(&plist_path);
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn set_autostart(enabled: bool) -> std::io::Result<()> {
    let home = std::env::var("HOME")
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::NotFound, "HOME not set"))?;
    let autostart_dir = PathBuf::from(&home).join(".config/autostart");
    std::fs::create_dir_all(&autostart_dir)?;
    let desktop_path = autostart_dir.join("bishram.desktop");

    if enabled {
        let exe = std::env::current_exe()?;
        let entry = format!(
            "[Desktop Entry]\nType=Application\nName=Bishram\nExec={}\nX-GNOME-Autostart-enabled=true\n",
            exe.display()
        );
        std::fs::write(&desktop_path, entry)?;
    } else {
        let _ = std::fs::remove_file(&desktop_path);
    }
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn set_autostart(_enabled: bool) -> std::io::Result<()> {
    Ok(())
}

// ── Window Management ────────────────────────────────────────────────────────

fn create_overlay(app: &tauri::AppHandle) {
    if app.get_webview_window("overlay").is_some() {
        return;
    }
    if let Ok(window) = WebviewWindowBuilder::new(
        app,
        "overlay",
        WebviewUrl::App(PathBuf::from("overlay.html")),
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

/// Fades the overlay out in the UI first, then destroys the window once the
/// CSS transition has had time to finish (closing immediately, as the old
/// code did, skipped the fade entirely).
fn schedule_hide_overlay(app: &tauri::AppHandle) {
    let _ = app.emit("hide-overlay", true);
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(850)).await;
        close_overlay(&app2);
    });
}

fn create_dashboard(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("dashboard") {
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }
    if let Ok(window) = WebviewWindowBuilder::new(
        app,
        "dashboard",
        WebviewUrl::App(PathBuf::from("dashboard.html")),
    )
    .title("Bishram")
    .inner_size(420.0, 680.0)
    .min_inner_size(360.0, 560.0)
    .resizable(true)
    .center()
    .build()
    {
        let _ = window.set_focus();
        let hide_target = window.clone();
        window.on_window_event(move |event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = hide_target.hide();
            }
        });
    }
}

// ── Shared action handlers (used by both tray menu clicks and JS commands) ──

fn do_toggle_timer(app: &tauri::AppHandle) {
    let state = app.state::<Mutex<TimerState>>();
    let (now_running, now_on_break) = {
        let mut s = state.lock().unwrap();
        s.is_running = !s.is_running;
        if s.is_running {
            s.is_paused = false;
            s.paused_by_idle = false;
        }
        (s.is_running, !s.is_working)
    };
    if now_running && now_on_break {
        create_overlay(app);
        let _ = app.emit("show-overlay", true);
    } else if !now_running {
        close_overlay(app);
    }
    sync_tray(app);
}

fn do_force_break(app: &tauri::AppHandle) {
    let remaining = {
        let state = app.state::<Mutex<TimerState>>();
        let mut s = state.lock().unwrap();
        s.is_working = false;
        s.is_snoozing = false;
        s.remaining = s.break_duration;
        s.is_running = true;
        s.is_paused = false;
        s.paused_by_idle = false;
        s.remaining
    };
    create_overlay(app);
    let _ = app.emit("force-break", true);
    let _ = app.emit("show-overlay", true);
    let _ = app.emit("timer-tick", remaining);
    send_notification("Bishram", "Forced break time. Step away and breathe.");
    sync_tray(app);
}

fn do_skip_break(app: &tauri::AppHandle) -> Result<(), String> {
    let state = app.state::<Mutex<TimerState>>();
    {
        let s = state.lock().map_err(|e| e.to_string())?;
        if s.strict_mode {
            return Err("Cannot skip break in strict mode".into());
        }
        if s.is_working {
            return Err("Not currently on a break".into());
        }
    }
    {
        let mut s = state.lock().map_err(|e| e.to_string())?;
        s.is_working = true;
        s.is_snoozing = false;
        s.remaining = s.work_duration;
        s.is_running = true;
        s.is_paused = false;
        s.paused_by_idle = false;
    }
    schedule_hide_overlay(app);
    sync_tray(app);
    Ok(())
}

fn do_snooze_break(app: &tauri::AppHandle) -> Result<(), String> {
    let state = app.state::<Mutex<TimerState>>();
    {
        let s = state.lock().map_err(|e| e.to_string())?;
        if s.strict_mode {
            return Err("Cannot snooze in strict mode".into());
        }
        if s.is_working {
            return Err("Not currently on a break".into());
        }
    }
    {
        let mut s = state.lock().map_err(|e| e.to_string())?;
        s.snoozed_break_remaining = s.remaining;
        s.is_snoozing = true;
        s.is_working = true;
        s.remaining = SNOOZE_SECONDS;
        s.is_running = true;
        s.is_paused = false;
    }
    schedule_hide_overlay(app);
    sync_tray(app);
    Ok(())
}

fn do_reset_timer(app: &tauri::AppHandle) {
    let state = app.state::<Mutex<TimerState>>();
    let mut s = state.lock().unwrap();
    s.remaining = s.work_duration;
    s.is_working = true;
    s.is_running = false;
    s.is_paused = false;
    s.paused_by_idle = false;
    s.is_snoozing = false;
    drop(s);
    sync_tray(app);
}

/// Keeps the tray tooltip and "Start Focus"/"Pause" label honest. The old
/// code only touched these from inside the timer loop's countdown branch,
/// so pausing, idling, or transitioning phases left stale text indefinitely.
fn sync_tray(app: &tauri::AppHandle) {
    let state = app.state::<Mutex<TimerState>>();
    let (tooltip, toggle_label, strict) = {
        let s = match state.lock() {
            Ok(s) => s,
            Err(_) => return,
        };
        let tooltip = if s.is_paused {
            if s.paused_by_idle {
                "Paused (stepped away)".to_string()
            } else {
                "Paused".to_string()
            }
        } else if !s.is_running {
            "Bishram — idle".to_string()
        } else if s.is_snoozing {
            format!("Snoozed {}", fmt_time(s.remaining))
        } else if s.is_working {
            format!("Focus {}", fmt_time(s.remaining))
        } else {
            format!("Break {}", fmt_time(s.remaining))
        };
        let toggle_label = if s.is_running { "Pause" } else { "Start Focus" };
        (tooltip, toggle_label, s.strict_mode)
    };

    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(&tooltip));
    }
    if let Some(handles) = app.try_state::<TrayHandles>() {
        let _ = handles.toggle.set_text(toggle_label);
        let _ = handles.skip.set_enabled(!strict);
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
        sound_enabled: s.sound_enabled,
        launch_at_login: s.launch_at_login,
        is_snoozing: s.is_snoozing,
        cycles_completed: s.cycle_history.len() as u32,
        work_duration: s.work_duration,
        break_duration: s.break_duration,
        cycle_history: s.cycle_history.clone(),
    })
}

#[tauri::command]
fn toggle_timer(app: tauri::AppHandle) -> Result<(), String> {
    do_toggle_timer(&app);
    Ok(())
}

#[tauri::command]
fn skip_break(app: tauri::AppHandle) -> Result<(), String> {
    do_skip_break(&app)
}

#[tauri::command]
fn snooze_break(app: tauri::AppHandle) -> Result<(), String> {
    do_snooze_break(&app)
}

#[tauri::command]
fn force_break(app: tauri::AppHandle) -> Result<(), String> {
    do_force_break(&app);
    Ok(())
}

#[tauri::command]
fn update_settings(
    app: tauri::AppHandle,
    state: tauri::State<Mutex<TimerState>>,
    work_duration: u64,
    break_duration: u64,
    strict_mode: bool,
    sound_enabled: bool,
) -> Result<(), String> {
    let mut s = state.lock().map_err(|e| e.to_string())?;
    s.work_duration = (work_duration * 60).clamp(60, 7200);
    s.break_duration = (break_duration * 60).clamp(10, 3600);
    s.strict_mode = strict_mode;
    s.sound_enabled = sound_enabled;
    if !s.is_running {
        s.remaining = if s.is_working {
            s.work_duration
        } else {
            s.break_duration
        };
    }
    save_persisted(&app, &s);
    Ok(())
}

#[tauri::command]
fn toggle_autostart(
    app: tauri::AppHandle,
    state: tauri::State<Mutex<TimerState>>,
    enabled: bool,
) -> Result<(), String> {
    set_autostart(enabled).map_err(|e| e.to_string())?;
    let mut s = state.lock().map_err(|e| e.to_string())?;
    s.launch_at_login = enabled;
    save_persisted(&app, &s);
    Ok(())
}

#[tauri::command]
fn reset_timer(app: tauri::AppHandle) -> Result<(), String> {
    do_reset_timer(&app);
    Ok(())
}

// ── Background Timer Loop ────────────────────────────────────────────────────

enum TimerAction {
    WorkComplete,
    SnoozeComplete,
    BreakComplete,
}

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
            let mut action = None;

            if s.is_running {
                let idle = get_idle_seconds();

                if s.is_working && !s.is_paused && idle > s.idle_threshold as f64 {
                    s.is_paused = true;
                    s.paused_by_idle = true;
                    send_notification("Bishram", "Detected idle — timer paused.");
                    let _ = app.emit("idle-paused", true);
                } else if s.paused_by_idle && idle <= s.idle_threshold as f64 {
                    s.is_paused = false;
                    s.paused_by_idle = false;
                    let _ = app.emit("resumed", true);
                }

                if !s.is_paused {
                    if s.remaining > 0 {
                        s.remaining -= 1;
                        let _ = app.emit("timer-tick", s.remaining);
                    } else if s.is_working {
                        if s.is_snoozing {
                            s.is_snoozing = false;
                            s.is_working = false;
                            s.remaining = s.snoozed_break_remaining.max(1);
                            action = Some(TimerAction::SnoozeComplete);
                        } else {
                            s.is_working = false;
                            s.remaining = s.break_duration;
                            action = Some(TimerAction::WorkComplete);
                        }
                    } else {
                        s.is_working = true;
                        s.remaining = s.work_duration;
                        push_cycle_completion(&mut s);
                        action = Some(TimerAction::BreakComplete);
                    }
                }
            }

            action
        };

        sync_tray(&app);

        match action {
            Some(TimerAction::WorkComplete) => {
                send_notification(
                    "Bishram",
                    "Focus session complete — time for a mindful break.",
                );
                create_overlay(&app);
                let app2 = app.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(600)).await;
                    let _ = app2.emit("show-overlay", true);
                });
            }
            Some(TimerAction::SnoozeComplete) => {
                send_notification("Bishram", "Snooze over — back to your break.");
                create_overlay(&app);
                let app2 = app.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(600)).await;
                    let _ = app2.emit("show-overlay", true);
                });
            }
            Some(TimerAction::BreakComplete) => {
                send_notification("Bishram", "Break over — let's return to focus.");
                schedule_hide_overlay(&app);
                let state = app.state::<Mutex<TimerState>>();
                let s = state.lock().unwrap();
                save_persisted(&app, &s);
            }
            None => {}
        }
    }
}

// ── Tray Setup ────────────────────────────────────────────────────────────────

fn setup_tray(app: &tauri::AppHandle) -> Result<TrayHandles, Box<dyn std::error::Error>> {
    let open_item = MenuItemBuilder::with_id("open", "Open Bishram").build(app)?;
    let toggle = MenuItemBuilder::with_id("toggle", "Start Focus").build(app)?;
    let force = MenuItemBuilder::with_id("force", "Force Break Now").build(app)?;
    let skip = MenuItemBuilder::with_id("skip", "Skip Next Break").build(app)?;
    let sep = tauri::menu::PredefinedMenuItem::separator(app)?;
    let quit = MenuItemBuilder::with_id("quit", "Quit Bishram").build(app)?;

    let menu = MenuBuilder::new(app)
        .items(&[&open_item, &toggle, &force, &skip, &sep, &quit])
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
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => create_dashboard(app),
            "toggle" => do_toggle_timer(app),
            "force" => do_force_break(app),
            "skip" => {
                let _ = do_skip_break(app);
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                create_dashboard(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(TrayHandles { toggle, skip })
}

// ── Entrypoint ────────────────────────────────────────────────────────────────

fn main() {
    tauri::Builder::default()
        .manage(Mutex::new(TimerState::default()))
        .invoke_handler(tauri::generate_handler![
            get_state,
            toggle_timer,
            skip_break,
            snooze_break,
            force_break,
            update_settings,
            toggle_autostart,
            reset_timer,
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            let persisted = load_persisted(&handle);
            {
                let state = app.state::<Mutex<TimerState>>();
                let mut s = state.lock().unwrap();
                s.work_duration = persisted.work_duration;
                s.break_duration = persisted.break_duration;
                s.remaining = s.work_duration;
                s.strict_mode = persisted.strict_mode;
                s.sound_enabled = persisted.sound_enabled;
                s.launch_at_login = persisted.launch_at_login;
                s.cycle_history = persisted.cycle_history;
            }

            let handles = setup_tray(app.handle())?;
            app.manage(handles);

            let loop_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                timer_loop(loop_handle).await;
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Bishram");
}
