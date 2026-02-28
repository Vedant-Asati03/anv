use std::{
    cmp::Ordering,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use clap::{Parser, Subcommand};
use config::AppConfig;
use dialoguer::{Confirm, FuzzySelect, Select, theme::ColorfulTheme};
use reqwest::StatusCode;

mod cache;
mod config;
mod history;
mod player;
mod providers;
mod proxy;
mod sync;
mod types;

use cache::{MangaCacheState, cache_manga_pages};
use history::{History, HistoryEntry, history_path, theme};
use player::{choose_stream, launch_image_viewer, launch_player};
use providers::{
    AnimeProvider, MangaProvider, allanime::AllAnimeClient, mangadex::MangaDexClient,
    mangapill::MangapillClient,
};
use sync::{
    SyncUpdate, WatchStatus,
    mal::{AnimeInfo, MalClient, MalIdCache, MalToken, build_mal_client_if_enabled},
    should_confirm_sync,
};
use types::{ChapterCounts, EpisodeCounts, MangaInfo, Provider, ShowInfo, Translation};

const INITIAL_MANGA_PAGE_PRELOAD: usize = 5;

#[derive(Debug, Parser)]
#[command(name = "anv", about = "Stream anime or read manga via mpv.", version)]
struct Cli {
    #[arg(long)]
    dub: bool,
    #[arg(long)]
    raw: bool,
    #[arg(long)]
    history: bool,
    #[arg(long)]
    manga: bool,
    #[arg(long, default_value = "allanime", value_enum)]
    provider: Provider,
    #[arg(long, value_name = "DIR")]
    cache_dir: Option<PathBuf>,
    #[arg(short = 'e', long, value_name = "EPISODE")]
    episode: Option<String>,
    #[arg(value_name = "QUERY")]
    query: Vec<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Manage sync with external anime list services.
    Sync {
        #[command(subcommand)]
        action: SyncAction,
    },
}

#[derive(Debug, Subcommand)]
enum SyncAction {
    /// Enable sync with a list provider and authenticate.
    Enable {
        #[command(subcommand)]
        provider: SyncProviderCmd,
    },
    /// Show current sync status and MAL authentication state.
    Status,
    /// Disable MAL sync (can be re-enabled by editing config).
    Disable,
}

#[derive(Debug, Subcommand)]
enum SyncProviderCmd {
    /// Authenticate with MyAnimeList (runs OAuth flow if no token stored).
    Mal,
}

#[tokio::main]
async fn main() -> Result<()> {
    run().await.map_err(|err| {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    })
}

async fn run() -> Result<()> {
    let cli = Cli::parse();
    let cfg = AppConfig::load().unwrap_or_else(|err| {
        eprintln!("Warning: failed to load config ({err}), using defaults.");
        AppConfig::default()
    });

    // Handle sync subcommands
    match &cli.command {
        Some(Commands::Sync {
            action:
                SyncAction::Enable {
                    provider: SyncProviderCmd::Mal,
                },
        }) => return run_sync_enable_mal(&cfg).await,
        Some(Commands::Sync {
            action: SyncAction::Status,
        }) => return run_sync_status(&cfg),
        Some(Commands::Sync {
            action: SyncAction::Disable,
        }) => return run_sync_disable(cfg).await,
        _ => {}
    }

    let history_mode =
        cli.history || (cli.query.len() == 1 && cli.query[0].eq_ignore_ascii_case("history"));
    let history_path = history_path()?;
    let mut history = History::load(&history_path)?;

    // Build MAL client if sync is enabled and a token exists
    let mal_client = build_mal_client_if_enabled(&cfg);

    if cli.manga {
        let translation = if cli.raw {
            Translation::Raw
        } else {
            Translation::Sub
        };
        match cli.provider {
            Provider::Allanime => {
                let client = AllAnimeClient::new()?;
                return run_manga_flow(&cli, translation, &mut history, &history_path, &client)
                    .await;
            }
            Provider::Mangadex => {
                let client = MangaDexClient::new()?;
                return run_manga_flow(&cli, translation, &mut history, &history_path, &client)
                    .await;
            }
            Provider::Mangapill => {
                let client = MangapillClient::new()?;
                return run_manga_flow(&cli, translation, &mut history, &history_path, &client)
                    .await;
            }
        }
    }

    let translation = if cli.dub {
        Translation::Dub
    } else {
        Translation::Sub
    };

    if !matches!(cli.provider, Provider::Allanime) {
        eprintln!("Warning: Only 'allanime' provider supports anime. Switching to 'allanime'.");
    }
    run_anime_flow(
        &cli,
        translation,
        history_mode,
        &mut history,
        &history_path,
        cfg.player.clone(),
        mal_client.as_ref(),
    )
    .await
}

/// `anv sync enable mal` — authenticates with MAL if needed.
async fn run_sync_enable_mal(cfg: &AppConfig) -> Result<()> {
    if cfg.mal.client_id.is_empty() {
        bail!(
            "MAL client_id is not set.\n\
             1. Go to https://myanimelist.net/apiconfig and create an application.\n\
             2. Set the app type to 'other' and redirect URI to: http://localhost:11422/callback\n\
             3. Copy the Client ID and add it to your config:\n\
             \n\
             [mal]\n\
             client_id = \"<your-client-id>\"\n\
             \n\
             Config location: {}",
            AppConfig::config_path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "<unknown>".into())
        );
    }

    // Check for an existing valid token
    match MalToken::load()? {
        Some(token) if !token.is_expired() => {
            println!("Already authenticated with MyAnimeList.");
            println!(
                "To activate sync, set `sync.enabled = true` in your config:\n  {}",
                AppConfig::config_path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| "<unknown>".into())
            );
            return Ok(());
        }
        _ => {}
    }

    // No valid token — run OAuth PKCE flow
    // OAuth involves blocking I/O; run it in a blocking thread so we don't
    // block the async executor.
    let client_id = cfg.mal.client_id.clone();
    let token = tokio::task::spawn_blocking(move || MalClient::authenticate(&client_id))
        .await
        .context("OAuth task panicked")?
        .context("MAL OAuth flow failed")?;

    println!("\n✓ Successfully authenticated with MyAnimeList!");
    println!(
        "Token stored at: {}",
        MalToken::token_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "<unknown>".into())
    );
    println!(
        "\nTo activate sync, set `sync.enabled = true` in:\n  {}",
        AppConfig::config_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "<unknown>".into())
    );
    let _ = token; // already saved inside authenticate()
    Ok(())
}

