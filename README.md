# Bishram — A Mindful Break Ritual

**Bishram** (meaning "rest" in Nepali) is a desktop application that sits quietly in your system tray and reminds you to take mindful breaks during deep focus sessions. It features a beautiful, full-screen glassmorphic overlay with breathing exercises, micro-stretches, and inspirational quotes.

![macOS](https://img.shields.io/badge/platform-macOS-lightgrey) ![Linux](https://img.shields.io/badge/platform-Linux-orange) ![Rust](https://img.shields.io/badge/built%20with-Rust-ff69b4)

## Features

- **Pomodoro-style timer** — Default 25 min focus / 5 min break cycles
- **Full-screen break overlay** — Frameless, transparent, always-on-top glassmorphic UI
- **Breathing ring animation** — A glowing SVG circle that expands and contracts
- **3 micro-stretch exercises** — Neck roll, shoulder roll, and wrist stretch with SVG illustrations
- **System tray** — Start/Pause, Force Break, Skip, and Quit without leaving your workspace
- **Idle detection** — Automatically pauses the timer when you step away (3 min threshold)
- **Strict mode** — Lock yourself in; breaks cannot be skipped
- **Customizable durations** — Adjust focus and break times from the settings panel
- **Rotating quotes** — Mindful quotes displayed during breaks

## Prerequisites

### macOS

1. Install Xcode Command Line Tools:
   ```bash
   xcode-select --install
   ```

2. Install Rust (if you don't have it):
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   source "$HOME/.cargo/env"
   ```

### Linux (Ubuntu / Debian)

1. Install system dependencies:
   ```bash
   sudo apt update
   sudo apt install -y \
     build-essential \
     curl \
     wget \
     file \
     libssl-dev \
     libgtk-3-dev \
     libayatana-appindicator3-dev \
     librsvg2-dev \
     libwebkit2gtk-4.0-dev \
     pkg-config \
     dbus
   ```

2. Install Rust:
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   source "$HOME/.cargo/env"
   ```

### Fedora / RHEL

```bash
sudo dnf install gcc-c++ curl wget file openssl-devel gtk3-devel \
  libappindicator-gtk3-devel librsvg2-devel webkit2gtk4.0-devel \
  pkg-config dbus-devel
```

### Arch Linux

```bash
sudo pacman -S base-devel curl wget file openssl gtk3 libappindicator-gtk3 \
  librsvg webkit2gtk pkg-config dbus
```

## Quick Start

### Run in development mode

```bash
# Navigate into the project
cd bishram-app

# Run with hot reload
cargo tauri dev
```

> **Note for macOS**: The first time you run, macOS may ask for accessibility permissions for the app. This is needed for idle detection. Go to **System Settings → Privacy & Security → Accessibility** and enable Bishram.

> **Note for Linux**: If the tray icon doesn't appear, install `libayatana-appindicator` (already in the prerequisites above) and restart your session.

### Build the production app

```bash
cargo tauri build
```

After building:

- **macOS**: Find `Bishram.app` in `src-tauri/target/release/bundle/macos/`. Drag it to your Applications folder.
- **Linux**: Find `bishram_x.x.x_amd64.deb` in `src-tauri/target/release/bundle/deb/`. Install with:
  ```bash
  sudo dpkg -i src-tauri/target/release/bundle/deb/bishram_*.deb
  ```
- **All platforms** (AppImage): Find `bishram_x.x.x_amd64.AppImage` in `src-tauri/target/release/bundle/appimage/`. Make it executable and run:
  ```bash
  chmod +x bishram_*.AppImage
  ./bishram_*.AppImage
  ```

## How to Use

1. **Launch Bishram** — It starts in your system tray (menu bar on macOS, notification area on Linux).
2. **Start a focus session** — Click the tray icon → **Start Focus**. The timer begins counting down in the background.
3. **During focus** — Bishram monitors your activity. If you step away for more than 3 minutes, the timer pauses automatically. When you return, check the tray menu and click **Start Focus** to resume.
4. **Break time!** — When the focus timer ends, a full-screen overlay appears with:
   - A **breathing ring** — Follow it to center yourself
   - A **break timer** — Shows how long remains
   - **3 stretches** — Neck roll, shoulder roll, and wrist stretch
   - A **mindful quote** — Changes each break
   - **Skip Break** button (hidden in Strict Mode)
5. **After the break** — The overlay closes and a new focus cycle begins automatically.
6. **Any time** — Use the tray menu:
   - **Start Focus / Pause** — Toggle the timer
   - **Force Break Now** — Start a break immediately
   - **Skip Next Break** — Skip the current break (not available in Strict Mode)
   - **Quit Bishram** — Exit the app

## Settings

Click **Settings** on the overlay or press `S` to open the settings panel:

| Setting | Description |
|---|---|
| **Focus Duration** | 1–120 minutes (default: 25) |
| **Break Duration** | 1–30 minutes (default: 5) |
| **Strict Mode** | When ON, breaks cannot be skipped. The Esc key also won't close the overlay. |
| **Cycles Completed** | Shows how many focus/break cycles you've finished this session. |

Press `Esc` or click the X button to close the settings panel.

## Keyboard Shortcuts

| Key | Action |
|---|---|
| `Esc` | Close settings / Skip break (if not strict) |
| `S` | Toggle settings panel |

## Idle Detection Details

Bishram automatically pauses your focus timer when it detects you've stepped away:

- **macOS**: Uses CoreGraphics (`CGEventSourceSecondsSinceLastEventType`) to read the system-level idle time. No additional permissions required for basic operation, but accessibility access improves accuracy.
- **Linux**: Queries `org.freedesktop.ScreenSaver` via DBus. Falls back to `org.gnome.Mutter.IdleMonitor` for GNOME/Wayland. Works out of the box on most desktop environments.

The idle threshold is **3 minutes** by default.

## Architecture

```
                  ┌─────────────────────────┐
                  │      BISHRAM SYSTEM      │
                  └───────────┬─────────────┘
                              │
            ┌─────────────────┴─────────────────┐
            ▼                                    ▼
┌───────────────────────┐            ┌───────────────────────┐
│     macOS Backend     │            │     Linux Backend     │
│  - CoreGraphics idle  │            │  - DBus idle query    │
│  - osascript notify   │            │  - notify-send notify │
└───────────┬───────────┘            └───────────┬───────────┘
            │                                    │
            └─────────────────┬──────────────────┘
                              ▼
                 ┌──────────────────────┐
                 │  Tauri Rust Daemon   │
                 │  - Timer state mgr   │
                 │  - Event loop (1s)   │
                 │  - System tray       │
                 └──────────┬───────────┘
                            ▼
                 ┌──────────────────────┐
                 │  Glass Overlay       │
                 │  - Frameless, blur   │
                 │  - Breathing ring    │
                 │  - Stretch cards     │
                 └──────────────────────┘
```

## Troubleshooting

### Tray icon doesn't appear on Linux
Make sure `libayatana-appindicator3-dev` is installed. On some desktop environments (GNOME), you may need the [AppIndicator extension](https://extensions.gnome.org/extension/615/appindicator-support/).

### Overlay window doesn't go fullscreen
Some window managers on Linux may not honor the fullscreen request. Try setting the window to floating mode for Bishram in your WM settings.

### Idle detection not working on macOS
Go to **System Settings → Privacy & Security → Input Monitoring** and ensure Bishram is enabled. You may need to add it manually if it doesn't appear.

### Timer feels inaccurate
The timer ticks every second. Small drift can accumulate over very long sessions but resets each cycle.

## Uninstalling

### macOS
```bash
rm -rf /Applications/Bishram.app
```

### Linux (deb)
```bash
sudo dpkg -r bishram
```

### AppImage
```bash
rm ~/Applications/bishram_*.AppImage
```

## Tech Stack

| Layer | Technology |
|---|---|
| Backend | Rust, Tauri v2 |
| Idle (macOS) | CoreGraphics FFI |
| Idle (Linux) | DBus (freedesktop / Mutter) |
| UI | HTML5, TailwindCSS, Vanilla JS |
| Animations | Inline SVG, CSS keyframes |
| Build | Cargo, Tauri CLI |

## License

MIT
