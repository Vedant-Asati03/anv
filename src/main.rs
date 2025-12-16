use std::{
    cmp::Ordering,
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use clap::Parser;
use dialoguer::{Select, theme::ColorfulTheme};
use dirs_next::data_dir;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};

const ALLANIME_API_URL: &str = "https://api.allanime.day/api";
const ALLANIME_BASE_URL: &str = "https://allanime.day";
const ALLANIME_REFERER: &str = "https://allmanga.to";
const ALLANIME_ORIGIN: &str = "https://allanime.day";
const PLAYER_ENV_KEY: &str = "ANV_PLAYER";
const PREFERRED_PROVIDERS: &[&str] = &["Default", "S-mp4", "Luf-Mp4", "Yt-mp4"];
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/121.0 Safari/537.36";

#[derive(Debug, Parser)]
#[command(name = "anv", about = "Stream anime from AllAnime via mpv.", version)]
struct Cli {
    #[arg(long)]
    dub: bool,
    #[arg(long)]
    history: bool,

    #[arg(long)]
    manga: bool,

    #[arg(value_name = "QUERY")]
    query: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Translation {
    Sub,
    Dub,
    Raw,
}

impl Translation {
    fn as_str(self) -> &'static str {
        match self {
            Translation::Sub => "sub",
            Translation::Dub => "dub",
            Translation::Raw => "raw",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Translation::Sub => "Sub",
            Translation::Dub => "Dub",
            Translation::Raw => "Raw",
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct HistoryEntry {
    show_id: String,
    show_title: String,
    episode: String,
    translation: Translation,
    #[serde(default)]
    is_manga: bool,
    watched_at: DateTime<Utc>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct History {
    entries: Vec<HistoryEntry>,
}

impl History {
    fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let data = fs::read_to_string(path)
            .with_context(|| format!("failed to read history file {}", path.display()))?;
        let history = serde_json::from_str(&data)
            .with_context(|| format!("failed to parse history file {}", path.display()))?;
        Ok(history)
    }

    fn save(&self, path: &Path) -> Result<()> {
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

    fn upsert(&mut self, entry: HistoryEntry) {
        if let Some(pos) = self.entries.iter().position(|e| {
            e.show_id == entry.show_id
                && e.translation == entry.translation
                && e.is_manga == entry.is_manga
        }) {
            self.entries.remove(pos);
        }
        self.entries.insert(0, entry);
    }

    fn last_episode(&self, show_id: &str, translation: Translation) -> Option<String> {
        self.entries
            .iter()
            .find(|e| e.show_id == show_id && e.translation == translation && !e.is_manga)
            .map(|e| e.episode.clone())
    }

    fn last_chapter(&self, show_id: &str, translation: Translation) -> Option<String> {
        self.entries
            .iter()
            .find(|e| e.show_id == show_id && e.translation == translation && e.is_manga)
            .map(|e| e.episode.clone())
    }

    fn select_entry(&self) -> Result<Option<HistoryEntry>> {
        if self.entries.is_empty() {
            println!("History is empty.");
            return Ok(None);
        }

        let theme = theme();
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
                    "[{}] {} · {} {} · watched {}",
                    tag,
                    entry.show_title,
                    if entry.is_manga { "chapter" } else { "episode" },
                    entry.episode,
                    entry.watched_at.format("%Y-%m-%d %H:%M")
                )
            })
            .collect();

        let selection = Select::with_theme(&theme)
            .with_prompt("Select an entry to replay (Esc to cancel)")
            .items(&items)
            .default(0)
            .interact_opt()?;
        Ok(selection.map(|idx| self.entries[idx].clone()))
    }
}

#[derive(Debug, Clone)]
struct ShowInfo {
    id: String,
    title: String,
    available_eps: EpisodeCounts,
}

#[derive(Debug, Clone, Default)]
struct EpisodeCounts {
    sub: usize,
    dub: usize,
}

#[derive(Debug, Deserialize)]
struct GraphQlEnvelope<T> {
    data: Option<T>,
    errors: Option<Vec<GraphQlError>>,
}

#[derive(Debug, Deserialize)]
struct GraphQlError {
    message: String,
}

#[derive(Debug, Deserialize)]
struct SearchPayload {
    shows: SearchShows,
}

#[derive(Debug, Deserialize)]
struct SearchShows {
    edges: Vec<SearchEdge>,
}

#[derive(Debug, Deserialize, Clone)]
struct SearchEdge {
    #[serde(rename = "_id")]
    id: String,
    name: String,
    #[serde(rename = "availableEpisodes")]
    #[serde(default)]
    available_episodes: AvailabilitySnapshot,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct AvailabilitySnapshot {
    #[serde(default)]
    sub: usize,
    #[serde(default)]
    dub: usize,
}

#[derive(Debug, Clone)]
struct MangaInfo {
    id: String,
    title: String,
    available_chapters: ChapterCounts,
}

#[derive(Debug, Clone, Default)]
struct ChapterCounts {
    sub: usize,
    raw: usize,
}

#[derive(Debug, Deserialize)]
struct SearchMangaPayload {
    mangas: SearchMangas,
}

#[derive(Debug, Deserialize)]
struct SearchMangas {
    edges: Vec<SearchMangaEdge>,
}

#[derive(Debug, Deserialize, Clone)]
struct SearchMangaEdge {
    #[serde(rename = "_id")]
    id: String,
    name: String,
    #[serde(rename = "availableChapters")]
    #[serde(default)]
    available_chapters: ChapterAvailabilitySnapshot,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct ChapterAvailabilitySnapshot {
    #[serde(default)]
    sub: usize,
    #[serde(default)]
    raw: usize,
}

#[derive(Debug, Deserialize)]
struct MangaDetailPayload {
    manga: MangaDetail,
}

#[derive(Debug, Deserialize)]
struct MangaDetail {
    #[serde(rename = "availableChaptersDetail")]
    #[serde(default)]
    available_chapters_detail: ChapterDetail,
}

#[derive(Debug, Deserialize, Default)]
struct ChapterDetail {
    #[serde(default)]
    sub: Vec<String>,
    #[serde(default)]
    raw: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ChapterPagesPayload {
    #[serde(rename = "chapterPages")]
    chapter_pages: ChapterPagesConnection,
}

#[derive(Debug, Deserialize)]
struct ChapterPagesConnection {
    edges: Vec<ChapterPageEdge>,
}

#[derive(Debug, Deserialize)]
struct ChapterPageEdge {
    #[serde(rename = "pictureUrlHead")]
    picture_url_head: String,
    #[serde(rename = "pictureUrls")]
    picture_urls: Vec<PictureUrl>,
}

#[derive(Debug, Deserialize)]
struct PictureUrl {
    url: String,
}

#[derive(Debug, Deserialize)]
struct ShowDetailPayload {
    show: ShowDetail,
}

#[derive(Debug, Deserialize)]
struct ShowDetail {
    #[serde(rename = "availableEpisodesDetail")]
    #[serde(default)]
    available_episodes_detail: EpisodeDetail,
}

#[derive(Debug, Deserialize, Default)]
struct EpisodeDetail {
    #[serde(default)]
    sub: Vec<String>,
    #[serde(default)]
    dub: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct EpisodePayload {
    episode: EpisodeSources,
}

#[derive(Debug, Deserialize)]
struct EpisodeSources {
    #[serde(rename = "sourceUrls")]
    source_urls: Vec<SourceDescriptor>,
}

#[derive(Debug, Deserialize)]
struct SourceDescriptor {
    #[serde(rename = "sourceUrl")]
    source_url: String,
    #[serde(rename = "sourceName")]
    source_name: String,
}

#[derive(Debug, Deserialize)]
struct ClockResponse {
    links: Vec<ClockLink>,
}

#[derive(Debug, Deserialize)]
struct ClockLink {
    link: String,
    #[serde(rename = "resolutionStr")]
    #[serde(default)]
    resolution: Option<String>,
    #[serde(default)]
    hls: bool,
    #[serde(default)]
    subtitles: Vec<ClockSubtitle>,
    #[serde(default)]
    headers: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct ClockSubtitle {
    src: String,
    #[serde(default)]
    lang: Option<String>,
    #[serde(default)]
    label: Option<String>,
}

#[derive(Debug, Clone)]
struct StreamOption {
    provider: String,
    url: String,
    quality_label: String,
    quality_rank: i32,
    is_hls: bool,
    headers: HashMap<String, String>,
    subtitle: Option<String>,
}

impl StreamOption {
    fn label(&self) -> String {
        let kind = if self.is_hls { "HLS" } else { "MP4" };
        format!("{} {} ({})", self.provider, self.quality_label, kind)
    }
}

struct AllAnimeClient {
    client: Client,
}

impl AllAnimeClient {
    fn new() -> Result<Self> {
        let client = Client::builder().user_agent(USER_AGENT).build()?;
        Ok(Self { client })
    }

