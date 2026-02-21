use std::{
    cmp::Ordering,
    fs,
    io::{BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering as AtomicOrdering},
    },
    thread,
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use clap::Parser;
use dialoguer::{Select, theme::ColorfulTheme};
use dirs_next::{cache_dir, data_dir};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};

mod providers;
mod types;

use providers::{
    AnimeProvider, MangaProvider, allanime::AllAnimeClient, mangadex::MangaDexClient,
    mangapill::MangapillClient,
};
use types::{ChapterCounts, EpisodeCounts, MangaInfo, Page, ShowInfo, StreamOption, Translation};

const PLAYER_ENV_KEY: &str = "ANV_PLAYER";
const INITIAL_MANGA_PAGE_PRELOAD: usize = 5;
const CACHE_USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/121.0 Safari/537.36";
const CACHE_ACCEPT: &str = "image/avif,image/webp,image/*,*/*;q=0.8";

#[derive(Clone)]
struct CachedPageTarget {
    page: Page,
    path: PathBuf,
}

struct LocalPageProxy {
    base_url: String,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl LocalPageProxy {
    fn start(targets: Vec<CachedPageTarget>) -> Result<Self> {
        let listener =
            TcpListener::bind(("127.0.0.1", 0)).context("failed to bind local page cache proxy")?;
        let addr = listener
            .local_addr()
            .context("failed to read local proxy address")?;
        listener
            .set_nonblocking(true)
            .context("failed to configure local proxy socket")?;

        let stop = Arc::new(AtomicBool::new(false));
        let stop_signal = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            while !stop_signal.load(AtomicOrdering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        if let Err(err) = handle_proxy_request(&mut stream, &targets) {
                            if is_benign_proxy_error(&err) {
                                continue;
                            }
                            let _ = write_http_error(&mut stream, 500, "proxy error");
                            println!("Local cache proxy request failed: {}", err);
                        }
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(25));
                    }
                    Err(err) => {
                        println!("Local cache proxy accept failed: {}", err);
                        thread::sleep(Duration::from_millis(50));
                    }
                }
            }
        });

        Ok(Self {
            base_url: format!("http://127.0.0.1:{}", addr.port()),
            stop,
            handle: Some(handle),
        })
    }

    fn page_url(&self, idx: usize) -> String {
        format!("{}/{}", self.base_url, idx)
    }

    fn shutdown(&mut self) {
        self.stop.store(true, AtomicOrdering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for LocalPageProxy {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[derive(Debug, Parser)]
#[command(name = "anv", about = "Stream anime from AllAnime via mpv.", version)]
struct Cli {
    #[arg(long)]
    dub: bool,
    #[arg(long)]
    history: bool,

    #[arg(long)]
    manga: bool,

    #[arg(long, default_value = "allanime")]
    provider: String,

    #[arg(long, value_name = "DIR")]
    cache_dir: Option<PathBuf>,

    #[arg(short = 'e', long, value_name = "EPISODE")]
    episode: Option<String>,

    #[arg(value_name = "QUERY")]
    query: Vec<String>,
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
        match cli.provider.to_lowercase().as_str() {
            "allanime" => {
                let client = AllAnimeClient::new()?;
                return run_manga_flow(&cli, translation, &mut history, &history_path, &client)
                    .await;
            }
            "mangadex" => {
                let client = MangaDexClient::new()?;
                return run_manga_flow(&cli, translation, &mut history, &history_path, &client)
                    .await;
            }
            "mangapill" => {
                let client = MangapillClient::new()?;
                return run_manga_flow(&cli, translation, &mut history, &history_path, &client)
                    .await;
            }
            _ => bail!("Unknown provider: {}", cli.provider),
        }
    }

    let translation = if cli.dub {
        Translation::Dub
    } else {
        Translation::Sub
    };

    if cli.provider.to_lowercase() != "allanime" {
        println!("Warning: Only 'allanime' provider supports anime. Switching to 'allanime'.");
    }
    run_anime_flow(&cli, translation, history_mode, &mut history, &history_path).await
}

async fn run_manga_flow(
    cli: &Cli,
    translation: Translation,
    history: &mut History,
    history_path: &Path,
    client: &impl MangaProvider,
) -> Result<()> {
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
    read_manga(
        client,
        translation,
        manga,
        history,
        history_path,
        None,
        cli.cache_dir.as_deref(),
    )
    .await
}

async fn read_manga(
    client: &impl MangaProvider,
    translation: Translation,
    manga: MangaInfo,
    history: &mut History,
    history_path: &Path,
    prefer_chapter: Option<String>,
    cache_base_override: Option<&Path>,
) -> Result<()> {
    let chapters = client.fetch_chapters(&manga.id, translation).await?;
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

        let pages = match client.fetch_pages(&manga.id, translation, &chosen).await {
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
        println!("Caching chapter pages locally...");
        let cache_state = match cache_manga_pages(
            &pages,
            &manga.id,
            translation,
            &chosen,
            cache_base_override,
        )
        .await
        {
            Ok(state) => {
                let cached_count = state.cached_pages.iter().filter(|p| p.is_some()).count();
                println!(
                    "Cached {cached_count}/{} pages upfront for Chapter {} (first {} pages).",
                    pages.len(),
                    chosen,
                    INITIAL_MANGA_PAGE_PRELOAD
                );
                if pages.len() > INITIAL_MANGA_PAGE_PRELOAD {
                    println!("Continuing to cache remaining pages in background...");
                }
                state
            }
            Err(err) => {
                println!(
                    "Page cache unavailable for Chapter {} ({}). Falling back to streaming URLs.",
                    chosen, err
                );
                MangaCacheState {
                    cached_pages: vec![None; pages.len()],
                    cache_files: Vec::new(),
                }
            }
        };

        launch_image_viewer(
            &pages,
            &cache_state.cached_pages,
            &cache_state.cache_files,
            &manga.title,
            &chosen,
        )?;

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

fn launch_image_viewer(
    pages: &[Page],
    cached_pages: &[Option<PathBuf>],
    cache_files: &[PathBuf],
    title: &str,
    chapter: &str,
) -> Result<()> {
    let player = detect_player();
    let mut cmd = Command::new(&player);
    let media_title = format!("{title} - Chapter {chapter}");
    cmd.arg("--quiet");
    cmd.arg("--terminal=no");
    cmd.arg(format!("--force-media-title={media_title}"));
    cmd.arg("--image-display-duration=inf");

    let all_cached = cached_pages.len() == pages.len() && cached_pages.iter().all(|p| p.is_some());
    if all_cached {
        for path in cached_pages.iter().flatten() {
            cmd.arg(path);
        }
    } else if cache_files.len() == pages.len() {
        let targets: Vec<CachedPageTarget> = pages
            .iter()
            .cloned()
            .zip(cache_files.iter().cloned())
            .map(|(page, path)| CachedPageTarget { page, path })
            .collect();
        match LocalPageProxy::start(targets) {
            Ok(mut proxy) => {
                for idx in 0..pages.len() {
                    cmd.arg(proxy.page_url(idx));
                }
                println!("Launching viewer for Chapter {}...", chapter);
                let status = cmd.status().context("failed to launch viewer")?;
                proxy.shutdown();

                if !status.success() {
                    bail!("viewer exited with status {status}");
                }
                return Ok(());
            }
            Err(err) => {
                println!(
                    "Local cache proxy unavailable ({}). Falling back to direct URLs.",
                    err
                );
            }
        }
    } else {
        if let Some(first) = pages.first() {
            for (key, value) in &first.headers {
                if key.eq_ignore_ascii_case("referer") {
                    cmd.arg(format!("--referrer={value}"));
                    cmd.arg(format!("--http-header-fields=Referer: {value}"));
                } else {
                    cmd.arg(format!("--http-header-fields={}: {value}", key));
                }
            }
        }
        for page in pages {
            cmd.arg(&page.url);
        }
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
                read_manga(
                    &client,
                    entry_translation,
                    manga,
                    history,
                    history_path,
                    preferred_chapter,
                    cli.cache_dir.as_deref(),
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
    play_show(
        &client,
        history,
        history_path,
        translation,
        show,
        cli.episode.clone(),
    )
    .await
}

async fn play_show(
    client: &impl AnimeProvider,
    history: &mut History,
    history_path: &Path,
    translation: Translation,
    show: ShowInfo,
    prefer_episode: Option<String>,
) -> Result<()> {
    let episodes = client.fetch_episodes(&show.id, translation).await?;
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

    // Determine starting episode and whether to skip the selection dialog on
    // the first iteration (when the caller provides a valid --episode flag).
    let (mut current_episode, mut skip_selection) = match &prefer_episode {
        Some(ep) if episodes.contains(ep) => {
            // Valid episode flag — jump directly, skip selection on first pass.
            (ep.clone(), true)
        }
        Some(ep) => {
            // Episode flag provided but doesn't exist — warn, then show list.
            println!(
                "Episode '{}' does not exist for '{}'. Showing episode list.",
                ep, show.title
            );
            (
                last_watched
                    .clone()
                    .unwrap_or_else(|| latest_available.clone()),
                false,
            )
        }
        None => {
            // No episode flag — use history / latest as the default selection.
            (
                last_watched
                    .clone()
                    .unwrap_or_else(|| latest_available.clone()),
                false,
            )
        }
    };

    loop {
        let default_idx = episodes
            .iter()
            .position(|ep| ep == &current_episode)
            .or_else(|| episodes.iter().position(|ep| ep == &latest_available))
            .unwrap_or(0);

        // On the first iteration of a direct-jump, bypass the selection dialog.
        let idx = if skip_selection {
            skip_selection = false; // only skip once
            default_idx
        } else {
            let selection = Select::with_theme(&theme())
                .with_prompt("Episode to play (Enter to select, Esc to cancel)")
                .items(&episodes)
                .default(default_idx)
                .interact_opt()?;
            let Some(i) = selection else {
                println!("Exiting playback loop.");
                return Ok(());
            };
            i
        };

        let chosen = episodes[idx].clone();
        let auto_advance = idx == default_idx;

        println!("Fetching streams for episode {}...", chosen);
        let streams = match client.fetch_streams(&show.id, translation, &chosen).await {
            Ok(streams) => streams,
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
                println!("Error fetching streams: {}", err);
                continue;
            }
        };

        if streams.is_empty() {
            println!(
                "No supported streams found for episode {chosen}. Try another episode or rerun later."
            );
            current_episode = latest_available.clone();
            continue;
        }

        let stream = match choose_stream(streams) {
            Ok(stream) => stream,
            Err(_) => {
                println!("Stream selection cancelled.");
                continue;
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
    for (key, value) in &stream.headers {
        if key.eq_ignore_ascii_case("user-agent") {
            cmd.arg(format!("--user-agent={value}"));
        } else if key.eq_ignore_ascii_case("referer") {
            cmd.arg(format!("--referrer={value}"));
            cmd.arg(format!("--http-header-fields=Referer: {value}"));
        } else {
            cmd.arg(format!("--http-header-fields={}: {value}", key));
        }
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

struct MangaCacheState {
    cached_pages: Vec<Option<PathBuf>>,
    cache_files: Vec<PathBuf>,
}

async fn cache_manga_pages(
    pages: &[Page],
    manga_id: &str,
    translation: Translation,
    chapter: &str,
    cache_base_override: Option<&Path>,
) -> Result<MangaCacheState> {
    let chapter_dir = manga_cache_chapter_dir(manga_id, translation, chapter, cache_base_override)?;
    fs::create_dir_all(&chapter_dir)
        .with_context(|| format!("failed to create cache directory {}", chapter_dir.display()))?;

    let preload_target = INITIAL_MANGA_PAGE_PRELOAD.min(pages.len());
    let cache_files: Vec<PathBuf> = pages
        .iter()
        .enumerate()
        .map(|(idx, page)| {
            let ext = infer_page_extension(&page.url);
            chapter_dir.join(format!("{:04}.{}", idx + 1, ext))
        })
        .collect();

    let http = build_cache_http_client()?;
    let mut cached = vec![None; pages.len()];

    for idx in 0..preload_target {
        let page = &pages[idx];
        let file = &cache_files[idx];

        if file.exists() {
            cached[idx] = Some(file.clone());
            continue;
        }

        match download_page(&http, page, file).await {
            Ok(()) => cached[idx] = Some(file.clone()),
            Err(err) => {
                println!("Cache miss for {}: {}", page.url, err);
            }
        }
    }

    let mut background_jobs: Vec<(Page, PathBuf)> = Vec::new();
    for idx in preload_target..pages.len() {
        let file = &cache_files[idx];
        if file.exists() {
            cached[idx] = Some(file.clone());
            continue;
        }
        background_jobs.push((pages[idx].clone(), file.clone()));
    }

    if !background_jobs.is_empty() {
        std::thread::spawn(move || {
            for (page, file) in background_jobs {
                if file.exists() {
                    continue;
                }
                if let Err(err) = download_page_curl(&page, &file) {
                    println!("Background cache miss for {}: {}", page.url, err);
                }
            }
        });
    }

    Ok(MangaCacheState {
        cached_pages: cached,
        cache_files,
    })
}

async fn download_page(http: &Client, page: &Page, file: &Path) -> Result<()> {
    match download_page_reqwest(http, page, file).await {
        Ok(()) => Ok(()),
        Err(primary_err) => download_page_curl(page, file)
            .with_context(|| format!("reqwest failed first: {primary_err}")),
    }
}

fn build_cache_http_client() -> Result<Client> {
    Client::builder()
        .user_agent(CACHE_USER_AGENT)
        .build()
        .context("failed to create cache HTTP client")
}

fn handle_proxy_request(stream: &mut TcpStream, targets: &[CachedPageTarget]) -> Result<()> {
    let mut reader = BufReader::new(stream.try_clone().context("failed to clone proxy stream")?);
    let mut request_line = String::new();
    let bytes_read = reader
        .read_line(&mut request_line)
        .context("failed to read proxy request")?;
    if bytes_read == 0 {
        return Ok(());
    }

    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let path = parts.next().unwrap_or_default();
    if method != "GET" && method != "HEAD" {
        write_http_error(stream, 405, "method not allowed")?;
        return Ok(());
    }

    let idx = path
        .trim_start_matches('/')
        .split('?')
        .next()
        .unwrap_or_default()
        .parse::<usize>()
        .ok();
    let Some(idx) = idx else {
        write_http_error(stream, 404, "not found")?;
        return Ok(());
    };

    let Some(target) = targets.get(idx) else {
        write_http_error(stream, 404, "not found")?;
        return Ok(());
    };

    if !target.path.exists()
        && let Err(err) = download_page_curl(&target.page, &target.path)
    {
        write_http_error(stream, 502, "cache fetch failed")?;
        return Err(err.context(format!(
            "failed to fetch page {} for proxy",
            target.page.url
        )));
    }

    let data = fs::read(&target.path)
        .with_context(|| format!("failed to read cached file {}", target.path.display()))?;
    if method == "HEAD" {
        write_http_head(stream, data.len(), mime_type_for_path(&target.path))?;
    } else {
        write_http_ok(stream, &data, mime_type_for_path(&target.path))?;
    }
    Ok(())
}

fn write_http_ok(stream: &mut TcpStream, body: &[u8], content_type: &str) -> Result<()> {
    if let Err(err) = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: {}\r\nConnection: close\r\n\r\n",
        body.len(),
        content_type
    ) {
        if is_benign_disconnect(&err) {
            return Ok(());
        }
        return Err(err).context("failed to write proxy headers");
    }
    if let Err(err) = stream.write_all(body) {
        if is_benign_disconnect(&err) {
            return Ok(());
        }
        return Err(err).context("failed to write proxy response body");
    }
    Ok(())
}

fn write_http_head(
    stream: &mut TcpStream,
    content_length: usize,
    content_type: &str,
) -> Result<()> {
    if let Err(err) = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: {}\r\nConnection: close\r\n\r\n",
        content_length, content_type
    ) {
        if is_benign_disconnect(&err) {
            return Ok(());
        }
        return Err(err).context("failed to write proxy head response");
    }
    Ok(())
}

fn write_http_error(stream: &mut TcpStream, status: u16, message: &str) -> Result<()> {
    let body = message.as_bytes();
    let reason = match status {
        404 => "Not Found",
        405 => "Method Not Allowed",
        502 => "Bad Gateway",
        _ => "Internal Server Error",
    };
    if let Err(err) = write!(
        stream,
        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: close\r\n\r\n{}",
        status,
        reason,
        body.len(),
        message
    ) {
        if is_benign_disconnect(&err) {
            return Ok(());
        }
        return Err(err).context("failed to write proxy error response");
    }
    Ok(())
}

fn is_benign_disconnect(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::UnexpectedEof
    )
}

fn is_benign_proxy_error(err: &anyhow::Error) -> bool {
    err.chain()
        .filter_map(|cause| cause.downcast_ref::<std::io::Error>())
        .any(is_benign_disconnect)
}

fn mime_type_for_path(path: &Path) -> &'static str {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        Some("gif") => "image/gif",
        _ => "application/octet-stream",
    }
}

async fn download_page_reqwest(http: &Client, page: &Page, file: &Path) -> Result<()> {
    let mut req = http.get(&page.url).header("Accept", CACHE_ACCEPT);
    for (key, value) in &page.headers {
        req = req.header(key, value);
    }
    let resp = req
        .send()
        .await
        .with_context(|| format!("request failed for {}", page.url))?;
    let status = resp.status();
    if !status.is_success() {
        bail!("HTTP {status}");
    }
    let bytes = resp
        .bytes()
        .await
        .with_context(|| format!("failed to read bytes for {}", page.url))?;
    fs::write(file, bytes.as_ref())
        .with_context(|| format!("failed to write cached page {}", file.display()))?;
    Ok(())
}

fn download_page_curl(page: &Page, file: &Path) -> Result<()> {
    let mut cmd = Command::new("curl");
    cmd.arg("--fail")
        .arg("--location")
        .arg("--silent")
        .arg("--show-error")
        .arg("--user-agent")
        .arg(CACHE_USER_AGENT)
        .arg("--header")
        .arg(format!("Accept: {CACHE_ACCEPT}"));
    for (key, value) in &page.headers {
        cmd.arg("--header").arg(format!("{key}: {value}"));
    }
    cmd.arg("--output").arg(file).arg(&page.url);
    let status = cmd
        .status()
        .with_context(|| "failed to run curl for cache download")?;
    if !status.success() {
        bail!("curl exited with status {status}");
    }
    Ok(())
}

fn manga_cache_chapter_dir(
    manga_id: &str,
    translation: Translation,
    chapter: &str,
    cache_base_override: Option<&Path>,
) -> Result<PathBuf> {
    let base = if let Some(path) = cache_base_override {
        path.to_path_buf()
    } else {
        cache_dir().ok_or_else(|| anyhow!("Could not determine cache directory"))?
    };
    Ok(base
        .join("anv")
        .join("manga-pages")
        .join(sanitize_cache_segment(manga_id))
        .join(translation.as_str())
        .join(sanitize_cache_segment(chapter)))
}

fn sanitize_cache_segment(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        String::from("unknown")
    } else {
        cleaned
    }
}

fn infer_page_extension(url: &str) -> String {
    let path = url.split('?').next().unwrap_or(url);
    match path.rsplit('.').next().map(|s| s.to_ascii_lowercase()) {
        Some(ext)
            if matches!(
                ext.as_str(),
                "jpg" | "jpeg" | "png" | "webp" | "avif" | "gif"
            ) =>
        {
            if ext == "jpeg" {
                String::from("jpg")
            } else {
                ext
            }
        }
        _ => String::from("jpg"),
    }
}

fn history_path() -> Result<PathBuf> {
    let base = data_dir().ok_or_else(|| anyhow!("Could not determine data directory"))?;
    Ok(base.join("anv").join("history.json"))
}

fn theme() -> ColorfulTheme {
    ColorfulTheme::default()
}