/// `anv sync status` — show current sync/auth state.
fn run_sync_status(cfg: &AppConfig) -> Result<()> {
    let config_path = AppConfig::config_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "<unknown>".into());

    println!("── MAL Sync Status ──");
    println!(
        "  sync.enabled : {}",
        if cfg.sync.enabled { "yes" } else { "no" }
    );

    if cfg.mal.client_id.is_empty() {
        println!("  client_id    : not set  (add to {})", config_path);
    } else {
        let masked = format!("{}…", &cfg.mal.client_id[..cfg.mal.client_id.len().min(8)]);
        println!("  client_id    : {}", masked);
    }

    match MalToken::load() {
        Ok(Some(token)) => {
            if token.is_expired() {
                println!("  token        : expired  (run `anv sync enable mal` to refresh)");
            } else {
                println!(
                    "  token        : valid, expires {}",
                    token.expires_at.format("%Y-%m-%d %H:%M UTC")
                );
            }
        }
        Ok(None) => println!("  token        : not found  (run `anv sync enable mal`)"),
        Err(err) => println!("  token        : error reading ({err})"),
    }
    Ok(())
}

/// `anv sync disable` — set sync.enabled = false and write config.
async fn run_sync_disable(mut cfg: AppConfig) -> Result<()> {
    if !cfg.sync.enabled {
        println!("Sync is already disabled.");
        return Ok(());
    }
    cfg.sync.enabled = false;
    cfg.save().context("failed to save config")?;
    println!(
        "Sync disabled. Edit {} to re-enable.",
        AppConfig::config_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "<unknown>".into())
    );
    Ok(())
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
                Translation::Dub => 0,
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
        cli.episode.clone(),
        cli.cache_dir.as_deref(),
        cli.provider,
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
    provider: Provider,
) -> Result<()> {
    let chapters = match client.fetch_chapters(&manga.id, translation).await {
        Ok(c) => c,
        Err(err) => {
            let msg = err.to_string();
            if msg.contains("connection closed")
                || msg.contains("SendRequest")
                || msg.contains("connect")
            {
                bail!(
                    "Could not connect to the provider \u{2014} your network may be blocking it.\nTry a different provider: --provider mangadex  or  --provider mangapill"
                );
            }
            return Err(err);
        }
    };
    if chapters.is_empty() {
        bail!(
            "No {} chapters available for {}",
            translation.label(),
            manga.title
        );
    }

    let chapter_labels: Vec<String> = chapters.iter().map(|c| c.label.clone()).collect();
    let sorted_labels = sorted_episode_labels(&chapter_labels);

    let latest_available = sorted_labels
        .last()
        .cloned()
        .expect("chapters is non-empty; bail!() above ensures this");
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

    let fallback = last_read.unwrap_or_else(|| latest_available.clone());
    let (mut current_label, mut skip_selection) = match &prefer_chapter {
        Some(ch) if chapter_labels.contains(ch) => (ch.clone(), true),
        Some(ch) => {
            println!(
                "Chapter '{}' does not exist for '{}'. Showing chapter list.",
                ch, manga.title
            );
            (fallback, false)
        }
        None => (fallback, false),
    };

    let theme = theme();
    loop {
        let default_idx = chapter_labels
            .iter()
            .position(|ch| ch == &current_label)
            .or_else(|| chapter_labels.iter().position(|ch| ch == &latest_available))
            .unwrap_or(0);

        let idx = if skip_selection {
            skip_selection = false;
            default_idx
        } else {
            let selection = FuzzySelect::with_theme(&theme)
                .with_prompt("Chapter to read (type to search, Esc to cancel)")
                .items(&chapter_labels)
                .default(default_idx)
                .interact_opt()?;
            let Some(i) = selection else {
                println!("Exiting reading loop.");
                return Ok(());
            };
            i
        };

        let chosen_label = chapter_labels[idx].clone();
        let chapter_id = chapters[idx].id.clone();
        let auto_advance = idx == default_idx;

        let pages = match client
            .fetch_pages(&manga.id, translation, &chapter_id)
            .await
        {
            Ok(pages) => pages,
            Err(err) => {
                eprintln!(
                    "Failed to fetch pages for chapter {}: {}",
                    chosen_label, err
                );
                continue;
            }
        };

        if pages.is_empty() {
            eprintln!("No pages found for chapter {}.", chosen_label);
            continue;
        }

        let next_candidate = next_episode_label_presorted(&chosen_label, &sorted_labels);
        let cache_state = match cache_manga_pages(
            &pages,
            &manga.id,
            translation,
            &chosen_label,
            cache_base_override,
            INITIAL_MANGA_PAGE_PRELOAD,
        )
        .await
        {
            Ok(state) => {
                let cached_count = state.cached_pages.iter().filter(|p| p.is_some()).count();
                if cached_count > 0 {
                    println!("Caching chapter pages locally...");
                    println!(
                        "Cached {cached_count}/{} pages upfront for Chapter {} (first {} pages).",
                        pages.len(),
                        chosen_label,
                        INITIAL_MANGA_PAGE_PRELOAD
                    );
                    if pages.len() > INITIAL_MANGA_PAGE_PRELOAD {
                        println!("Continuing to cache remaining pages in background...");
                    }
                }
                state
            }
            Err(err) => {
                eprintln!(
                    "Page cache unavailable for Chapter {} ({}). Falling back to streaming URLs.",
                    chosen_label, err
                );
                MangaCacheState {
                    cached_pages: vec![None; pages.len()],
                    cache_files: Vec::new(),
                    cdn_blocked: false,
                }
            }
        };

        if cache_state.cdn_blocked {
            if auto_advance {
                if let Some(next) = next_candidate {
                    current_label = next;
                }
            }
            continue;
        }

        launch_image_viewer(
            &pages,
            &cache_state.cached_pages,
            &cache_state.cache_files,
            &manga.title,
            &chosen_label,
        )
        .await?;

        history.upsert(HistoryEntry {
            show_id: manga.id.clone(),
            show_title: manga.title.clone(),
            episode: chosen_label.clone(),
            translation,
            provider,
            is_manga: true,
            watched_at: Utc::now(),
        });
        history.save(history_path)?;

        match (auto_advance, next_candidate) {
            (true, Some(next)) => current_label = next,
            (true, None) => {
                println!("No further chapters found. Exiting.");
                return Ok(());
            }
            (false, candidate) => current_label = candidate.unwrap_or(chosen_label),
        }
    }
}