    async fn search_shows(&self, query: &str, translation: Translation) -> Result<Vec<ShowInfo>> {
        let body = serde_json::json!({
            "query": SEARCH_SHOWS_QUERY,
            "variables": {
                "search": {
                    "allowAdult": false,
                    "allowUnknown": false,
                    "query": query,
                },
                "limit": 25,
                "page": 1,
                "translationType": translation.as_str(),
                "countryOrigin": "ALL"
            }
        });
        let response = self
            .client
            .post(ALLANIME_API_URL)
            .header("Referer", ALLANIME_REFERER)
            .header("Origin", ALLANIME_ORIGIN)
            .header("Accept", "application/json")
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        let text = response.text().await?;
        if !status.is_success() {
            bail!("AllAnime API HTTP {status}: {text}");
        }
        let envelope: GraphQlEnvelope<SearchPayload> =
            serde_json::from_str(&text).with_context(|| "failed to parse search response")?;
        Self::extract_data(envelope).map(|payload| {
            payload
                .shows
                .edges
                .into_iter()
                .map(|edge| ShowInfo {
                    id: edge.id,
                    title: edge.name,
                    available_eps: EpisodeCounts {
                        sub: edge.available_episodes.sub,
                        dub: edge.available_episodes.dub,
                    },
                })
                .collect()
        })
    }

