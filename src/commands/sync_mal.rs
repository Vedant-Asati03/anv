use crate::{adapters::mal_client::MalSyncGateway, config::AppConfig};

use anyhow::{Context, Result, bail};

/// `anv sync enable` — authenticates with MAL if needed.
pub async fn run_sync_enable_mal(config: &AppConfig) -> Result<()> {
    let sync_gateway = MalSyncGateway;
    if config.sync.client_id.is_empty() {
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
            config.path.display()
        );
    }

    match sync_gateway.load_token()? {
        Some(token) if !token.is_expired() => {
            println!("Already authenticated with MyAnimeList.");
            println!(
                "To activate sync, set `sync.enabled = true` in your config:\n  {}",
                config.path.display()
            );
            return Ok(());
        }
        _ => {}
    }

    let client_id = config.sync.client_id.clone();
    let token = sync_gateway
        .authenticate(&client_id)
        .await
        .context("MAL OAuth flow failed")?;

    println!("\n✓ Successfully authenticated with MyAnimeList!");
    println!(
        "Token stored at: {}",
        sync_gateway
            .token_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "<unknown>".into())
    );
    println!(
        "\nTo activate sync, set `sync.enabled = true` in:\n  {}",
        config.path.display()
    );
    let _ = token;
    Ok(())
}

/// `anv sync status` — show current sync/auth state.
pub fn run_sync_status(config: &AppConfig) -> Result<()> {
    let sync_gateway = MalSyncGateway;
    println!("── MAL Sync Status ──");
    println!(
        "  sync.enabled : {}",
        if config.sync.enabled { "yes" } else { "no" }
    );

    if config.sync.client_id.is_empty() {
        println!(
            "  client_id    : not set  (add to {})",
            config.path.display()
        );
    } else {
        let masked = format!(
            "{}…",
            &config.sync.client_id[..config.sync.client_id.len().min(8)]
        );
        println!("  client_id    : {}", masked);
    }

    match sync_gateway.load_token() {
        Ok(Some(token)) => {
            if token.is_expired() {
                println!("  token        : expired  (run `anv sync enable` to refresh)");
            } else {
                println!(
                    "  token        : valid, expires {}",
                    token.expires_at.format("%Y-%m-%d %H:%M UTC")
                );
            }
        }
        Ok(None) => println!("  token        : not found  (run `anv sync enable`)"),
        Err(err) => println!("  token        : error reading ({err})"),
    }
    Ok(())
}

/// `anv sync disable` — set sync.enabled = false and write config.
pub async fn run_sync_disable(config: &mut AppConfig) -> Result<()> {
    if !config.sync.enabled {
        println!("Sync is already disabled.");
        return Ok(());
    }
    config.sync.enabled = false;
    config.save().context("failed to save config")?;
    println!(
        "Sync disabled. Edit {} to re-enable.",
        config.path.display()
    );
    Ok(())
}
