use crate::{
    cli::Cli,
    commands::{play_anime, read_manga},
    history::History,
    infra::{
        providers::{
            allanime::AllAnimeClient, mangadex::MangaDexClient, mangapill::MangapillClient,
        },
        sync::mal_client::MalClient,
    },
    prompt::select_history_entry,
    types::{ChapterCounts, EpisodeCounts, MangaInfo, Provider, ShowInfo},
};

use anyhow::Result;

pub async fn run_history_command(
    cli: &Cli,
    history: &mut History,
    history_mode: bool,
    player: &String,
    mal_client: Option<&MalClient>,
    binge: bool,
) -> Result<()> {
    if history_mode {
        if let Some(entry) = select_history_entry(history)? {
            if entry.is_manga {
                let manga_info = MangaInfo {
                    id: entry.show_id.clone(),
                    title: entry.show_title.clone(),
                    available_chapters: ChapterCounts::default(),
                };
                match entry.provider {
                    Provider::Allanime => {
                        read_manga::read_manga(
                            history,
                            &AllAnimeClient::new()?,
                            entry.translation,
                            &manga_info,
                            Some(entry.episode.clone()),
                            cli.cache_dir.as_deref(),
                            entry.provider,
                        )
                        .await?
                    }
                    Provider::Mangadex => {
                        read_manga::read_manga(
                            history,
                            &MangaDexClient::new()?,
                            entry.translation,
                            &manga_info,
                            Some(entry.episode.clone()),
                            cli.cache_dir.as_deref(),
                            entry.provider,
                        )
                        .await?
                    }
                    Provider::Mangapill => {
                        read_manga::read_manga(
                            history,
                            &MangapillClient::new()?,
                            entry.translation,
                            &manga_info,
                            Some(entry.episode.clone()),
                            cli.cache_dir.as_deref(),
                            entry.provider,
                        )
                        .await?
                    }
                }
            } else {
                let show_info = ShowInfo {
                    id: entry.show_id.clone(),
                    title: entry.show_title.clone(),
                    available_eps: EpisodeCounts::default(),
                };

                play_anime::play_show(
                    history,
                    &AllAnimeClient::new()?,
                    entry.translation,
                    entry.provider,
                    &show_info,
                    Some(entry.episode.clone()),
                    player,
                    mal_client,
                    binge,
                )
                .await?;
            }
        }
        return Ok(());
    }
    Ok(())
}