    async fn fetch_show_detail(&self, show_id: &str) -> Result<ShowDetail> {
        let body = serde_json::json!({
            "query": SHOW_DETAIL_QUERY,
            "variables": { "showId": show_id }
        });
        let response = self
            .client
            .post(ALLANIME_API_URL)
            .header("Referer", ALLANIME_REFERER)
            .header("Origin", ALLANIME_ORIGIN)
            .header("Accept", "application/json")
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        let text = response.text().await?;
        if !status.is_success() {
            bail!("AllAnime API HTTP {status}: {text}");
        }
        let envelope: GraphQlEnvelope<ShowDetailPayload> =
            serde_json::from_str(&text).with_context(|| "failed to parse show detail response")?;
        Self::extract_data(envelope).map(|payload| payload.show)
    }

    async fn fetch_episode_sources(
        &self,
        show_id: &str,
        translation: Translation,
        episode: &str,
    ) -> Result<Vec<SourceDescriptor>> {
        let body = serde_json::json!({
            "query": EPISODE_SOURCES_QUERY,
            "variables": {
                "showId": show_id,
                "translationType": translation.as_str(),
                "episodeString": episode
            }
        });
        let response = self
            .client
            .post(ALLANIME_API_URL)
            .header("Referer", ALLANIME_REFERER)
            .header("Origin", ALLANIME_ORIGIN)
            .header("Accept", "application/json")
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        let text = response.text().await?;
        if !status.is_success() {
            bail!("AllAnime API HTTP {status}: {text}");
        }
        let envelope: GraphQlEnvelope<EpisodePayload> =
            serde_json::from_str(&text).with_context(|| "failed to parse episode response")?;
        Self::extract_data(envelope).map(|payload| payload.episode.source_urls)
    }

    async fn fetch_clock_json(&self, path: &str) -> Result<ClockResponse> {
        let url = if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{ALLANIME_BASE_URL}{path}")
        };
        let response = self
            .client
            .get(&url)
            .header("Referer", ALLANIME_REFERER)
            .header("Origin", ALLANIME_ORIGIN)
            .header("Accept", "application/json")
            .send()
            .await?
            .error_for_status()?
            .json::<ClockResponse>()
            .await?;
        Ok(response)
    }

    async fn search_mangas(&self, query: &str, translation: Translation) -> Result<Vec<MangaInfo>> {
        let body = serde_json::json!({
            "query": SEARCH_MANGAS_QUERY,
            "variables": {
                "search": {
                    "allowAdult": false,
                    "allowUnknown": false,
                    "query": query,
                },
                "limit": 25,
                "page": 1,
                "translationType": translation.as_str(),
                "countryOrigin": "ALL"
            }
        });
        let response = self
            .client
            .post(ALLANIME_API_URL)
            .header("Referer", ALLANIME_REFERER)
            .header("Origin", ALLANIME_ORIGIN)
            .header("Accept", "application/json")
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        let text = response.text().await?;
        if !status.is_success() {
            bail!("AllAnime API HTTP {status}: {text}");
        }
        let envelope: GraphQlEnvelope<SearchMangaPayload> =
            serde_json::from_str(&text).with_context(|| "failed to parse search response")?;
        Self::extract_data(envelope).map(|payload| {
            payload
                .mangas
                .edges
                .into_iter()
                .map(|edge| MangaInfo {
                    id: edge.id,
                    title: edge.name,
                    available_chapters: ChapterCounts {
                        sub: edge.available_chapters.sub,
                        raw: edge.available_chapters.raw,
                    },
                })
                .collect()
        })
    }

    async fn fetch_manga_detail(&self, manga_id: &str) -> Result<MangaDetail> {
        let body = serde_json::json!({
            "query": MANGA_DETAIL_QUERY,
            "variables": { "mangaId": manga_id }
        });
        let response = self
            .client
            .post(ALLANIME_API_URL)
            .header("Referer", ALLANIME_REFERER)
            .header("Origin", ALLANIME_ORIGIN)
            .header("Accept", "application/json")
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        let text = response.text().await?;
        if !status.is_success() {
            bail!("AllAnime API HTTP {status}: {text}");
        }
        let envelope: GraphQlEnvelope<MangaDetailPayload> =
            serde_json::from_str(&text).with_context(|| "failed to parse manga detail response")?;
        Self::extract_data(envelope).map(|payload| payload.manga)
    }

