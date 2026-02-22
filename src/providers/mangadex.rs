use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Result, bail};
use reqwest::Client;
use serde::Deserialize;

use super::MangaProvider;
use crate::types::{Chapter, ChapterCounts, MangaInfo, Page, Translation};

const MANGADEX_API_URL: &str = "https://api.mangadex.org";
const CHAPTER_PAGE_LIMIT: usize = 500;

pub struct MangaDexClient {
    client: Client,
}

impl MangaDexClient {
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .user_agent(concat!("anv/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self { client })
    }

    async fn fetch_manga_feed(
        &self,
        manga_id: &str,
        limit: usize,
        offset: usize,
        languages: &[&str],
    ) -> Result<MangaFeedResponse> {
        let mut query = vec![
            ("limit", limit.to_string()),
            ("offset", offset.to_string()),
            ("order[chapter]", "desc".to_string()),
        ];
        for lang in languages {
            query.push(("translatedLanguage[]", lang.to_string()));
        }

        let url = format!("{}/manga/{}/feed", MANGADEX_API_URL, manga_id);
        let response = self.client.get(&url).query(&query).send().await?;

        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let wait = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(2);
            tokio::time::sleep(Duration::from_secs(wait)).await;
            bail!("MangaDex rate limited; retry after {}s", wait);
        }

        if !response.status().is_success() {
            let status = response.status();
            let text = response
                .text()
                .await
                .unwrap_or_else(|_| String::from("<unreadable>"));
            bail!("MangaDex API error: {} - {}", status, text);
        }

        Ok(response.json().await?)
    }
}

impl Default for MangaDexClient {
    fn default() -> Self {
        Self::new().expect("failed to build HTTP client")
    }
}

impl MangaProvider for MangaDexClient {
    async fn search_mangas(
        &self,
        query: &str,
        _translation: Translation,
    ) -> Result<Vec<MangaInfo>> {
        let url = format!("{}/manga", MANGADEX_API_URL);
        let response = self
            .client
            .get(&url)
            .query(&[("title", query), ("limit", "25")])
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response
                .text()
                .await
                .unwrap_or_else(|_| String::from("<unreadable>"));
            bail!("MangaDex API error: {} - {}", status, text);
        }

        let result: MangaListResponse = response.json().await?;

        Ok(result
            .data
            .into_iter()
            .map(|manga| {
                let title = manga
                    .attributes
                    .title
                    .en
                    .or(manga.attributes.title.ja)
                    .or_else(|| manga.attributes.title.other.values().next().cloned())
                    .unwrap_or_else(|| "Unknown Title".to_string());

                MangaInfo {
                    id: manga.id,
                    title,
                    available_chapters: ChapterCounts::default(),
                }
            })
            .collect())
    }

    async fn fetch_chapters(
        &self,
        manga_id: &str,
        translation: Translation,
    ) -> Result<Vec<Chapter>> {
        let languages = match translation {
            Translation::Sub => vec!["en"],
            Translation::Raw => vec!["ja"],
            Translation::Dub => bail!("Dub translation is not supported for manga"),
        };

        let mut chapters: Vec<(String, String)> = Vec::new();
        let mut offset = 0;

        loop {
            let feed = self
                .fetch_manga_feed(manga_id, CHAPTER_PAGE_LIMIT, offset, &languages)
                .await?;
            let count = feed.data.len();

            for chapter in feed.data {
                if let Some(ch_num) = chapter.attributes.chapter {
                    chapters.push((ch_num, chapter.id));
                }
            }

            if count < CHAPTER_PAGE_LIMIT {
                break;
            }
            offset += CHAPTER_PAGE_LIMIT;
        }

        chapters.sort_by(|a, b| {
            let a_num = a.0.parse::<f64>().unwrap_or(0.0);
            let b_num = b.0.parse::<f64>().unwrap_or(0.0);
            a_num
                .partial_cmp(&b_num)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        chapters.dedup_by(|a, b| a.0 == b.0);

        Ok(chapters
            .into_iter()
            .map(|(label, id)| Chapter { id, label })
            .collect())
    }

    async fn fetch_pages(
        &self,
        _manga_id: &str,
        _translation: Translation,
        chapter_id: &str,
    ) -> Result<Vec<Page>> {
        let url = format!("{}/at-home/server/{}", MANGADEX_API_URL, chapter_id);
        let response = self.client.get(&url).send().await?;
        if !response.status().is_success() {
            let status = response.status();
            let text = response
                .text()
                .await
                .unwrap_or_else(|_| String::from("<unreadable>"));
            bail!("Failed to get at-home server: {} - {}", status, text);
        }

        let at_home: AtHomeResponse = response.json().await?;

        let base_url = at_home.base_url;
        let hash = at_home.chapter.hash;

        Ok(at_home
            .chapter
            .data
            .into_iter()
            .map(|filename| {
                let url = format!("{}/data/{}/{}", base_url, hash, filename);
                Page {
                    url,
                    headers: HashMap::new(),
                }
            })
            .collect())
    }
}

#[derive(Deserialize)]
struct MangaListResponse {
    data: Vec<MangaData>,
}

#[derive(Deserialize)]
struct MangaData {
    id: String,
    attributes: MangaAttributes,
}

#[derive(Deserialize)]
struct MangaAttributes {
    title: TitleMap,
}

#[derive(Deserialize)]
struct TitleMap {
    en: Option<String>,
    ja: Option<String>,
    #[serde(flatten)]
    other: HashMap<String, String>,
}

#[derive(Deserialize)]
struct MangaFeedResponse {
    data: Vec<ChapterData>,
}

#[derive(Deserialize)]
struct ChapterData {
    id: String,
    attributes: ChapterAttributes,
}

#[derive(Deserialize)]
struct ChapterAttributes {
    chapter: Option<String>,
}

#[derive(Deserialize)]
struct AtHomeResponse {
    #[serde(rename = "baseUrl")]
    base_url: String,
    chapter: AtHomeChapter,
}

#[derive(Deserialize)]
struct AtHomeChapter {
    hash: String,
    data: Vec<String>,
}
