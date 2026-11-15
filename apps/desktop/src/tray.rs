// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! "Close to tray" (Settings → General): while the tray icon exists,
//! closing the main window hides it instead of quitting, so recording and
//! sync keep running. The icon only exists while the setting is on.
//!
//! The tray menu has a status line (greyed out), *Show Stintory* and
//! *Quit*. On Windows a left click on
//! the icon shows the window too; on Linux the tray (AppIndicator) only
//! opens the menu. GNOME shows no tray icons without the AppIndicator
//! extension: there, starting the app again brings the hidden window back
//! (single instance, see [`crate::run`]).
//!
//! While a game is being recorded the icon gets a red dot. The status line
//! (and on Windows the tooltip; AppIndicator has none) names the game.

use std::sync::Mutex;

use sy_model::api::{RecorderState, RecorderStatus};
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Window, WindowEvent};

use crate::state::AppState;

const TRAY_ID: &str = "main";
const MAIN_WINDOW: &str = "main";
const MENU_SHOW: &str = "show";
const MENU_QUIT: &str = "quit";

/// What [`show_status`] updates while the tray icon exists.
struct TrayStatus {
    /// The status line in the menu
    item: MenuItem<tauri::Wry>,
    /// Whether the icon shows the red dot, so the image is only replaced
    /// when that changes (AppIndicator writes it to a file each time)
    recording: bool,
    /// The status line's text, so the menu is only touched when it changes
    text: String,
}

/// Locked only to read or swap, never across a Tauri call: those wait for
/// the main thread, which may be waiting for this lock.
static TRAY_STATUS: Mutex<Option<TrayStatus>> = Mutex::new(None);

/// Creates the tray icon (no-op if it exists). Fails on Linux without the
/// AppIndicator library.
pub fn enable(app: &AppHandle) -> Result<(), String> {
    if app.tray_by_id(TRAY_ID).is_some() {
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    if !appindicator_available() {
        return Err("The system tray needs the AppIndicator library \
             (libayatana-appindicator3 or libappindicator3). Install it and try again."
            .into());
    }
    let icon = tray_icon(app, false).ok_or("the app has no icon")?;
    let status_item =
        MenuItem::new(app, "Stintory", false, None::<&str>).map_err(|e| e.to_string())?;
    let separator = PredefinedMenuItem::separator(app).map_err(|e| e.to_string())?;
    let show = MenuItem::with_id(app, MENU_SHOW, "Show Stintory", true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let quit =
        MenuItem::with_id(app, MENU_QUIT, "Quit", true, None::<&str>).map_err(|e| e.to_string())?;
    let menu = Menu::with_items(app, &[&status_item, &separator, &show, &quit])
        .map_err(|e| e.to_string())?;
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .tooltip("Stintory")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            MENU_SHOW => show_main_window(app),
            MENU_QUIT => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)
        .map_err(|e| format!("couldn't create the tray icon: {e}"))?;
    *lock_status() = Some(TrayStatus {
        item: status_item,
        recording: false,
        text: String::new(),
    });
    // a recording may have started before the tray existed
    if let Some(state) = app.try_state::<AppState>() {
        let status = state
            .recorder
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        show_status(app, &status);
    }
    Ok(())
}

fn lock_status() -> std::sync::MutexGuard<'static, Option<TrayStatus>> {
    TRAY_STATUS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Shows the recorder status on the tray icon (if there is one).
pub fn show_status(app: &AppHandle, status: &RecorderStatus) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    let recording = status.state == RecorderState::Recording;
    let text = status_text(status);
    let (item, icon_changed, text_changed) = {
        let mut guard = lock_status();
        let Some(tray_status) = guard.as_mut() else {
            return;
        };
        let icon_changed = tray_status.recording != recording;
        let text_changed = tray_status.text != text;
        tray_status.recording = recording;
        tray_status.text.clone_from(&text);
        (tray_status.item.clone(), icon_changed, text_changed)
    };
    if icon_changed
        && let Some(icon) = tray_icon(app, recording)
        && let Err(e) = tray.set_icon(Some(icon))
    {
        tracing::warn!(error = %e, "couldn't update the tray icon");
    }
    if !text_changed {
        return;
    }
    if let Err(e) = item.set_text(&text) {
        tracing::warn!(error = %e, "couldn't update the tray menu");
    }
    if let Err(e) = tray.set_tooltip(Some(format!("Stintory: {text}"))) {
        tracing::warn!(error = %e, "couldn't update the tray tooltip");
    }
}

/// The status line: what the recorder does, without track or laps.
fn status_text(status: &RecorderStatus) -> String {
    match status.state {
        RecorderState::Recording => match status.game {
            Some(game) => format!("Recording {}", game.display_name()),
            None => "Recording".to_owned(),
        },
        // the hint is a sentence for the app's window
        RecorderState::Waiting if status.hint.is_some() => {
            "Game found, but it can't be recorded yet".into()
        }
        RecorderState::Waiting => "Waiting for a game…".into(),
        RecorderState::Unsupported => "Live recording is off".into(),
    }
}

/// The app icon, with the red dot while recording.
fn tray_icon(app: &AppHandle, recording: bool) -> Option<Image<'static>> {
    let icon = app.default_window_icon()?;
    let (width, height) = (icon.width(), icon.height());
    let mut rgba = icon.rgba().to_vec();
    if recording {
        draw_recording_dot(&mut rgba, width, height);
    }
    Some(Image::new_owned(rgba, width, height))
}

