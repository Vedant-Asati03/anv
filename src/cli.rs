use crate::types::Provider;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "anv", about = "Stream anime or read manga via mpv.", version)]
pub struct Cli {
    #[arg(long)]
    pub dub: bool,

    #[arg(long)]
    pub raw: bool,

    #[arg(long)]
    pub manga: bool,

    #[arg(long)]
    pub binge: bool,

    #[arg(long, default_value = "allanime", value_enum)]
    pub provider: Provider,

    #[arg(long, value_name = "DIR")]
    pub cache_dir: Option<PathBuf>,

    #[arg(short = 'e', long, value_name = "EPISODE")]
    pub episode: Option<String>,

    #[arg(value_name = "QUERY")]
    pub query: Vec<String>,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Open watch/read history and replay an entry.
    History,
    /// Manage sync with external anime list services.
    Sync {
        #[command(subcommand)]
        action: SyncAction,
    },
}

#[derive(Debug, Subcommand)]
pub enum SyncAction {
    /// Enable MAL sync and authenticate.
    Enable,
    /// Show current sync status and MAL authentication state.
    Status,
    /// Disable MAL sync (can be re-enabled by editing config).
    Disable,
}
