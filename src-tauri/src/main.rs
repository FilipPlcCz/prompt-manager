// Prevent an extra console window on Windows in release builds.
#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod commands;
mod settings;
mod windows;

use commands::AppState;
use pm_core::Library;
use settings::Settings;
use std::sync::Mutex;
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

fn main() {
    let settings = Settings::load();

    let shortcut_str = settings.shortcut.clone();
    let library_dir = settings.library_dir.clone();
    // Built here, consumed in setup() - i.e. only after the single-instance
    // plugin has decided that this process is the one that keeps running.
    let api_cfg = if settings.api_enabled {
        Some(pm_api::ApiConfig {
            library_root: settings.library_dir.clone(),
            api_key: settings.api_key.clone(),
            port: settings.api_port,
        })
    } else {
        None
    };

    tauri::Builder::default()
        // Must be the first plugin: when another instance is already running,
        // this hands the launch over to it (show the sidebar) and exits this
        // process - one window, one tray icon, no duplicated file writes.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            let _ = windows::show_sidebar(app);
        }))
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, _shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        let _ = windows::toggle_sidebar(app);
                    }
                })
                .build(),
        )
        .manage(AppState {
            settings: Mutex::new(settings),
        })
        .setup(move |app| {
            // ----- library -----
            // Ensure the library exists and seed sample data on first run, then
            // bring variables.json in line with the recipes. migrate() writes
            // only when something actually changes, and must never run on the
            // watcher thread. Runs here (not in main) so a second launch, which
            // exits in the single-instance plugin above, never touches the files.
            if let Ok(lib) = Library::open(&library_dir) {
                let _ = lib.seed_if_empty();
                if let Err(e) = lib.migrate() {
                    eprintln!("library migration failed: {}", e);
                }
            }

            // ----- local REST API (loopback only) -----
            if let Some(cfg) = api_cfg {
                match pm_api::ApiServer::start(cfg) {
                    Ok(server) => {
                        eprintln!("REST API listening on 127.0.0.1:{}", server.port);
                        // keep serving for the app lifetime
                        std::mem::forget(server);
                    }
                    Err(e) => eprintln!("REST API failed to start: {}", e),
                }
            }

            // ----- tray -----
            let open_manager =
                MenuItemBuilder::with_id("open_manager", "Otevřít správu").build(app)?;
            let open_folder =
                MenuItemBuilder::with_id("open_folder", "Otevřít složku knihovny").build(app)?;
            let quit = MenuItemBuilder::with_id("quit", "Ukončit").build(app)?;
            let menu = MenuBuilder::new(app)
                .items(&[&open_manager, &open_folder])
                .separator()
                .items(&[&quit])
                .build()?;
            let mut tray = TrayIconBuilder::with_id("main")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .tooltip("Prompt Manager")
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "open_manager" => {
                        let _ = windows::show_main(app);
                    }
                    "open_folder" => {
                        // Scope the mutex guard tightly so `state` does not
                        // outlive the borrow (Tauri v2 / Rust 2021, E0597).
                        let dir = {
                            let state = app.state::<AppState>();
                            let guard = state.settings.lock();
                            guard.ok().map(|s| s.library_dir.clone())
                        };
                        if let Some(dir) = dir {
                            #[cfg(target_os = "windows")]
                            let _ = std::process::Command::new("explorer").arg(&dir).spawn();
                            #[cfg(target_os = "macos")]
                            let _ = std::process::Command::new("open").arg(&dir).spawn();
                            #[cfg(all(unix, not(target_os = "macos")))]
                            let _ = std::process::Command::new("xdg-open").arg(&dir).spawn();
                        }
                    }
                    "quit" => {
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let _ = windows::toggle_sidebar(tray.app_handle());
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;

            // ----- global shortcut -----
            match app.global_shortcut().register(shortcut_str.as_str()) {
                Ok(()) => {}
                Err(e) => eprintln!("global shortcut '{}' failed: {}", shortcut_str, e),
            }

            // ----- polling watcher (std-only, no notify dependency) -----
            {
                let handle = app.handle().clone();
                let dir = library_dir.clone();
                std::thread::Builder::new()
                    .name("pm-watcher".into())
                    .spawn(move || {
                        let mut last: u64 = Library::open(&dir)
                            .map(|l| l.fingerprint())
                            .unwrap_or(0);
                        loop {
                            std::thread::sleep(std::time::Duration::from_millis(1500));
                            if let Ok(l) = Library::open(&dir) {
                                let f = l.fingerprint();
                                if f != last {
                                    last = f;
                                    let _ = handle.emit("library-changed", ());
                                }
                            }
                        }
                    })
                    .ok();
            }

            // show sidebar on first launch
            let _ = windows::show_sidebar(&app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            // closing a window hides it; the app lives in the tray
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::state_snapshot,
            commands::create_prompt,
            commands::save_prompt,
            commands::delete_prompt,
            commands::duplicate_prompt,
            commands::reorder_prompts,
            commands::set_selected_recipe,
            commands::create_recipe,
            commands::save_recipe,
            commands::delete_recipe,
            commands::recipe_usage,
            commands::add_variable,
            commands::rename_variable,
            commands::delete_variable,
            commands::variable_usage,
            commands::set_always_on_top,
            commands::render_prompt,
            commands::copy_prompt,
            commands::copy_text,
            commands::get_settings,
            commands::save_settings,
            commands::open_library_folder,
            commands::mcp_config_snippet,
            commands::toggle_sidebar,
            commands::open_main,
            commands::hide_sidebar,
            commands::hide_main,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Prompt Manager");
}
