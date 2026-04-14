use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use dirs_next::data_dir;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

use crate::types::{Provider, Translation};

const FALLBACK_HISTORY_PATH: &str = "~/.local/share/anv/history.json";

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HistoryEntry {
    pub show_id: String,
    pub show_title: String,
    pub episode: String,
    pub translation: Translation,
    #[serde(default)]
    pub provider: Provider,
    #[serde(default)]
    pub is_manga: bool,
    pub watched_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct History {
    pub entries: Vec<HistoryEntry>,

    #[serde(skip, default = "history_path")]
    path: PathBuf,
}

impl Default for History {
    fn default() -> Self {
        Self {
            entries: vec![],
            path: history_path(),
        }
    }
}

impl History {
    pub fn load(&self) -> Result<Self> {
        if !self.path.exists() {
            return Ok(Self::default());
        }

        let data = fs::read_to_string(&self.path)
            .with_context(|| format!("failed to read history file {}", self.path.display()))?;
        let history = serde_json::from_str(&data)
            .with_context(|| format!("failed to parse history file {}", self.path.display()))?;

        Ok(history)
    }

    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create history directory {}", parent.display())
            })?;
        }
        let data = serde_json::to_string_pretty(self)?;
        fs::write(self.path.clone(), data)
            .with_context(|| format!("failed to write history file {}", self.path.display()))?;
        Ok(())
    }

    pub fn upsert(&mut self, entry: HistoryEntry) {
        if let Some(pos) = self.entries.iter().position(|e| {
            e.show_id == entry.show_id
                && e.translation == entry.translation
                && e.is_manga == entry.is_manga
        }) {
            self.entries.remove(pos);
        }
        self.entries.insert(0, entry);
    }

    pub fn last_episode(&self, show_id: &str, translation: Translation) -> Option<String> {
        self.entries
            .iter()
            .find(|e| e.show_id == show_id && e.translation == translation && !e.is_manga)
            .map(|e| e.episode.clone())
    }

    pub fn last_chapter(&self, show_id: &str, translation: Translation) -> Option<String> {
        self.entries
            .iter()
            .find(|e| e.show_id == show_id && e.translation == translation && e.is_manga)
            .map(|e| e.episode.clone())
    }
}

fn history_path() -> PathBuf {
    let base = data_dir().unwrap_or_else(|| PathBuf::from(FALLBACK_HISTORY_PATH));
    base.join("anv").join("history.json")
}
