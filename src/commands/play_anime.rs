use crate::{
    adapters::{
        mal_client::{
            AnimeInfo, MalClient, MalIdCache, SyncUpdate, WatchStatus, should_confirm_sync,
        },
        player::DefaultPlayerGateway,
        providers::allanime::AllAnimeClient,
    },
    cli::Cli,
    domain::services::progression::{next_label_presorted, sorted_labels_numeric},
    history::{History, HistoryEntry},
    ports::{providers::AnimeProvider},
    prompt::{confirm, rate, select_episode, select_show_entry},
    types::{Provider, ShowInfo, Translation},
};

use anyhow::{Result, bail};
use chrono::Utc;
use reqwest::StatusCode;

pub async fn run_anime_command(
    cli: &Cli,
    history: &mut History,
    player: &String,
    mal_client: Option<&MalClient>,
    binge: bool,
) -> Result<()> {
    let translation = if cli.dub {
        Translation::Dub
    } else {
        Translation::Sub
    };

    if !matches!(cli.provider, Provider::Allanime) {
        eprintln!("Warning: Only 'allanime' provider supports anime. Switching to 'allanime'.");
    }

    run_anime_flow(cli, history, translation, player, mal_client, binge).await
}

pub async fn run_anime_flow(
    cli: &Cli,
    history: &mut History,
    translation: Translation,
    player: &String,
    mal_client: Option<&MalClient>,
    binge: bool,
) -> Result<()> {
    let client = AllAnimeClient::new()?;

    if cli.query.is_empty() {
        println!("No query provided. Use `anv <name>` or `anv history`.");
        return Ok(());
    }

    let query = cli.query.join(" ");
    let shows = client.search_shows(&query, translation).await?;
    if shows.is_empty() {
        bail!("No results for \"{}\" ({})", query, translation.label());
    }

    let Some(show) = select_show_entry(&shows, translation)? else {
        println!("Cancelled.");
        return Ok(());
    };

    play_show(
        history,
        &client,
        translation,
        Provider::Allanime,
        show,
        cli.episode.clone(),
        &player,
        mal_client,
        binge,
    )
    .await
}

pub(crate) async fn play_show(
    history: &mut History,
    client: &impl AnimeProvider,
    translation: Translation,
    provider: Provider,
    show: &ShowInfo,
    prefer_episode: Option<String>,
    player: &str,
    mal_client: Option<&MalClient>,
    binge: bool,
) -> Result<()> {
    let player_gateway = DefaultPlayerGateway;
    let episodes = client.fetch_episodes(&show.id, translation).await?;
    if episodes.is_empty() {
        bail!(
            "No {} episodes available for {}",
            translation.label(),
            show.title
        );
    }

    let sorted_episodes = sorted_labels_numeric(&episodes);

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

    let mut mal_id_cache = if mal_client.is_some() {
        MalIdCache::load().unwrap_or_else(|err| {
            eprintln!("[sync] Warning: could not load ID cache ({err}), starting fresh.");
            MalIdCache::default()
        })
    } else {
        MalIdCache::default()
    };

    loop {
        let default_idx = episodes
            .iter()
            .position(|ep| ep == &current_episode)
            .or_else(|| episodes.iter().position(|ep| ep == &latest_available))
            .unwrap_or(0);

        let idx = if skip_selection || binge {
            skip_selection = false;
            default_idx
        } else {
            let Some(i) = select_episode(
                &episodes,
                default_idx,
                "Episode to play (type to search, Esc to cancel)",
            )?
            else {
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

        let Some(stream) = player_gateway.choose_stream(streams)? else {
            continue;
        };

        let next_candidate = next_label_presorted(&chosen, &sorted_episodes);

        player_gateway
            .launch_player(&stream, &show.title, &chosen, player)
            .await?;

        history.upsert(HistoryEntry {
            show_id: show.id.clone(),
            show_title: show.title.clone(),
            episode: chosen.clone(),
            translation,
            provider,
            is_manga: false,
            watched_at: Utc::now(),
        });
        history.save()?;

        if let Some(mal) = mal_client {
            let ep_num = chosen.parse::<u32>().unwrap_or(0);

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
                    Ok(None) => None,
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

                let new_status = if anime_info.num_episodes > 0 && ep_num >= anime_info.num_episodes
                {
                    WatchStatus::Completed
                } else {
                    WatchStatus::Watching
                };

                let needs_confirm = should_confirm_sync(&current, new_status);

                let should_update = confirm(&format!(
                    "[sync] Update MAL: \"{}\" ep {} → {}?",
                    show.title,
                    ep_num,
                    new_status.label()
                ))?;

                if should_update {
                    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
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
                    let finish_date = if new_status == WatchStatus::Completed {
                        Some(today)
                    } else {
                        None
                    };

                    let score: Option<u8> = if new_status == WatchStatus::Completed {
                        let rating_idx = rate(&show.title)?;

                        match rating_idx {
                            Some(idx) if idx < 10 => Some(idx as u8 + 1),
                            _ => None,
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
        match (auto_advance || binge, next_candidate) {
            (true, Some(next)) => current_episode = next,
            (true, None) => {
                println!("No further episodes found. Exiting.");
                return Ok(());
            }
            (false, candidate) => current_episode = candidate.unwrap_or(chosen),
        }
    }
}