    async fn fetch_chapter_pages(
        &self,
        manga_id: &str,
        translation: Translation,
        chapter: &str,
    ) -> Result<Vec<String>> {
        let body = serde_json::json!({
            "query": CHAPTER_PAGES_QUERY,
            "variables": {
                "mangaId": manga_id,
                "translationType": translation.as_str(),
                "chapterString": chapter
            }
        });
        let response = self
            .client
            .post(ALLANIME_API_URL)
            .header("Referer", ALLANIME_REFERER)
            .header("Origin", ALLANIME_ORIGIN)
            .header("Accept", "application/json")
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        let text = response.text().await?;
        if !status.is_success() {
            bail!("AllAnime API HTTP {status}: {text}");
        }
        let envelope: GraphQlEnvelope<ChapterPagesPayload> = serde_json::from_str(&text)
            .with_context(|| "failed to parse chapter pages response")?;
        Self::extract_data(envelope).map(|payload| {
            if let Some(edge) = payload.chapter_pages.edges.first() {
                let head = &edge.picture_url_head;
                edge.picture_urls
                    .iter()
                    .map(|p| {
                        if p.url.starts_with("http") {
                            p.url.clone()
                        } else {
                            format!("{}{}", head, p.url)
                        }
                    })
                    .collect()
            } else {
                Vec::new()
            }
        })
    }

    fn extract_data<T>(envelope: GraphQlEnvelope<T>) -> Result<T> {
        if let Some(errors) = envelope.errors {
            let joined = errors
                .into_iter()
                .map(|e| e.message)
                .collect::<Vec<_>>()
                .join("; ");
            bail!("AllAnime API error: {joined}");
        }
        envelope
            .data
            .ok_or_else(|| anyhow!("AllAnime API returned empty response"))
    }
}

const SEARCH_SHOWS_QUERY: &str = r#"query($search: SearchInput, $limit: Int, $page: Int, $translationType: VaildTranslationTypeEnumType, $countryOrigin: VaildCountryOriginEnumType) {
  shows(search: $search, limit: $limit, page: $page, translationType: $translationType, countryOrigin: $countryOrigin) {
    edges {
      _id
      name
      availableEpisodes
    }
  }
}"#;

const SHOW_DETAIL_QUERY: &str = r#"query($showId: String!) {
  show(_id: $showId) {
    _id
    name
    availableEpisodesDetail
  }
}"#;

const EPISODE_SOURCES_QUERY: &str = r#"query($showId: String!, $translationType: VaildTranslationTypeEnumType!, $episodeString: String!) {
  episode(showId: $showId, translationType: $translationType, episodeString: $episodeString) {
    episodeString
        sourceUrls
  }
}"#;

const SEARCH_MANGAS_QUERY: &str = r#"query($search: SearchInput, $limit: Int, $page: Int, $translationType: VaildTranslationTypeMangaEnumType, $countryOrigin: VaildCountryOriginEnumType) {
  mangas(search: $search, limit: $limit, page: $page, translationType: $translationType, countryOrigin: $countryOrigin) {
    edges {
      _id
      name
      availableChapters
    }
  }
}"#;

const MANGA_DETAIL_QUERY: &str = r#"query($mangaId: String!) {
  manga(_id: $mangaId) {
    availableChaptersDetail
  }
}"#;

const CHAPTER_PAGES_QUERY: &str = r#"query($mangaId: String!, $translationType: VaildTranslationTypeMangaEnumType!, $chapterString: String!) {
  chapterPages(mangaId: $mangaId, translationType: $translationType, chapterString: $chapterString) {
    edges {
      pictureUrlHead
      pictureUrls
    }
  }
}"#;

#[tokio::main]
async fn main() -> Result<()> {
    let result = run().await;
    if let Err(err) = &result {
        eprintln!("error: {err:?}");
    }
    result
}

async fn run() -> Result<()> {
    let cli = Cli::parse();
    let history_mode =
        cli.history || (cli.query.len() == 1 && cli.query[0].eq_ignore_ascii_case("history"));
    let history_path = history_path()?;
    let mut history = History::load(&history_path)?;

    if cli.manga {
        let translation = Translation::Sub;
        return run_manga_flow(&cli, translation, &mut history, &history_path).await;
    }

    let translation = if cli.dub {
        Translation::Dub
    } else {
        Translation::Sub
    };
    run_anime_flow(&cli, translation, history_mode, &mut history, &history_path).await
}

