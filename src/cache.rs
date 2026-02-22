use anyhow::{Context, Result, anyhow, bail};
use dirs_next::cache_dir;
use reqwest::Client;
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use crate::types::{Page, Translation};

pub const CACHE_USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/121.0 Safari/537.36";
pub const CACHE_ACCEPT: &str = "image/avif,image/webp,image/*,*/*;q=0.8";

pub struct MangaCacheState {
    pub cached_pages: Vec<Option<PathBuf>>,
    pub cache_files: Vec<PathBuf>,
    pub cdn_blocked: bool,
}

pub fn build_cache_http_client() -> Result<Client> {
    Client::builder()
        .user_agent(CACHE_USER_AGENT)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() > 5 {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .build()
        .context("failed to create cache HTTP client")
}

pub async fn cache_manga_pages(
    pages: &[Page],
    manga_id: &str,
    translation: Translation,
    chapter: &str,
    cache_base_override: Option<&Path>,
    preload_count: usize,
) -> Result<MangaCacheState> {
    let chapter_dir = manga_cache_chapter_dir(manga_id, translation, chapter, cache_base_override)?;
    fs::create_dir_all(&chapter_dir)
        .with_context(|| format!("failed to create cache directory {}", chapter_dir.display()))?;

    let preload_target = preload_count.min(pages.len());
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
                let msg = err.to_string();
                if msg.contains("403") || msg.contains("Forbidden") {
                    eprintln!(
                        "Image CDN returned 403 \u{2014} this domain is blocked on your network.\n\
                         Try a different provider: --provider mangadex  or  --provider mangapill"
                    );
                    return Ok(MangaCacheState {
                        cached_pages: cached,
                        cache_files,
                        cdn_blocked: true,
                    });
                } else {
                    eprintln!("Cache miss for {}: {}", page.url, err);
                    break;
                }
            }
        }
    }

    if !cached.iter().any(|p| p.is_some()) {
        return Ok(MangaCacheState {
            cached_pages: cached,
            cache_files,
            cdn_blocked: false,
        });
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
                    let msg = err.to_string();
                    if msg.contains("403") || msg.contains("exit status: 22") {
                        break;
                    }
                    eprintln!("Background cache miss for {}: {}", page.url, err);
                }
            }
        });
    }

    Ok(MangaCacheState {
        cached_pages: cached,
        cache_files,
        cdn_blocked: false,
    })
}

pub async fn download_page(http: &Client, page: &Page, file: &Path) -> Result<()> {
    match download_page_reqwest(http, page, file).await {
        Ok(()) => Ok(()),
        Err(primary_err) => download_page_curl(page, file)
            .with_context(|| format!("reqwest failed first: {primary_err}")),
    }
}

async fn download_page_reqwest(http: &Client, page: &Page, file: &Path) -> Result<()> {
    let bytes = fetch_with_headers(http, &page.url, &page.headers).await?;
    fs::write(file, &bytes)
        .with_context(|| format!("failed to write cached page {}", file.display()))?;
    Ok(())
}

async fn fetch_with_headers(
    http: &Client,
    url: &str,
    headers: &HashMap<String, String>,
) -> Result<Vec<u8>> {
    let build_req = |u: &str| {
        let mut req = http.get(u).header("Accept", CACHE_ACCEPT);
        for (key, value) in headers {
            req = req.header(key, value);
        }
        req
    };

    let resp = build_req(url)
        .send()
        .await
        .with_context(|| format!("request failed for {url}"))?;

    // Follow redirects manually to preserve custom Referer/Origin headers.
    let resp = if resp.status().is_redirection() {
        let location = resp
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!("redirect with no Location header"))?;
        build_req(&location)
            .send()
            .await
            .with_context(|| format!("request failed after redirect to {location}"))?
    } else {
        resp
    };

    let status = resp.status();
    if !status.is_success() {
        bail!("HTTP {status}");
    }
    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .with_context(|| format!("failed to read bytes for {url}"))
}

pub fn download_page_curl(page: &Page, file: &Path) -> Result<()> {
    let mut cmd = Command::new("curl");
    cmd.arg("--fail")
        .arg("--location")
        .arg("--silent")
        .arg("--show-error")
        .arg("--location-trusted")
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
        .context("failed to run curl for cache download")?;
    if !status.success() {
        bail!("curl exited with status {status}");
    }
    Ok(())
}

pub fn manga_cache_chapter_dir(
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

pub fn sanitize_cache_segment(value: &str) -> String {
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

pub fn infer_page_extension(url: &str) -> String {
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
