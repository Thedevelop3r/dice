//! Persistent player stats + settings, stored as a simple `key=value` file.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Store {
    path: PathBuf,
    map: BTreeMap<String, String>,
}

impl Store {
    pub fn load() -> Store {
        let path = data_path();
        let mut map = BTreeMap::new();
        if let Ok(text) = fs::read_to_string(&path) {
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((k, v)) = line.split_once('=') {
                    map.insert(k.trim().to_string(), v.trim().to_string());
                }
            }
        }
        Store { path, map }
    }

    pub fn save(&self) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut out = String::from("# dice_arena save file\n");
        for (k, v) in &self.map {
            out.push_str(&format!("{k}={v}\n"));
        }
        fs::write(&self.path, out)
    }

    pub fn path_display(&self) -> String {
        self.path.display().to_string()
    }

    pub fn get_i64(&self, key: &str, default: i64) -> i64 {
        self.map.get(key).and_then(|v| v.parse().ok()).unwrap_or(default)
    }

    pub fn set_i64(&mut self, key: &str, value: i64) {
        self.map.insert(key.to_string(), value.to_string());
    }

    pub fn bump(&mut self, key: &str, by: i64) {
        let v = self.get_i64(key, 0) + by;
        self.set_i64(key, v);
    }

    /// Records `value` only if it beats the stored best.
    pub fn record_best(&mut self, key: &str, value: i64) -> bool {
        if value > self.get_i64(key, i64::MIN) {
            self.set_i64(key, value);
            true
        } else {
            false
        }
    }

    pub fn get_str(&self, key: &str, default: &str) -> String {
        self.map.get(key).cloned().unwrap_or_else(|| default.to_string())
    }

    pub fn set_str(&mut self, key: &str, value: &str) {
        self.map.insert(key.to_string(), value.to_string());
    }

    pub fn entries(&self) -> impl Iterator<Item = (&String, &String)> {
        self.map.iter()
    }

    pub fn reset(&mut self) {
        let name = self.get_str("player.name", "Player");
        let colors = self.get_i64("cfg.colors", 1);
        self.map.clear();
        self.set_str("player.name", &name);
        self.set_i64("cfg.colors", colors);
    }
}

/// The app's save directory — `$XDG_DATA_HOME/dice_arena` (falling back to
/// `~/.local/share/dice_arena`, then the current directory). Shared with
/// `history.rs`, which keeps its own plain-text log alongside `save.conf`
/// rather than folding a growing log into this file's `key=value` format.
pub fn data_dir() -> PathBuf {
    let base = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|_| PathBuf::from("."));
    base.join("dice_arena")
}

fn data_path() -> PathBuf {
    data_dir().join("save.conf")
}