async fn run_manga_flow(
    cli: &Cli,
    translation: Translation,
    history: &mut History,
    history_path: &Path,
) -> Result<()> {
    let client = AllAnimeClient::new()?;

    if cli.query.is_empty() {
        println!("No query provided. Use `anv --manga <name>`.");
        return Ok(());
    }

    let query = cli.query.join(" ");
    let mangas = client.search_mangas(&query, translation).await?;
    if mangas.is_empty() {
        bail!("No results for \"{}\" ({})", query, translation.label());
    }

    let theme = theme();
    let options: Vec<String> = mangas
        .iter()
        .map(|m| {
            let count = match translation {
                Translation::Sub => m.available_chapters.sub,
                Translation::Raw => m.available_chapters.raw,
                _ => 0,
            };
            format!("{} [{} chapters]", m.title, count)
        })
        .collect();
    let selection = Select::with_theme(&theme)
        .with_prompt("Select a manga (Esc to cancel)")
        .items(&options)
        .default(0)
        .interact_opt()?;
    let Some(idx) = selection else {
        println!("Cancelled.");
        return Ok(());
    };
    let manga = mangas[idx].clone();
    read_manga(&client, translation, manga, history, history_path, None).await
}

async fn read_manga(
    client: &AllAnimeClient,
    translation: Translation,
    manga: MangaInfo,
    history: &mut History,
    history_path: &Path,
    prefer_chapter: Option<String>,
) -> Result<()> {
    let detail = client.fetch_manga_detail(&manga.id).await?;
    let chapters = match translation {
        Translation::Sub => detail.available_chapters_detail.sub,
        Translation::Raw => detail.available_chapters_detail.raw,
        _ => vec![],
    };
    if chapters.is_empty() {
        bail!(
            "No {} chapters available for {}",
            translation.label(),
            manga.title
        );
    }

    let latest_available = chapters
        .iter()
        .max_by(|a, b| compare_episode_labels(a, b))
        .cloned()
        .unwrap_or_else(|| String::from("1"));
    println!(
        "Found {} {} chapters. Latest available: {}.",
        chapters.len(),
        translation.label(),
        latest_available
    );

    let last_read = history.last_chapter(&manga.id, translation);
    if let Some(prev) = &last_read {
        println!("Last read {} chapter: {}.", translation.label(), prev);
    }

    let mut current_chapter = prefer_chapter
        .or_else(|| last_read.clone())
        .unwrap_or_else(|| latest_available.clone());

    loop {
        let default_idx = chapters
            .iter()
            .position(|ch| ch == &current_chapter)
            .or_else(|| chapters.iter().position(|ch| ch == &latest_available))
            .unwrap_or(0);

        let selection = Select::with_theme(&theme())
            .with_prompt("Chapter to read (Enter to select, Esc to cancel)")
            .items(&chapters)
            .default(default_idx)
            .interact_opt()?;
        let Some(idx) = selection else {
            println!("Exiting reading loop.");
            return Ok(());
        };

        let chosen = chapters[idx].clone();
        let auto_advance = idx == default_idx;

        let pages = match client
            .fetch_chapter_pages(&manga.id, translation, &chosen)
            .await
        {
            Ok(pages) => pages,
            Err(err) => {
                println!("Failed to fetch pages for chapter {}: {}", chosen, err);
                continue;
            }
        };

        if pages.is_empty() {
            println!("No pages found for chapter {}.", chosen);
            continue;
        }

        let next_candidate = next_episode_label(&chosen, &chapters);

        launch_image_viewer(&pages, &manga.title, &chosen)?;

        let chosen_copy = chosen.clone();
        history.upsert(HistoryEntry {
            show_id: manga.id.clone(),
            show_title: manga.title.clone(),
            episode: chosen_copy.clone(),
            translation,
            is_manga: true,
            watched_at: Utc::now(),
        });
        history.save(history_path)?;

        match (auto_advance, next_candidate) {
            (true, Some(next)) => {
                current_chapter = next;
            }
            (true, None) => {
                println!("No further chapters found. Exiting.");
                return Ok(());
            }
            (false, candidate) => {
                current_chapter = candidate.unwrap_or_else(|| chosen.clone());
            }
        }
    }
}

fn launch_image_viewer(pages: &[String], title: &str, chapter: &str) -> Result<()> {
    let player = detect_player();
    let mut cmd = Command::new(&player);
    let media_title = format!("{title} - Chapter {chapter}");
    cmd.arg("--quiet");
    cmd.arg("--terminal=no");
    cmd.arg(format!("--force-media-title={media_title}"));
    cmd.arg("--image-display-duration=inf");
    cmd.arg(format!("--referrer={ALLANIME_REFERER}"));
    cmd.arg(format!("--http-header-fields=Referer: {ALLANIME_REFERER}"));

    for page in pages {
        cmd.arg(page);
    }

    println!("Launching viewer for Chapter {}...", chapter);
    let status = cmd.status().context("failed to launch viewer")?;

    if !status.success() {
        bail!("viewer exited with status {status}");
    }
    Ok(())
}

