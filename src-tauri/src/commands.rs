//! Tauri commands – thin JSON-string wrappers around pm-core.
//! All payloads are JSON strings so the app needs no serde dependency.

use crate::settings::Settings;
use pm_core::json::Json;
use pm_core::{Library, Prompt, Recipe};
use std::collections::BTreeMap;
use std::sync::Mutex;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_clipboard_manager::ClipboardExt;

pub struct AppState {
    pub settings: Mutex<Settings>,
}

fn lib(state: &State<AppState>) -> Result<Library, String> {
    let s = state.settings.lock().map_err(|e| e.to_string())?;
    Library::open(&s.library_dir).map_err(|e| e.to_string())
}

fn obj(pairs: Vec<(&str, Json)>) -> Json {
    let mut m = BTreeMap::new();
    for (k, v) in pairs {
        m.insert(k.to_string(), v);
    }
    Json::Obj(m)
}

/// Everything the frontend needs in one call.
#[tauri::command]
pub fn state_snapshot(state: State<AppState>) -> Result<String, String> {
    let l = lib(&state)?;
    let prompts = l.list_prompts().map_err(|e| e.to_string())?;
    let recipes = l.list_recipes().map_err(|e| e.to_string())?;
    let vars = l.variables().map_err(|e| e.to_string())?;
    let mut var_arr = Vec::new();
    for (name, uses) in vars {
        var_arr.push(obj(vec![
            ("name", Json::str(&name)),
            (
                "recipes",
                Json::Arr(
                    uses.iter()
                        .map(|(rid, rname, val)| {
                            obj(vec![
                                ("recipe_id", Json::str(rid)),
                                ("recipe_name", Json::str(rname)),
                                ("value", Json::str(val)),
                            ])
                        })
                        .collect(),
                ),
            ),
        ]));
    }
    let s = state.settings.lock().map_err(|e| e.to_string())?;
    Ok(obj(vec![
        (
            "prompts",
            Json::Arr(prompts.iter().map(|p| p.to_json()).collect()),
        ),
        (
            "recipes",
            Json::Arr(recipes.iter().map(|r| r.to_json()).collect()),
        ),
        ("variables", Json::Arr(var_arr)),
        ("settings", s.to_json()),
    ])
    .dump())
}

#[tauri::command]
pub fn create_prompt(
    title: String,
    content: String,
    state: State<AppState>,
) -> Result<String, String> {
    let l = lib(&state)?;
    let p = l.create_prompt(&title, &content).map_err(|e| e.to_string())?;
    Ok(p.to_json().dump())
}

#[tauri::command]
pub fn save_prompt(prompt_json: String, state: State<AppState>) -> Result<String, String> {
    let l = lib(&state)?;
    let v = Json::parse(&prompt_json).map_err(|e| e)?;
    let id = v.get_str("id").ok_or("missing id")?;
    let mut p: Prompt = l.get_prompt(id).map_err(|e| e.to_string())?;
    if let Some(t) = v.get_str("title") {
        p.title = t.to_string();
    }
    if let Some(c) = v.get_str("content") {
        p.content = c.to_string();
    }
    if let Some(rs) = v.get("recipes").and_then(|a| a.as_arr()) {
        p.recipes = rs
            .iter()
            .filter_map(|x| x.as_str().map(|s| s.to_string()))
            .collect();
    }
    if let Some(sel) = v.get("selected_recipe") {
        p.selected_recipe = sel.as_str().map(|s| s.to_string());
    }
    let p = l.save_prompt(&p).map_err(|e| e.to_string())?;
    Ok(p.to_json().dump())
}

