use anv::{
    adapters::mal_client::build_mal_client_if_enabled,
    cli::{Cli, Commands, SyncAction},
    commands::{
        history::run_history_command,
        play_anime::run_anime_command,
        read_manga::run_manga_command,
        sync_mal::{run_sync_disable, run_sync_enable_mal, run_sync_status},
    },
    config::AppConfig,
    history::History,
};

use anyhow::Result;
use clap::Parser;

async fn run() -> Result<()> {
    let cli = Cli::parse();
    let mut history = History::default().load().unwrap_or_else(|err| {
        eprintln!("Warning: failed to load history: {err}");
        History::default()
    });
    let mut config = AppConfig::default();

    match cli.command {
        Some(Commands::History) => {
            return run_history_command(
                &cli,
                &mut history,
                true,
                &config.player,
                None,
                cli.binge || config.binge,
            )
            .await;
        }
        Some(Commands::Sync {
            action: SyncAction::Enable,
        }) => return run_sync_enable_mal(&config).await,
        Some(Commands::Sync {
            action: SyncAction::Status,
        }) => return run_sync_status(&config),
        Some(Commands::Sync {
            action: SyncAction::Disable,
        }) => return run_sync_disable(&mut config).await,
        None => {}
    }

    let mal_client = build_mal_client_if_enabled(&config).await;

    if cli.manga {
        return run_manga_command(&cli, &mut history).await;
    }

    let binge = cli.binge || config.binge;
    run_anime_command(
        &cli,
        &mut history,
        &config.player,
        mal_client.as_ref(),
        binge,
    )
    .await
}

#[tokio::main]
async fn main() -> Result<()> {
    run().await.map_err(|err| {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    })
}