async fn run_anime_flow(
    cli: &Cli,
    translation: Translation,
    history_mode: bool,
    history: &mut History,
    history_path: &Path,
) -> Result<()> {
    let client = AllAnimeClient::new()?;

    if history_mode {
        if let Some(entry) = history.select_entry()? {
            if entry.is_manga {
                let manga = MangaInfo {
                    id: entry.show_id.clone(),
                    title: entry.show_title.clone(),
                    available_chapters: ChapterCounts::default(),
                };
                let preferred_chapter = Some(entry.episode.clone());
                let entry_translation = entry.translation;
                // We need to call read_manga but it expects a loop.
                // read_manga handles fetching details and looping.
                // We just need to pass the preferred chapter logic if we want to start from there.
                // But read_manga currently doesn't take a preferred chapter argument.
                // I should update read_manga to take an optional preferred chapter.
                // For now, I'll just call it and it will default to last read which is what we want.
                // Wait, read_manga uses history.last_chapter().
                // If we selected an entry, we probably want to continue from there.
                // But history.last_chapter() returns the latest one in history.
                // If we selected an older entry, we might want to replay that specific one?
                // The current implementation of play_show takes prefer_episode.
                // I should update read_manga to take prefer_chapter.
                read_manga(
                    &client,
                    entry_translation,
                    manga,
                    history,
                    history_path,
                    preferred_chapter,
                )
                .await?;
            } else {
                let show = ShowInfo {
                    id: entry.show_id.clone(),
                    title: entry.show_title.clone(),
                    available_eps: EpisodeCounts::default(),
                };
                let preferred_episode = Some(entry.episode.clone());
                let entry_translation = entry.translation;
                play_show(
                    &client,
                    history,
                    history_path,
                    entry_translation,
                    show,
                    preferred_episode,
                )
                .await?;
            }
        }
        return Ok(());
    }

    if cli.query.is_empty() {
        println!("No query provided. Use `anv <name>` or `anv --history`.");
        return Ok(());
    }

    let query = cli.query.join(" ");
    let shows = client.search_shows(&query, translation).await?;
    if shows.is_empty() {
        bail!("No results for \"{}\" ({})", query, translation.label());
    }

    let theme = theme();
    let options: Vec<String> = shows
        .iter()
        .map(|s| {
            let count = match translation {
                Translation::Sub => s.available_eps.sub,
                Translation::Dub => s.available_eps.dub,
                _ => 0,
            };
            format!("{} [{} episodes]", s.title, count)
        })
        .collect();
    let selection = Select::with_theme(&theme)
        .with_prompt("Select a show (Esc to cancel)")
        .items(&options)
        .default(0)
        .interact_opt()?;
    let Some(idx) = selection else {
        println!("Cancelled.");
        return Ok(());
    };
    let show = shows[idx].clone();
    play_show(&client, history, history_path, translation, show, None).await
}

async fn play_show(
    client: &AllAnimeClient,
    history: &mut History,
    history_path: &Path,
    translation: Translation,
    show: ShowInfo,
    prefer_episode: Option<String>,
) -> Result<()> {
    let detail = client.fetch_show_detail(&show.id).await?;
    let episodes = match translation {
        Translation::Sub => detail.available_episodes_detail.sub,
        Translation::Dub => detail.available_episodes_detail.dub,
        _ => vec![],
    };
    if episodes.is_empty() {
        bail!(
            "No {} episodes available for {}",
            translation.label(),
            show.title
        );
    }

    let latest_available = episodes
        .iter()
        .max_by(|a, b| compare_episode_labels(a, b))
        .cloned()
        .unwrap_or_else(|| String::from("1"));
    println!(
        "Found {} {} episodes. Latest available: {}.",
        episodes.len(),
        translation.label(),
        latest_available
    );

    let last_watched = history.last_episode(&show.id, translation);
    if let Some(prev) = &last_watched {
        println!("Last watched {} episode: {}.", translation.label(), prev);
    }

    let mut current_episode = prefer_episode
        .or_else(|| last_watched.clone())
        .unwrap_or_else(|| latest_available.clone());

    loop {
        let default_idx = episodes
            .iter()
            .position(|ep| ep == &current_episode)
            .or_else(|| episodes.iter().position(|ep| ep == &latest_available))
            .unwrap_or(0);

        let selection = Select::with_theme(&theme())
            .with_prompt("Episode to play (Enter to select, Esc to cancel)")
            .items(&episodes)
            .default(default_idx)
            .interact_opt()?;
        let Some(idx) = selection else {
            println!("Exiting playback loop.");
            return Ok(());
        };

        let chosen = episodes[idx].clone();
        let auto_advance = idx == default_idx;

        let sources = match client
            .fetch_episode_sources(&show.id, translation, &chosen)
            .await
        {
            Ok(sources) => sources,
            Err(err) => {
                if let Some(req_err) = err.downcast_ref::<reqwest::Error>() {
                    if req_err.status() == Some(StatusCode::BAD_REQUEST) {
                        println!(
                            "Episode {chosen} is not yet available for {} translation.",
                            translation.label()
                        );
                        current_episode = latest_available.clone();
                        continue;
                    }
                }
                return Err(err);
            }
        };

        let stream = match select_stream_option(client, &sources).await {
            Ok(stream) => stream,
            Err(err) => {
                if err.to_string().contains("No supported providers") {
                    println!(
                        "No supported streams found for episode {chosen}. Try another episode or rerun later."
                    );
                    current_episode = latest_available.clone();
                    continue;
                }
                return Err(err);
            }
        };

        let next_candidate = next_episode_label(&chosen, &episodes);

        launch_player(&stream, &show.title, &chosen)?;

        let chosen_copy = chosen.clone();
        history.upsert(HistoryEntry {
            show_id: show.id.clone(),
            show_title: show.title.clone(),
            episode: chosen_copy.clone(),
            translation,
            is_manga: false,
            watched_at: Utc::now(),
        });
        history.save(history_path)?;
        match (auto_advance, next_candidate) {
            (true, Some(next)) => {
                current_episode = next;
            }
            (true, None) => {
                println!("No further episodes found. Exiting.");
                return Ok(());
            }
            (false, candidate) => {
                current_episode = candidate.unwrap_or_else(|| chosen_copy.clone());
            }
        }
    }
}

