use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use dialoguer::{Select, theme::ColorfulTheme};
use dirs_next::data_dir;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::types::Translation;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HistoryEntry {
    pub show_id: String,
    pub show_title: String,
    pub episode: String,
    pub translation: Translation,
    #[serde(default)]
    pub is_manga: bool,
    pub watched_at: DateTime<Utc>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct History {
    pub entries: Vec<HistoryEntry>,
}

impl History {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let data = fs::read_to_string(path)
            .with_context(|| format!("failed to read history file {}", path.display()))?;
        let history = serde_json::from_str(&data)
            .with_context(|| format!("failed to parse history file {}", path.display()))?;
        Ok(history)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create history directory {}", parent.display())
            })?;
        }
        let data = serde_json::to_string_pretty(self)?;
        fs::write(path, data)
            .with_context(|| format!("failed to write history file {}", path.display()))?;
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

    pub fn select_entry(&self) -> Result<Option<HistoryEntry>> {
        if self.entries.is_empty() {
            println!("History is empty.");
            return Ok(None);
        }

        let items: Vec<String> = self
            .entries
            .iter()
            .map(|entry| {
                let tag = if entry.is_manga {
                    if entry.translation == Translation::Raw {
                        "Raw"
                    } else {
                        "Man"
                    }
                } else {
                    entry.translation.label()
                };
                format!(
                    "[{}] {} \u{00b7} {} {} \u{00b7} watched {}",
                    tag,
                    entry.show_title,
                    if entry.is_manga { "chapter" } else { "episode" },
                    entry.episode,
                    entry.watched_at.format("%Y-%m-%d %H:%M")
                )
            })
            .collect();

        let selection = Select::with_theme(&theme())
            .with_prompt("Select an entry to replay (Esc to cancel)")
            .items(&items)
            .default(0)
            .interact_opt()?;
        Ok(selection.map(|idx| self.entries[idx].clone()))
    }
}

pub fn history_path() -> Result<PathBuf> {
    let base = data_dir().ok_or_else(|| anyhow!("Could not determine data directory"))?;
    Ok(base.join("anv").join("history.json"))
}

pub fn theme() -> ColorfulTheme {
    ColorfulTheme::default()
}
