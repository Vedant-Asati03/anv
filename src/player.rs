use anyhow::{Context, Result, anyhow, bail};
use dialoguer::Select;
use std::path::PathBuf;
use tokio::process::Command;

use crate::history::theme;
use crate::proxy::{CachedPageTarget, LocalPageProxy};
use crate::types::{Page, StreamOption};

pub const PLAYER_ENV_KEY: &str = "ANV_PLAYER";

pub fn detect_player() -> String {
    std::env::var(PLAYER_ENV_KEY)
        .ok()
        .filter(|val| !val.trim().is_empty())
        .unwrap_or_else(|| "mpv".to_string())
}

pub fn choose_stream(mut options: Vec<StreamOption>) -> Result<StreamOption> {
    if options.len() == 1 {
        return Ok(options.remove(0));
    }
    let labels: Vec<String> = options.iter().map(StreamOption::label).collect();
    let selection = Select::with_theme(&theme())
        .with_prompt("Select a stream")
        .items(&labels)
        .default(0)
        .interact_opt()?;
    let Some(idx) = selection else {
        bail!("Stream selection cancelled.");
    };
    Ok(options.remove(idx))
}

pub async fn launch_player(stream: &StreamOption, title: &str, episode: &str) -> Result<()> {
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

    let status = match cmd.status().await {
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

pub async fn launch_image_viewer(
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

    if !cached_pages.iter().any(|p| p.is_some()) {
        add_direct_url_args(&mut cmd, pages);
    } else if cached_pages.iter().all(|p| p.is_some()) {
        for path in cached_pages.iter().flatten() {
            cmd.arg(path);
        }
    } else {
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
                println!("Launching viewer for Chapter {chapter}...");
                let status = cmd.status().await.context("failed to launch viewer")?;
                proxy.shutdown();
                if !status.success() {
                    bail!("viewer exited with status {status}");
                }
                return Ok(());
            }
            Err(err) => {
                eprintln!("Local cache proxy unavailable ({err}). Falling back to direct URLs.");
                add_direct_url_args(&mut cmd, pages);
            }
        }
    }

    println!("Launching viewer for Chapter {chapter}...");
    let status = cmd.status().await.context("failed to launch viewer")?;
    if !status.success() && status.code() != Some(2) {
        bail!("viewer exited with status {status}");
    }
    Ok(())
}

fn add_direct_url_args(cmd: &mut Command, pages: &[Page]) {
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
