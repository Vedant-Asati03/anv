use crate::{
    adapters::player::{build_command, detect_player},
    proxy::{CachedPageTarget, LocalPageProxy},
    types::Page,
};

use anyhow::{Context, Result, bail};
use std::path::PathBuf;
use tokio::process::Command;

pub struct DefaultReaderGateway;

pub async fn launch_reader(
    pages: &[Page],
    cached_pages: &[Option<PathBuf>],
    cache_files: &[PathBuf],
    title: &str,
    chapter: &str,
) -> Result<()> {
    let player = detect_player();
    let mut cmd = build_command(&player)?;
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
                if !status.success() && status.code() != Some(2) {
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

impl DefaultReaderGateway {
    pub async fn launch_reader(
        &self,
        pages: &[Page],
        cached_pages: &[Option<PathBuf>],
        cache_files: &[PathBuf],
        title: &str,
        chapter: &str,
    ) -> Result<()> {
        launch_reader(pages, cached_pages, cache_files, title, chapter).await
    }
}
