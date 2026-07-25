//! App settings stored in %APPDATA%/PromptManager/settings.json.

use pm_core::json::Json;
use pm_core::util::new_token;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Bumped when a default changes in a way that must reach existing users.
/// v1: sidebar_ratio 0.13 -> 0.20 (the wider one-line sidebar rows).
/// v2: sidebar_ratio 0.20 -> 0.15 (sidebar 25 % narrower again).
pub const SETTINGS_VERSION: u32 = 2;

#[derive(Debug, Clone)]
pub struct Settings {
    pub library_dir: PathBuf,
    pub api_enabled: bool,
    pub api_port: u16,
    pub api_key: String,
    pub shortcut: String,
    /// sidebar width as fraction of the monitor width
    pub sidebar_ratio: f64,
    /// whether the sidebar floats above other windows (the pin)
    pub always_on_top: bool,
    /// UI language: "en" (default), "cs" or "de"
    pub language: String,
    /// the small always-on-top launcher pill at the left screen edge
    pub widget_enabled: bool,
    /// user-dragged position of the pill (logical px); None = default spot
    pub widget_x: Option<f64>,
    pub widget_y: Option<f64>,
    pub settings_version: u32,
}

pub fn config_dir() -> PathBuf {
    #[cfg(windows)]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return PathBuf::from(appdata).join("PromptManager");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".config").join("PromptManager");
    }
    PathBuf::from(".").join("PromptManager")
}

pub fn settings_path() -> PathBuf {
    config_dir().join("settings.json")
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            library_dir: config_dir().join("library"),
            api_enabled: true,
            api_port: 8737,
            api_key: new_token(),
            shortcut: "ctrl+alt+p".into(),
            sidebar_ratio: 0.15,
            always_on_top: true,
            language: "en".into(),
            widget_enabled: true,
            widget_x: None,
            widget_y: None,
            settings_version: SETTINGS_VERSION,
        }
    }
}

impl Settings {
    pub fn load() -> Settings {
        let mut s = Settings::default();
        if let Ok(text) = std::fs::read_to_string(settings_path()) {
            if let Ok(v) = Json::parse(&text) {
                if let Some(d) = v.get_str("library_dir") {
                    if !d.trim().is_empty() {
                        s.library_dir = PathBuf::from(d);
                    }
                }
                if let Some(b) = v.get("api_enabled").and_then(|x| x.as_bool()) {
                    s.api_enabled = b;
                }
                if let Some(p) = v.get("api_port").and_then(|x| x.as_f64()) {
                    if p >= 1.0 && p <= 65535.0 {
                        s.api_port = p as u16;
                    }
                }
                if let Some(k) = v.get_str("api_key") {
                    if !k.is_empty() {
                        s.api_key = k.to_string();
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
                if let Some(x) = v.get("widget_x").and_then(|x| x.as_f64()) {
                    s.widget_x = Some(x);
                }
                if let Some(y) = v.get("widget_y").and_then(|x| x.as_f64()) {
                    s.widget_y = Some(y);
                }
                let ver = v
                    .get("settings_version")
                    .and_then(|x| x.as_f64())
                    .unwrap_or(0.0) as u32;
                // One-time bump of the old defaults. Without this an existing
                // settings.json keeps the old width forever and the new sidebar
                // would look like it simply does not work. Applied in order, so
                // a 0.13 file walks 0.13 -> 0.20 -> 0.15 in a single load.
                if ver < SETTINGS_VERSION {
                    if ver < 1 && (s.sidebar_ratio - 0.13).abs() < 1e-9 {
                        s.sidebar_ratio = 0.20;
                    }
                    if ver < 2 && (s.sidebar_ratio - 0.20).abs() < 1e-9 {
                        s.sidebar_ratio = 0.15;
                    }
                    s.settings_version = SETTINGS_VERSION;
                    let _ = s.save();
                } else {
                    s.settings_version = ver;
                }
            }
        } else {
            // first run: persist defaults (incl. generated API key)
            let _ = s.save();
        }
        s
    }

    pub fn save(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(config_dir())?;
        let mut o = BTreeMap::new();
        o.insert(
            "library_dir".to_string(),
            Json::str(&self.library_dir.to_string_lossy()),
        );
        o.insert("api_enabled".to_string(), Json::Bool(self.api_enabled));
        o.insert("api_port".to_string(), Json::Num(self.api_port as f64));
        o.insert("api_key".to_string(), Json::str(&self.api_key));
        o.insert("shortcut".to_string(), Json::str(&self.shortcut));
        o.insert("always_on_top".to_string(), Json::Bool(self.always_on_top));
        o.insert("language".to_string(), Json::str(&self.language));
        o.insert("widget_enabled".to_string(), Json::Bool(self.widget_enabled));
        if let Some(x) = self.widget_x {
            o.insert("widget_x".to_string(), Json::Num(x));
        }
        if let Some(y) = self.widget_y {
            o.insert("widget_y".to_string(), Json::Num(y));
        }
        o.insert(
            "settings_version".to_string(),
            Json::Num(self.settings_version as f64),
        );
        o.insert(
            "sidebar_ratio".to_string(),
            Json::Num(self.sidebar_ratio),
        );
        // atomic: a crash/kill mid-write must not leave settings.json torn
        let path = settings_path();
        let tmp = path.with_extension("tmp~");
        std::fs::write(&tmp, Json::Obj(o).pretty())?;
        match std::fs::rename(&tmp, &path) {
            Ok(()) => Ok(()),
            Err(_) => {
                let _ = std::fs::remove_file(&path);
                std::fs::rename(&tmp, &path)
            }
        }
    }

    pub fn to_json(&self) -> Json {
        let mut o = BTreeMap::new();
        o.insert(
            "library_dir".to_string(),
            Json::str(&self.library_dir.to_string_lossy()),
        );
        o.insert("api_enabled".to_string(), Json::Bool(self.api_enabled));
        o.insert("api_port".to_string(), Json::Num(self.api_port as f64));
        o.insert("api_key".to_string(), Json::str(&self.api_key));
        o.insert("shortcut".to_string(), Json::str(&self.shortcut));
        o.insert("always_on_top".to_string(), Json::Bool(self.always_on_top));
        o.insert("language".to_string(), Json::str(&self.language));
        o.insert("widget_enabled".to_string(), Json::Bool(self.widget_enabled));
        o.insert(
            "settings_version".to_string(),
            Json::Num(self.settings_version as f64),
        );
        o.insert("sidebar_ratio".to_string(), Json::Num(self.sidebar_ratio));
        Json::Obj(o)
    }
}
