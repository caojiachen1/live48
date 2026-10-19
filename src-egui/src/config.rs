use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Preset {
    pub name: String,
    pub groups: Vec<String>,
    pub types: Vec<String>,
    pub members: Vec<String>,
    pub date_start: String,
    pub date_end: String,
}

impl Default for Preset {
    fn default() -> Self {
        Preset {
            name: String::new(),
            groups: Vec::new(),
            types: Vec::new(),
            members: Vec::new(),
            date_start: String::new(),
            date_end: String::new(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Config {
    /// "system" | "vlc" | "builtin"
    pub player_mode: Option<String>,
    pub presets: Vec<Preset>,
    pub fetch_audio: Option<bool>,
}

pub fn dir() -> PathBuf {
    #[cfg(windows)]
    let base = std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    #[cfg(not(windows))]
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(|h| PathBuf::from(h).join(".config"))
                .unwrap_or_else(|_| PathBuf::from("."))
        });
    base.join("live48-egui")
}

pub fn path() -> PathBuf {
    dir().join("config.json")
}

pub fn load() -> Config {
    std::fs::read(path())
        .ok()
        .and_then(|b| serde_json::from_slice::<Config>(&b).ok())
        .unwrap_or_default()
}

pub fn save(cfg: &Config) {
    let dir = dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(json) = serde_json::to_vec_pretty(cfg) {
        let _ = std::fs::write(path(), json);
    }
}