#[tauri::command]
pub fn delete_prompt(id: String, state: State<AppState>) -> Result<(), String> {
    lib(&state)?.delete_prompt(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn duplicate_prompt(id: String, state: State<AppState>) -> Result<String, String> {
    let l = lib(&state)?;
    let mut p = l.duplicate_prompt(&id).map_err(|e| e.to_string())?;
    // pm-core appends a Czech " (kopie)"; localize it to the UI language
    let lang = {
        let s = state.settings.lock().map_err(|e| e.to_string())?;
        s.language.clone()
    };
    let suffix = match lang.as_str() {
        "cs" => None,
        "de" => Some(" (Kopie)"),
        _ => Some(" (copy)"),
    };
    if let Some(suf) = suffix {
        if let Some(base) = p.title.strip_suffix(" (kopie)") {
            p.title = format!("{}{}", base, suf);
            p = l.save_prompt(&p).map_err(|e| e.to_string())?;
        }
    }
    Ok(p.to_json().dump())
}

#[tauri::command]
pub fn reorder_prompts(ids_json: String, state: State<AppState>) -> Result<(), String> {
    let v = Json::parse(&ids_json)?;
    let ids: Vec<String> = v
        .as_arr()
        .ok_or("expected array of ids")?
        .iter()
        .filter_map(|x| x.as_str().map(|s| s.to_string()))
        .collect();
    lib(&state)?
        .reorder_prompts(&ids)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_selected_recipe(
    prompt_id: String,
    recipe_id: Option<String>,
    state: State<AppState>,
) -> Result<(), String> {
    let l = lib(&state)?;
    let mut p = l.get_prompt(&prompt_id).map_err(|e| e.to_string())?;
    p.selected_recipe = recipe_id;
    l.save_prompt(&p).map(|_| ()).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn create_recipe(name: String, state: State<AppState>) -> Result<String, String> {
    let r = lib(&state)?
        .create_recipe(&name)
        .map_err(|e| e.to_string())?;
    Ok(r.to_json().dump())
}

#[tauri::command]
pub fn save_recipe(recipe_json: String, state: State<AppState>) -> Result<String, String> {
    let l = lib(&state)?;
    let v = Json::parse(&recipe_json)?;
    let id = v.get_str("id").ok_or("missing id")?;
    let mut r: Recipe = l.get_recipe(id).map_err(|e| e.to_string())?;
    if let Some(n) = v.get_str("name") {
        r.name = n.to_string();
    }
    if let Some(vals) = v.get("values").and_then(|x| x.as_obj()) {
        r.values = vals
            .iter()
            .filter_map(|(k, val)| val.as_str().map(|s| (k.clone(), s.to_string())))
            .collect();
    }
    // preserve explicit ordering if provided as values_order
    if let Some(order) = v.get("values_order").and_then(|x| x.as_arr()) {
        let mut ordered = Vec::new();
        for key in order.iter().filter_map(|x| x.as_str()) {
            if let Some(pos) = r.values.iter().position(|(k, _)| k == key) {
                ordered.push(r.values.remove(pos));
            }
        }
        ordered.append(&mut r.values);
        r.values = ordered;
    }
    let r = l.save_recipe(&r).map_err(|e| e.to_string())?;
    Ok(r.to_json().dump())
}

#[tauri::command]
pub fn delete_recipe(id: String, state: State<AppState>) -> Result<String, String> {
    let affected = lib(&state)?
        .delete_recipe(&id)
        .map_err(|e| e.to_string())?;
    Ok(Json::Arr(affected.iter().map(|t| Json::str(t)).collect()).dump())
}

#[tauri::command]
pub fn recipe_usage(id: String, state: State<AppState>) -> Result<String, String> {
    let titles = lib(&state)?
        .recipe_usage(&id)
        .map_err(|e| e.to_string())?;
    Ok(Json::Arr(titles.iter().map(|t| Json::str(t)).collect()).dump())
}

#[tauri::command]
pub fn render_prompt(
    prompt_id: String,
    recipe_id: Option<String>,
    state: State<AppState>,
) -> Result<String, String> {
    let out = lib(&state)?
        .render_prompt(&prompt_id, recipe_id.as_deref(), &BTreeMap::new())
        .map_err(|e| e.to_string())?;
    Ok(obj(vec![
        ("text", Json::str(&out.text)),
        (
            "missing",
            Json::Arr(out.missing.iter().map(|m| Json::str(m)).collect()),
        ),
    ])
    .dump())
}

/// Renders and puts the result into the system clipboard.
#[tauri::command]
pub fn copy_prompt(
    app: AppHandle,
    prompt_id: String,
    recipe_id: Option<String>,
    state: State<AppState>,
) -> Result<String, String> {
    let out = lib(&state)?
        .render_prompt(&prompt_id, recipe_id.as_deref(), &BTreeMap::new())
        .map_err(|e| e.to_string())?;
    app.clipboard()
        .write_text(out.text.clone())
        .map_err(|e| e.to_string())?;
    Ok(obj(vec![
        ("text", Json::str(&out.text)),
        (
            "missing",
            Json::Arr(out.missing.iter().map(|m| Json::str(m)).collect()),
        ),
    ])
    .dump())
}

/// Puts an arbitrary text into the system clipboard. Used by the sidebar's
/// Shift+click chain copy, where the frontend concatenates several rendered
/// prompts and the backend only performs the clipboard write.
#[tauri::command]
pub fn copy_text(app: AppHandle, text: String) -> Result<(), String> {
    app.clipboard().write_text(text).map_err(|e| e.to_string())
}

// ---------- global variables ----------

fn names_json(names: Vec<String>) -> String {
    Json::Arr(names.iter().map(|n| Json::str(n)).collect()).dump()
}

#[tauri::command]
pub fn add_variable(name: String, state: State<AppState>) -> Result<String, String> {
    let names = lib(&state)?.add_variable(&name).map_err(|e| e.to_string())?;
    Ok(names_json(names))
}

#[tauri::command]
pub fn rename_variable(
    old_name: String,
    new_name: String,
    state: State<AppState>,
) -> Result<String, String> {
    let names = lib(&state)?
        .rename_variable(&old_name, &new_name)
        .map_err(|e| e.to_string())?;
    Ok(names_json(names))
}

#[tauri::command]
pub fn delete_variable(name: String, state: State<AppState>) -> Result<String, String> {
    let names = lib(&state)?
        .delete_variable(&name)
        .map_err(|e| e.to_string())?;
    Ok(names_json(names))
}

/// Titles of prompts whose body still uses the variable (delete confirmation).
#[tauri::command]
pub fn variable_usage(name: String, state: State<AppState>) -> Result<String, String> {
    let titles = lib(&state)?
        .variable_usage(&name)
        .map_err(|e| e.to_string())?;
    Ok(names_json(titles))
}

#[tauri::command]
pub fn set_always_on_top(app: AppHandle, on: bool) -> Result<(), String> {
    crate::windows::set_always_on_top(&app, on)
}

#[tauri::command]
pub fn get_settings(state: State<AppState>) -> Result<String, String> {
    let s = state.settings.lock().map_err(|e| e.to_string())?;
    Ok(s.to_json().dump())
}

#[tauri::command]
pub fn save_settings(settings_json: String, state: State<AppState>) -> Result<String, String> {
    let v = Json::parse(&settings_json)?;
    let mut s = state.settings.lock().map_err(|e| e.to_string())?;
    if let Some(d) = v.get_str("library_dir") {
        if !d.trim().is_empty() {
            s.library_dir = std::path::PathBuf::from(d);
        }
    }
    if let Some(b) = v.get("api_enabled").and_then(|x| x.as_bool()) {
        s.api_enabled = b;
    }
    if let Some(p) = v.get("api_port").and_then(|x| x.as_f64()) {
        if (1.0..=65535.0).contains(&p) {
            s.api_port = p as u16;
        }
    }
    if let Some(sc) = v.get_str("shortcut") {
        if !sc.is_empty() {
            s.shortcut = sc.to_string();
        }
    }
    if let Some(r) = v.get("sidebar_ratio").and_then(|x| x.as_f64()) {
        if (0.05..=0.5).contains(&r) {
            s.sidebar_ratio = r;
        }
    }
    if let Some(b) = v.get("always_on_top").and_then(|x| x.as_bool()) {
        s.always_on_top = b;
    }
    if let Some(l) = v.get_str("language") {
        if matches!(l, "en" | "cs" | "de") {
            s.language = l.to_string();
        }
    }
    if let Some(b) = v.get("widget_enabled").and_then(|x| x.as_bool()) {
        s.widget_enabled = b;
    }
    if v.get("regenerate_api_key").and_then(|x| x.as_bool()) == Some(true) {
        s.api_key = pm_core::util::new_token();
    }
    s.save().map_err(|e| e.to_string())?;
    Ok(s.to_json().dump())
}

/// Opens the library folder in the system file manager.
#[tauri::command]
pub fn open_library_folder(state: State<AppState>) -> Result<(), String> {
    let s = state.settings.lock().map_err(|e| e.to_string())?;
    let dir = s.library_dir.clone();
    drop(s);
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(&dir)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&dir)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(&dir)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Returns a ready-to-paste MCP configuration snippet for Claude Code /
/// Claude Desktop.
#[tauri::command]
pub fn mcp_config_snippet(app: AppHandle, state: State<AppState>) -> Result<String, String> {
    let s = state.settings.lock().map_err(|e| e.to_string())?;
    let exe = app
        .path()
        .resource_dir()
        .ok()
        .map(|d| d.join("prompt-manager-mcp.exe"))
        .filter(|p| p.exists())
        .or_else(|| {
            std::env::current_exe().ok().and_then(|e| {
                e.parent()
                    .map(|d| d.join("prompt-manager-mcp.exe"))
            })
        })
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "prompt-manager-mcp.exe".to_string());
    let snippet = obj(vec![(
        "mcpServers",
        obj(vec![(
            "prompt-manager",
            obj(vec![
                ("command", Json::str(&exe)),
                (
                    "args",
                    Json::Arr(vec![
                        Json::str("--library"),
                        Json::str(&s.library_dir.to_string_lossy()),
                    ]),
                ),
            ]),
        )]),
    )]);
    Ok(snippet.pretty())
}

// ---------- window management ----------

#[tauri::command]
pub fn toggle_sidebar(app: AppHandle) -> Result<(), String> {
    crate::windows::toggle_sidebar(&app)
}

#[tauri::command]
pub fn open_main(app: AppHandle) -> Result<(), String> {
    crate::windows::show_main(&app)
}

#[tauri::command]
pub fn hide_sidebar(app: AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("sidebar") {
        w.hide().map_err(|e| e.to_string())?;
    }
    // the launcher pill takes the sidebar's place once it is hidden
    crate::windows::widget_sync(&app);
    Ok(())
}

/// Shows/hides the always-on-top launcher widget (persisted).
#[tauri::command]
pub fn set_widget_visible(app: AppHandle, on: bool) -> Result<(), String> {
    crate::windows::set_widget_visible(&app, on)
}

#[tauri::command]
pub fn hide_main(app: AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("main") {
        w.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}
