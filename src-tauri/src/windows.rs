//! Sidebar/main window positioning and toggling.

use tauri::{AppHandle, LogicalPosition, LogicalSize, Manager};

/// Docks the sidebar to the left edge: ~15 % of the monitor width
/// (min 195 logical px), full working-area height.
pub fn position_sidebar(app: &AppHandle) -> Result<(), String> {
    let win = app
        .get_webview_window("sidebar")
        .ok_or("sidebar window missing")?;
    let (ratio, on_top) = {
        let state = app.state::<crate::commands::AppState>();
        let s = state.settings.lock().map_err(|e| e.to_string())?;
        (s.sidebar_ratio, s.always_on_top)
    };
    let monitor = win
        .current_monitor()
        .map_err(|e| e.to_string())?
        .or_else(|| win.primary_monitor().ok().flatten())
        .ok_or("no monitor")?;
    let scale = monitor.scale_factor();
    let area = monitor.work_area();
    let pos_l: LogicalPosition<f64> = area.position.to_logical(scale);
    let size_l: LogicalSize<f64> = area.size.to_logical(scale);
    // floor for the one-line rows: name + recipe badge + copy button still
    // need room. 25 % narrower than the previous 260 px floor.
    let width = (size_l.width * ratio).max(195.0).round();
    win.set_position(LogicalPosition::new(pos_l.x, pos_l.y))
        .map_err(|e| e.to_string())?;
    win.set_size(LogicalSize::new(width, size_l.height))
        .map_err(|e| e.to_string())?;
    win.set_always_on_top(on_top).map_err(|e| e.to_string())?;
    Ok(())
}

/// Applies and persists the pin (always-on-top) state.
pub fn set_always_on_top(app: &AppHandle, on: bool) -> Result<(), String> {
    {
        let state = app.state::<crate::commands::AppState>();
        let mut s = state.settings.lock().map_err(|e| e.to_string())?;
        s.always_on_top = on;
        s.save().map_err(|e| e.to_string())?;
    }
    if let Some(win) = app.get_webview_window("sidebar") {
        win.set_always_on_top(on).map_err(|e| e.to_string())?;
    }
    // the pin changes what "sidebar is in front" means for the pill
    widget_sync(app);
    Ok(())
}

pub fn toggle_sidebar(app: &AppHandle) -> Result<(), String> {
    let win = app
        .get_webview_window("sidebar")
        .ok_or("sidebar window missing")?;
    if win.is_visible().map_err(|e| e.to_string())? {
        // visible but buried behind another window: bring it forward instead
        // of hiding - that is what a click on the launcher pill means there
        if !win.is_focused().unwrap_or(false) {
            win.set_focus().map_err(|e| e.to_string())?;
        } else {
            win.hide().map_err(|e| e.to_string())?;
        }
    } else {
        // show BEFORE positioning: WebView2 does not re-layout the page of a
        // hidden window, so resizing first left the content at the old height
        // (visible as an unfilled window once the list grew long)
        win.show().map_err(|e| e.to_string())?;
        position_sidebar(app)?;
        win.set_focus().map_err(|e| e.to_string())?;
    }
    widget_sync(app);
    Ok(())
}

pub fn show_sidebar(app: &AppHandle) -> Result<(), String> {
    let win = app
        .get_webview_window("sidebar")
        .ok_or("sidebar window missing")?;
    win.show().map_err(|e| e.to_string())?;
    position_sidebar(app)?;
    win.set_focus().map_err(|e| e.to_string())?;
    widget_sync(app);
    Ok(())
}

/// Puts the launcher widget where the user last dragged it, or at the
/// default spot (left edge, a bit below the top). Clamped to the work area
/// so a remembered position from another monitor cannot end up off-screen.
pub fn position_widget(app: &AppHandle) -> Result<(), String> {
    let win = app
        .get_webview_window("widget")
        .ok_or("widget window missing")?;
    let (wx, wy) = {
        let state = app.state::<crate::commands::AppState>();
        let s = state.settings.lock().map_err(|e| e.to_string())?;
        (s.widget_x, s.widget_y)
    };
    let monitor = win
        .current_monitor()
        .map_err(|e| e.to_string())?
        .or_else(|| win.primary_monitor().ok().flatten())
        .ok_or("no monitor")?;
    let scale = monitor.scale_factor();
    let area = monitor.work_area();
    let pos_l: LogicalPosition<f64> = area.position.to_logical(scale);
    let size_l: LogicalSize<f64> = area.size.to_logical(scale);
    let (x, y) = match (wx, wy) {
        // keep the whole pill on-screen (window is 24 x 54 logical px)
        (Some(x), Some(y)) => (
            x.clamp(pos_l.x, pos_l.x + size_l.width - 30.0),
            y.clamp(pos_l.y, pos_l.y + size_l.height - 60.0),
        ),
        _ => (pos_l.x + 6.0, pos_l.y + 110.0),
    };
    win.set_position(LogicalPosition::new(x, y))
        .map_err(|e| e.to_string())?;
    win.set_always_on_top(true).map_err(|e| e.to_string())?;
    Ok(())
}

/// The launcher pill stands in for the sidebar whenever the sidebar is not
/// actually in front of the user. A pinned (always-on-top) sidebar is on
/// screen even without focus, so the pill must never cover it; an unpinned
/// one counts as "in front" only while focused - once another window buries
/// it, the pill comes back. Switched off entirely via widget_enabled.
pub fn widget_sync(app: &AppHandle) {
    let (enabled, pinned) = {
        let state = app.state::<crate::commands::AppState>();
        state
            .settings
            .lock()
            .map(|s| (s.widget_enabled, s.always_on_top))
            .unwrap_or((true, true))
    };
    let sidebar_visible = app
        .get_webview_window("sidebar")
        .map(|w| {
            w.is_visible().unwrap_or(false) && (pinned || w.is_focused().unwrap_or(false))
        })
        .unwrap_or(false);
    if let Some(win) = app.get_webview_window("widget") {
        if enabled && !sidebar_visible {
            let _ = win.show();
            let _ = position_widget(app);
        } else {
            let _ = win.hide();
        }
    }
}

/// Persists the on/off choice, then lets widget_sync decide what shows.
pub fn set_widget_visible(app: &AppHandle, on: bool) -> Result<(), String> {
    {
        let state = app.state::<crate::commands::AppState>();
        let mut s = state.settings.lock().map_err(|e| e.to_string())?;
        s.widget_enabled = on;
        s.save().map_err(|e| e.to_string())?;
    }
    widget_sync(app);
    Ok(())
}

pub fn show_main(app: &AppHandle) -> Result<(), String> {
    let win = app
        .get_webview_window("main")
        .ok_or("main window missing")?;
    win.show().map_err(|e| e.to_string())?;
    win.unminimize().ok();
    win.set_focus().map_err(|e| e.to_string())?;
    Ok(())
}
