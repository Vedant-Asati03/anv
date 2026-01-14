use anyhow::{Context, Result, anyhow, bail};
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;

use super::{AnimeProvider, MangaProvider};
use crate::types::{
    ChapterCounts, EpisodeCounts, MangaInfo, Page, ShowInfo, StreamOption, Translation,
};

const ALLANIME_API_URL: &str = "https://api.allanime.day/api";
const ALLANIME_BASE_URL: &str = "https://allanime.day";
const ALLANIME_REFERER: &str = "https://allmanga.to";
const ALLANIME_ORIGIN: &str = "https://allanime.day";
const PREFERRED_PROVIDERS: &[&str] = &["Default", "S-mp4", "Luf-Mp4", "Yt-mp4"];
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/121.0 Safari/537.36";

pub struct AllAnimeClient {
    client: Client,
}

impl AllAnimeClient {
    pub fn new() -> Result<Self> {
        let client = Client::builder().user_agent(USER_AGENT).build()?;
        Ok(Self { client })
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

    async fn fetch_episode_sources_internal(
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

impl AnimeProvider for AllAnimeClient {
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

    async fn fetch_episodes(&self, show_id: &str, translation: Translation) -> Result<Vec<String>> {
        let detail = self.fetch_show_detail(show_id).await?;
        Ok(match translation {
            Translation::Sub => detail.available_episodes_detail.sub,
            Translation::Dub => detail.available_episodes_detail.dub,
            _ => vec![],
        })
    }

    async fn fetch_streams(
        &self,
        show_id: &str,
        translation: Translation,
        episode: &str,
    ) -> Result<Vec<StreamOption>> {
        let sources = self
            .fetch_episode_sources_internal(show_id, translation, episode)
            .await?;

        for provider in PREFERRED_PROVIDERS {
            if let Some(source) = sources.iter().find(|s| s.source_name == *provider) {
                let decoded = match decode_provider_path(&source.source_url) {
                    Some(d) => d,
                    None => continue,
                };

                let response = match self.fetch_clock_json(&decoded).await {
                    Ok(r) => r,
                    Err(_) => continue,
                };

                let mut options = Vec::new();
                for link in response.links {
                    options.push(build_stream_option(&source.source_name, link));
                }

                if !options.is_empty() {
                    // Sort by quality
                    options.sort_by(|a, b| b.quality_rank.cmp(&a.quality_rank));
                    return Ok(options);
                }
            }
        }

        Ok(Vec::new())
    }
}

impl MangaProvider for AllAnimeClient {
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

    async fn fetch_chapters(
        &self,
        manga_id: &str,
        translation: Translation,
    ) -> Result<Vec<String>> {
        let detail = self.fetch_manga_detail(manga_id).await?;
        Ok(match translation {
            Translation::Sub => detail.available_chapters_detail.sub,
            Translation::Raw => detail.available_chapters_detail.raw,
            _ => vec![],
        })
    }

    async fn fetch_pages(
        &self,
        manga_id: &str,
        translation: Translation,
        chapter: &str,
    ) -> Result<Vec<Page>> {
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
                        let url = if p.url.starts_with("http") {
                            p.url.clone()
                        } else {
                            format!("{}{}", head, p.url)
                        };
                        let mut headers = HashMap::new();
                        headers.insert("Referer".to_string(), ALLANIME_REFERER.to_string());
                        Page { url, headers }
                    })
                    .collect()
            } else {
                Vec::new()
            }
        })
    }
}

// --- Helper Functions ---

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

    let mut headers = link.headers;
    if !headers.keys().any(|k| k.eq_ignore_ascii_case("referer")) {
        headers.insert("Referer".to_string(), ALLANIME_REFERER.to_string());
    }

    StreamOption {
        provider: provider.to_string(),
        url: link.link,
        quality_label,
        quality_rank,
        is_hls: link.hls,
        headers,
        subtitle,
    }
}

fn quality_rank(label: &str) -> i32 {
    if label.eq_ignore_ascii_case("auto") {
        return 10_000;
    }
    label.trim_end_matches('p').parse::<i32>().unwrap_or(0)
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

// --- GraphQL Structs ---

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

// --- Queries ---

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