async fn select_stream_option(
    client: &AllAnimeClient,
    sources: &[SourceDescriptor],
) -> Result<StreamOption> {
    for provider in PREFERRED_PROVIDERS {
        if let Some(source) = sources.iter().find(|s| s.source_name == *provider) {
            let decoded = decode_provider_path(&source.source_url)
                .with_context(|| format!("failed to decode provider {}", source.source_name))?;
            let response = client.fetch_clock_json(&decoded).await.with_context(|| {
                format!(
                    "failed to fetch stream list for provider {}",
                    source.source_name
                )
            })?;
            let mut options: Vec<StreamOption> = response
                .links
                .into_iter()
                .map(|link| build_stream_option(&source.source_name, link))
                .collect();
            if options.is_empty() {
                continue;
            }
            options.sort_by(|a, b| b.quality_rank.cmp(&a.quality_rank));
            return choose_stream(options);
        }
    }
    bail!("No supported providers found for this episode.")
}

fn choose_stream(mut options: Vec<StreamOption>) -> Result<StreamOption> {
    if options.len() == 1 {
        return Ok(options.remove(0));
    }
    let theme = theme();
    let labels: Vec<String> = options.iter().map(StreamOption::label).collect();
    let selection = Select::with_theme(&theme)
        .with_prompt("Select a stream")
        .items(&labels)
        .default(0)
        .interact_opt()?;
    let Some(idx) = selection else {
        bail!("Stream selection cancelled.");
    };
    Ok(options.remove(idx))
}

fn build_stream_option(provider: &str, link: ClockLink) -> StreamOption {
    let quality_label = link
        .resolution
        .clone()
        .unwrap_or_else(|| String::from("auto"));
    let quality_rank = quality_rank(&quality_label);
    let subtitle = link
        .subtitles
        .iter()
        .find(|sub| sub.lang.as_deref() == Some("en") || sub.label.as_deref() == Some("English"))
        .map(|sub| sub.src.clone());
    StreamOption {
        provider: provider.to_string(),
        url: link.link,
        quality_label,
        quality_rank,
        is_hls: link.hls,
        headers: link.headers,
        subtitle,
    }
}

fn quality_rank(label: &str) -> i32 {
    if label.eq_ignore_ascii_case("auto") {
        return 10_000;
    }
    label.trim_end_matches('p').parse::<i32>().unwrap_or(0)
}

fn launch_player(stream: &StreamOption, title: &str, episode: &str) -> Result<()> {
    let player = detect_player();
    let mut cmd = Command::new(&player);
    let media_title = format!("{title} - Episode {episode}");
    cmd.arg("--quiet");
    cmd.arg("--terminal=no");
    cmd.arg(format!("--force-media-title={media_title}"));
    if let Some(sub) = &stream.subtitle {
        cmd.arg(format!("--sub-file={sub}"));
    }
    let mut has_referer = false;
    for (key, value) in &stream.headers {
        if key.eq_ignore_ascii_case("user-agent") {
            cmd.arg(format!("--user-agent={value}"));
        } else if key.eq_ignore_ascii_case("referer") {
            has_referer = true;
            cmd.arg(format!("--referrer={value}"));
            cmd.arg(format!("--http-header-fields=Referer: {value}"));
        } else {
            cmd.arg(format!("--http-header-fields={}: {value}", key));
        }
    }
    if !has_referer {
        cmd.arg(format!("--referrer={ALLANIME_REFERER}"));
        cmd.arg(format!("--http-header-fields=Referer: {ALLANIME_REFERER}"));
    }
    cmd.arg(&stream.url);

    let status = match cmd.status() {
        Ok(status) => status,
        Err(err) => {
            if err.kind() == std::io::ErrorKind::NotFound {
                return Err(anyhow!(
                    "Player '{}' not found. Install mpv or set {} to a valid command.",
                    player,
                    PLAYER_ENV_KEY
                ));
            }
            return Err(anyhow!(err).context(format!("failed to launch player '{player}'")));
        }
    };

    if !status.success() {
        bail!("player exited with status {status}");
    }
    Ok(())
}

