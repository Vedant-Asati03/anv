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