async fn run_anime_flow(
    cli: &Cli,
    translation: Translation,
    history_mode: bool,
    history: &mut History,
    history_path: &Path,
    player: String,
    mal_client: Option<&MalClient>,
) -> Result<()> {
    let client = AllAnimeClient::new()?;

    if history_mode {
        if let Some(entry) = history.select_entry()? {
            if entry.is_manga {
                let manga_info = MangaInfo {
                    id: entry.show_id.clone(),
                    title: entry.show_title.clone(),
                    available_chapters: ChapterCounts::default(),
                };
                match entry.provider {
                    Provider::Allanime => {
                        read_manga(
                            &AllAnimeClient::new()?,
                            entry.translation,
                            manga_info,
                            history,
                            history_path,
                            Some(entry.episode.clone()),
                            cli.cache_dir.as_deref(),
                            entry.provider,
                        )
                        .await?
                    }
                    Provider::Mangadex => {
                        read_manga(
                            &MangaDexClient::new()?,
                            entry.translation,
                            manga_info,
                            history,
                            history_path,
                            Some(entry.episode.clone()),
                            cli.cache_dir.as_deref(),
                            entry.provider,
                        )
                        .await?
                    }
                    Provider::Mangapill => {
                        read_manga(
                            &MangapillClient::new()?,
                            entry.translation,
                            manga_info,
                            history,
                            history_path,
                            Some(entry.episode.clone()),
                            cli.cache_dir.as_deref(),
                            entry.provider,
                        )
                        .await?
                    }
                }
            } else {
                play_show(
                    &client,
                    history,
                    history_path,
                    entry.translation,
                    Provider::Allanime,
                    ShowInfo {
                        id: entry.show_id.clone(),
                        title: entry.show_title.clone(),
                        available_eps: EpisodeCounts::default(),
                    },
                    Some(entry.episode.clone()),
                    &player,
                    mal_client,
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
                Translation::Raw => 0,
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
        Provider::Allanime,
        show,
        cli.episode.clone(),
        &player,
        mal_client,
    )
    .await
}

async fn play_show(
    client: &impl AnimeProvider,
    history: &mut History,
    history_path: &Path,
    translation: Translation,
    provider: Provider,
    show: ShowInfo,
    prefer_episode: Option<String>,
    player: &str,
    mal_client: Option<&MalClient>,
) -> Result<()> {
    let episodes = client.fetch_episodes(&show.id, translation).await?;
    if episodes.is_empty() {
        bail!(
            "No {} episodes available for {}",
            translation.label(),
            show.title
        );
    }

    let sorted_episodes = sorted_episode_labels(&episodes);

    let latest_available = sorted_episodes
        .last()
        .cloned()
        .expect("episodes is non-empty; bail!() above ensures this");
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

    let fallback = last_watched.unwrap_or_else(|| latest_available.clone());
    let (mut current_episode, mut skip_selection) = match &prefer_episode {
        Some(ep) if episodes.contains(ep) => (ep.clone(), true),
        Some(ep) => {
            println!(
                "Episode '{}' does not exist for '{}'. Showing episode list.",
                ep, show.title
            );
            (fallback, false)
        }
        None => (fallback, false),
    };

    // Load the persistent AllAnime→MAL ID cache once per play_show invocation.
    // For non-sync sessions (mal_client is None), this is a no-op load.
    let mut mal_id_cache = if mal_client.is_some() {
        MalIdCache::load().unwrap_or_else(|err| {
            eprintln!("[sync] Warning: could not load ID cache ({err}), starting fresh.");
            MalIdCache::default()
        })
    } else {
        MalIdCache::default()
    };

    let theme = theme();
    loop {
        let default_idx = episodes
            .iter()
            .position(|ep| ep == &current_episode)
            .or_else(|| episodes.iter().position(|ep| ep == &latest_available))
            .unwrap_or(0);

        let idx = if skip_selection {
            skip_selection = false;
            default_idx
        } else {
            let selection = FuzzySelect::with_theme(&theme)
                .with_prompt("Episode to play (type to search, Esc to cancel)")
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
                        eprintln!(
                            "Episode {chosen} is not yet available for {} translation.",
                            translation.label()
                        );
                        current_episode = latest_available.clone();
                        continue;
                    }
                }
                eprintln!("Error fetching streams: {}", err);
                continue;
            }
        };

        if streams.is_empty() {
            eprintln!(
                "No supported streams found for episode {chosen}. Try another episode or rerun later."
            );
            current_episode = latest_available.clone();
            continue;
        }

        let Some(stream) = choose_stream(streams)? else {
            continue;
        };

        let next_candidate = next_episode_label_presorted(&chosen, &sorted_episodes);

        launch_player(&stream, &show.title, &chosen, player).await?;

        history.upsert(HistoryEntry {
            show_id: show.id.clone(),
            show_title: show.title.clone(),
            episode: chosen.clone(),
            translation,
            provider,
            is_manga: false,
            watched_at: Utc::now(),
        });
        history.save(history_path)?;

        // MAL sync: look up cached MAL ID or resolve+confirm once, then update
        if let Some(mal) = mal_client {
            let ep_num = chosen.parse::<u32>().unwrap_or(0);

            // Resolve MAL ID: check persistent cache first, confirm only on miss
            let mal_id_opt = if let Some(cached_id) = mal_id_cache.get(&show.id) {
                Some(cached_id)
            } else {
                match mal.resolve_and_confirm_mal_id(&show.title).await {
                    Ok(Some(id)) => {
                        if let Err(err) = mal_id_cache.insert_and_save(&show.id, id) {
                            eprintln!("[sync] Warning: could not save ID cache: {err}");
                        }
                        Some(id)
                    }
                    Ok(None) => None, // user declined
                    Err(err) => {
                        eprintln!("[sync] MAL ID resolution failed: {err}");
                        None
                    }
                }
            };

            if let Some(mal_id) = mal_id_opt {
                let anime_info = mal.get_anime_info(mal_id).await.unwrap_or_else(|err| {
                    eprintln!(
                        "[sync] Warning: could not fetch anime info ({err}), assuming Watching."
                    );
                    AnimeInfo {
                        list_status: None,
                        num_episodes: 0,
                    }
                });
                let current = anime_info.list_status;

                // Use MAL's planned total as the source of truth.
                // If num_episodes == 0, MAL doesn't know the total yet (still
                // airing / TBA) — never auto-mark as Completed in that case.
                let new_status = if anime_info.num_episodes > 0 && ep_num >= anime_info.num_episodes
                {
                    WatchStatus::Completed
                } else {
                    WatchStatus::Watching
                };

                let needs_confirm = should_confirm_sync(&current, new_status);

                let should_update = if needs_confirm {
                    // Status is changing or not on list — ask the user
                    Confirm::with_theme(&ColorfulTheme::default())
                        .with_prompt(format!(
                            "[sync] Update MAL: \"{}\" ep {} → {}?",
                            show.title,
                            ep_num,
                            new_status.label()
                        ))
                        .default(false)
                        .interact()
                        .unwrap_or(false)
                } else {
                    // Only episode count advanced, same status — silent
                    true
                };

                if should_update {
                    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
                    // Set start_date when first *actually starting* to watch:
                    // - not on list at all, OR
                    // - was plan_to_watch (never actually started)
                    let is_first_start = new_status == WatchStatus::Watching
                        && match &current {
                            None => true,
                            Some(cur) => cur.status == "plan_to_watch",
                        };
                    let start_date = if is_first_start {
                        Some(today.clone())
                    } else {
                        None
                    };
                    // Set finish_date whenever marking as completed
                    let finish_date = if new_status == WatchStatus::Completed {
                        Some(today)
                    } else {
                        None
                    };

                    // If the anime is now complete, ask the user to rate it before
                    let score: Option<u8> = if new_status == WatchStatus::Completed {
                        let mut rating_options: Vec<String> =
                            (1u8..=10).map(|n| format!("{}/10", n)).collect();
                        rating_options.push("Skip (no rating)".to_string());

                        let rating_idx = Select::with_theme(&ColorfulTheme::default())
                            .with_prompt(format!("Rate \"{}\" on MAL (Esc to skip)", show.title))
                            .items(&rating_options)
                            .default(rating_options.len() - 1) // default to Skip
                            .interact_opt()?;

                        match rating_idx {
                            Some(idx) if idx < 10 => Some(idx as u8 + 1),
                            _ => None, // Skip selected or Esc pressed
                        }
                    } else {
                        None
                    };

                    let update = SyncUpdate {
                        title: show.title.clone(),
                        episode: ep_num,
                        total_episodes: if anime_info.num_episodes > 0 {
                            Some(anime_info.num_episodes)
                        } else {
                            None
                        },
                        status: new_status,
                        start_date,
                        finish_date,
                        score,
                    };
                    match mal.update_status_with_id(mal_id, &update).await {
                        Ok(()) => {
                            if needs_confirm {
                                println!(
                                    "[sync] MAL updated: ep {} → {}",
                                    ep_num,
                                    new_status.label()
                                );
                            } else {
                                println!("[sync] MAL progress saved: ep {}", ep_num);
                            }
                            if let Some(score_val) = score {
                                println!("[sync] MAL score submitted: {}/10", score_val);
                            } else if new_status == WatchStatus::Completed {
                                println!("[sync] Rating skipped.");
                            }
                        }
                        Err(err) => eprintln!("[sync] Failed to update MAL: {err}"),
                    }
                } else {
                    println!("[sync] Skipped MAL update.");
                }
            }
        };
        match (auto_advance, next_candidate) {
            (true, Some(next)) => current_episode = next,
            (true, None) => {
                println!("No further episodes found. Exiting.");
                return Ok(());
            }
            (false, candidate) => current_episode = candidate.unwrap_or(chosen),
        }
    }
}

fn parse_episode_key(label: &str) -> f64 {
    label.parse::<f64>().unwrap_or(0.0)
}

fn sorted_episode_labels(episodes: &[String]) -> Vec<String> {
    let mut sorted = episodes.to_vec();
    sorted.sort_by(|a, b| {
        parse_episode_key(a)
            .partial_cmp(&parse_episode_key(b))
            .unwrap_or(Ordering::Equal)
    });
    sorted.dedup();
    sorted
}

fn next_episode_label_presorted(current: &str, sorted: &[String]) -> Option<String> {
    let pos = sorted.iter().position(|ep| ep == current)?;
    sorted.get(pos + 1).cloned()
}