fn detect_player() -> String {
    std::env::var(PLAYER_ENV_KEY)
        .ok()
        .filter(|val| !val.trim().is_empty())
        .unwrap_or_else(|| "mpv".to_string())
}

fn decode_provider_path(raw: &str) -> Option<String> {
    if !raw.starts_with("--") {
        return None;
    }
    let bytes = raw.trim_start_matches("--");
    if bytes.len() % 2 != 0 {
        return None;
    }
    let mut decoded = String::with_capacity(bytes.len() / 2);
    for chunk in bytes.as_bytes().chunks(2) {
        let pair = std::str::from_utf8(chunk).ok()?.to_ascii_lowercase();
        let ch = decode_pair(&pair)?;
        decoded.push(ch);
    }
    if decoded.contains("/clock") && !decoded.contains(".json") {
        decoded = decoded.replacen("/clock", "/clock.json", 1);
    }
    Some(decoded)
}

fn decode_pair(pair: &str) -> Option<char> {
    match pair {
        "79" => Some('A'),
        "7a" => Some('B'),
        "7b" => Some('C'),
        "7c" => Some('D'),
        "7d" => Some('E'),
        "7e" => Some('F'),
        "7f" => Some('G'),
        "70" => Some('H'),
        "71" => Some('I'),
        "72" => Some('J'),
        "73" => Some('K'),
        "74" => Some('L'),
        "75" => Some('M'),
        "76" => Some('N'),
        "77" => Some('O'),
        "68" => Some('P'),
        "69" => Some('Q'),
        "6a" => Some('R'),
        "6b" => Some('S'),
        "6c" => Some('T'),
        "6d" => Some('U'),
        "6e" => Some('V'),
        "6f" => Some('W'),
        "60" => Some('X'),
        "61" => Some('Y'),
        "62" => Some('Z'),
        "59" => Some('a'),
        "5a" => Some('b'),
        "5b" => Some('c'),
        "5c" => Some('d'),
        "5d" => Some('e'),
        "5e" => Some('f'),
        "5f" => Some('g'),
        "50" => Some('h'),
        "51" => Some('i'),
        "52" => Some('j'),
        "53" => Some('k'),
        "54" => Some('l'),
        "55" => Some('m'),
        "56" => Some('n'),
        "57" => Some('o'),
        "48" => Some('p'),
        "49" => Some('q'),
        "4a" => Some('r'),
        "4b" => Some('s'),
        "4c" => Some('t'),
        "4d" => Some('u'),
        "4e" => Some('v'),
        "4f" => Some('w'),
        "40" => Some('x'),
        "41" => Some('y'),
        "42" => Some('z'),
        "08" => Some('0'),
        "09" => Some('1'),
        "0a" => Some('2'),
        "0b" => Some('3'),
        "0c" => Some('4'),
        "0d" => Some('5'),
        "0e" => Some('6'),
        "0f" => Some('7'),
        "00" => Some('8'),
        "01" => Some('9'),
        "15" => Some('-'),
        "16" => Some('.'),
        "67" => Some('_'),
        "46" => Some('~'),
        "02" => Some(':'),
        "17" => Some('/'),
        "07" => Some('?'),
        "1b" => Some('#'),
        "63" => Some('['),
        "65" => Some(']'),
        "78" => Some('@'),
        "19" => Some('!'),
        "1c" => Some('$'),
        "1e" => Some('&'),
        "10" => Some('('),
        "11" => Some(')'),
        "12" => Some('*'),
        "13" => Some('+'),
        "14" => Some(','),
        "03" => Some(';'),
        "05" => Some('='),
        "1d" => Some('%'),
        _ => None,
    }
}

fn compare_episode_labels(left: &str, right: &str) -> Ordering {
    let l = parse_episode_key(left);
    let r = parse_episode_key(right);
    l.partial_cmp(&r).unwrap_or(Ordering::Equal)
}

fn parse_episode_key(label: &str) -> f32 {
    label.parse::<f32>().unwrap_or(0.0)
}

fn sorted_episode_labels(episodes: &[String]) -> Vec<String> {
    let mut sorted = episodes.to_vec();
    sorted.sort_by(|a, b| compare_episode_labels(a, b));
    sorted.dedup();
    sorted
}

fn next_episode_label(current: &str, episodes: &[String]) -> Option<String> {
    let sorted = sorted_episode_labels(episodes);
    let pos = sorted.iter().position(|ep| ep == current)?;
    sorted.get(pos + 1).cloned()
}

fn history_path() -> Result<PathBuf> {
    let base = data_dir().ok_or_else(|| anyhow!("Could not determine data directory"))?;
    Ok(base.join("anv").join("history.json"))
}

fn theme() -> ColorfulTheme {
    ColorfulTheme::default()
}
