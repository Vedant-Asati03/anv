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
use dialoguer::{Input, Select, theme::ColorfulTheme};
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
    /// Prefer dubbed episodes
    #[arg(long)]
    dub: bool,

    /// Show history and optionally replay previous episodes
    #[arg(long)]
    history: bool,

    /// Search query when starting playback
    #[arg(value_name = "QUERY")]
    query: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Translation {
    Sub,
    Dub,
}

impl Translation {
    fn as_str(self) -> &'static str {
        match self {
            Translation::Sub => "sub",
            Translation::Dub => "dub",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Translation::Sub => "Sub",
            Translation::Dub => "Dub",
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct HistoryEntry {
    show_id: String,
    show_title: String,
    episode: String,
    translation: Translation,
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
        if let Some(pos) = self
            .entries
            .iter()
            .position(|e| e.show_id == entry.show_id && e.translation == entry.translation)
        {
            self.entries.remove(pos);
        }
        self.entries.insert(0, entry);
    }

    fn last_episode(&self, show_id: &str, translation: Translation) -> Option<String> {
        self.entries
            .iter()
            .find(|e| e.show_id == show_id && e.translation == translation)
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
                format!(
                    "[{}] {} · episode {} · watched {}",
                    entry.translation.label(),
                    entry.show_title,
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
    let translation = if cli.dub {
        Translation::Dub
    } else {
        Translation::Sub
    };
    let history_mode =
        cli.history || (cli.query.len() == 1 && cli.query[0].eq_ignore_ascii_case("history"));
    let history_path = history_path()?;
    let mut history = History::load(&history_path)?;
    let client = AllAnimeClient::new()?;

    if history_mode {
        if let Some(entry) = history.select_entry()? {
            let show = ShowInfo {
                id: entry.show_id.clone(),
                title: entry.show_title.clone(),
                available_eps: EpisodeCounts::default(),
            };
            play_show(
                &client,
                &mut history,
                &history_path,
                entry.translation,
                show,
                Some(entry.episode),
            )
            .await?;
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
    play_show(
        &client,
        &mut history,
        &history_path,
        translation,
        show,
        None,
    )
    .await
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

    let mut default_episode = prefer_episode
        .or_else(|| last_watched.clone())
        .unwrap_or_else(|| latest_available.clone());

    loop {
        let chosen = loop {
            let input: String = Input::with_theme(&theme())
                .with_prompt("Episode to play")
                .default(default_episode.clone())
                .interact_text()?;
            if episodes.iter().any(|ep| ep == &input) {
                break input;
            }
            println!(
                "Episode {input} is not available for {} translation.",
                translation.label()
            );
            default_episode = latest_available.clone();
        };

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
                        default_episode = latest_available.clone();
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
                    default_episode = latest_available.clone();
                    continue;
                }
                return Err(err);
            }
        };

        launch_player(&stream, &show.title, &chosen)?;

        history.upsert(HistoryEntry {
            show_id: show.id,
            show_title: show.title,
            episode: chosen,
            translation,
            watched_at: Utc::now(),
        });
        history.save(history_path)?;
        return Ok(());
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

fn history_path() -> Result<PathBuf> {
    let base = data_dir().ok_or_else(|| anyhow!("Could not determine data directory"))?;
    Ok(base.join("anv").join("history.json"))
}

fn theme() -> ColorfulTheme {
    ColorfulTheme::default()
}
