use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fmt};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Translation {
    Sub,
    Dub,
    Raw,
}

impl Translation {
    pub fn as_str(self) -> &'static str {
        match self {
            Translation::Sub => "sub",
            Translation::Dub => "dub",
            Translation::Raw => "raw",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Translation::Sub => "Sub",
            Translation::Dub => "Dub",
            Translation::Raw => "Raw",
        }
    }
}

impl fmt::Display for Translation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Debug, Clone)]
pub struct ShowInfo {
    pub id: String,
    pub title: String,
    pub available_eps: EpisodeCounts,
}

#[derive(Debug, Clone, Default)]
pub struct EpisodeCounts {
    pub sub: usize,
    pub dub: usize,
}

#[derive(Debug, Clone)]
pub struct MangaInfo {
    pub id: String,
    pub title: String,
    pub available_chapters: ChapterCounts,
}

#[derive(Debug, Clone, Default)]
pub struct ChapterCounts {
    pub sub: usize,
    pub raw: usize,
}

/// A manga chapter with a human-readable display label (e.g. `"271.5"`) and a
/// provider-specific identifier used to fetch pages (may differ from the label,
/// e.g. a UUID on MangaDex or a URL slug on Mangapill).
#[derive(Debug, Clone)]
pub struct Chapter {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct StreamOption {
    pub provider: String,
    pub url: String,
    pub quality_label: String,
    pub quality_rank: i32,
    pub is_hls: bool,
    pub headers: HashMap<String, String>,
    pub subtitle: Option<String>,
}

impl StreamOption {
    pub fn label(&self) -> String {
        let kind = if self.is_hls { "HLS" } else { "MP4" };
        format!("{} {} ({})", self.provider, self.quality_label, kind)
    }
}

#[derive(Debug, Clone)]
pub struct Page {
    pub url: String,
    pub headers: HashMap<String, String>,
}