/// Red, like the live sector in the logo.
const DOT: [u8; 3] = [0xef, 0x44, 0x44];
/// The logo's background, so the dot stands out from the white dial.
const DOT_RING: [u8; 3] = [0x23, 0x23, 0x28];

/// Paints a red dot with a dark ring into the bottom right corner of an
/// RGBA image (straight alpha), anti-aliased.
fn draw_recording_dot(rgba: &mut [u8], width: u32, height: u32) {
    let size = width.min(height) as f32;
    let radius = size * 0.22;
    let outer = radius + (size * 0.06).max(1.0);
    let (cx, cy) = (width as f32 - outer, height as f32 - outer);
    for y in 0..height {
        for x in 0..width {
            let distance = (x as f32 + 0.5 - cx).hypot(y as f32 + 0.5 - cy);
            let i = ((y * width + x) * 4) as usize;
            let Some(pixel) = rgba.get_mut(i..i + 4) else {
                return;
            };
            blend(pixel, DOT_RING, (outer - distance + 0.5).clamp(0.0, 1.0));
            blend(pixel, DOT, (radius - distance + 0.5).clamp(0.0, 1.0));
        }
    }
}

/// Paints an opaque `color` with `coverage` (0–1) over a straight-alpha pixel.
fn blend(pixel: &mut [u8], color: [u8; 3], coverage: f32) {
    if coverage <= 0.0 {
        return;
    }
    let dst_a = f32::from(pixel[3]) / 255.0;
    let out_a = coverage + dst_a * (1.0 - coverage);
    for (dst, src) in pixel.iter_mut().zip(color) {
        let mixed =
            (f32::from(src) * coverage + f32::from(*dst) * dst_a * (1.0 - coverage)) / out_a;
        *dst = mixed.round() as u8;
    }
    pixel[3] = (out_a * 255.0).round() as u8;
}

/// Removes the tray icon; closing the window quits the app again.
pub fn disable(app: &AppHandle) {
    app.remove_tray_by_id(TRAY_ID);
    lock_status().take();
}

/// Window event handler: hides the main window instead of closing it while
/// the tray icon exists.
pub fn on_window_event(window: &Window, event: &WindowEvent) {
    if let WindowEvent::CloseRequested { api, .. } = event
        && window.label() == MAIN_WINDOW
        && window.app_handle().tray_by_id(TRAY_ID).is_some()
    {
        api.prevent_close();
        if let Err(e) = window.hide() {
            tracing::warn!(error = %e, "couldn't hide the window");
        }
    }
}

/// Shows, restores and focuses the main window.
pub fn show_main_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
        return;
    };
    let result = window
        .show()
        .and_then(|()| window.unminimize())
        .and_then(|()| window.set_focus());
    if let Err(e) = result {
        tracing::warn!(error = %e, "couldn't show the window");
    }
}

/// The tray library (libappindicator-sys) loads one of these at runtime and
/// panics if neither exists. The deb and rpm depend on it and the AppImage
/// ships it (added by the Tauri bundler); a self-built binary may lack it.
#[cfg(target_os = "linux")]
#[allow(unsafe_code)] // dlopen, see SAFETY comment
fn appindicator_available() -> bool {
    ["libayatana-appindicator3.so.1", "libappindicator3.so.1"]
        .iter()
        // SAFETY: loads a system library whose initializers have no
        // preconditions; libappindicator-sys loads the same one later.
        .any(|lib| unsafe { libloading::Library::new(lib) }.is_ok())
}

#[cfg(test)]
mod tests {
    use sy_model::GameId;

    use super::*;

    #[test]
    fn status_line() {
        let mut status = RecorderStatus::idle(RecorderState::Waiting);
        assert_eq!(status_text(&status), "Waiting for a game…");
        status.hint = Some("turn on Enable Plugins".into());
        assert_eq!(
            status_text(&status),
            "Game found, but it can't be recorded yet"
        );

        let mut status = RecorderStatus::idle(RecorderState::Recording);
        status.game = Some(GameId::Lmu);
        status.track_name = Some("Spa".into());
        status.laps = 7;
        assert_eq!(status_text(&status), "Recording Le Mans Ultimate");
    }

    #[test]
    fn dot_is_red_in_the_corner_and_leaves_the_rest() {
        let (w, h) = (64, 64);
        let mut rgba = [10, 20, 30, 255].repeat((w * h) as usize);
        draw_recording_dot(&mut rgba, w, h);
        let px = |x: u32, y: u32| {
            let i = ((y * w + x) * 4) as usize;
            [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
        };
        // center of the dot: radius 14.08, ring to 17.92 → center at 46.08
        assert_eq!(px(46, 46), [0xef, 0x44, 0x44, 255]);
        // the ring
        assert_eq!(px(46, 62), [0x18, 0x18, 0x1b, 255]);
        // far from the dot
        assert_eq!(px(5, 5), [10, 20, 30, 255]);
    }

    #[test]
    fn dot_is_opaque_over_transparent_pixels() {
        let mut pixel = [0, 0, 0, 0];
        blend(&mut pixel, DOT, 1.0);
        assert_eq!(pixel, [0xef, 0x44, 0x44, 255]);
        let mut pixel = [0, 0, 0, 0];
        blend(&mut pixel, DOT, 0.5);
        assert_eq!(pixel, [0xef, 0x44, 0x44, 128]);
    }
}
