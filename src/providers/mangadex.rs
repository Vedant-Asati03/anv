use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use reqwest::Client;
use serde::Deserialize;

use super::MangaProvider;
use crate::types::{ChapterCounts, MangaInfo, Page, Translation};

const MANGADEX_API_URL: &str = "https://api.mangadex.org";

pub struct MangaDexClient {
    client: Client,
}

impl MangaDexClient {
    pub fn new() -> Result<Self> {
        let client = Client::builder().user_agent("anv/0.2.0").build()?;
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

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            bail!("MangaDex API error: {} - {}", status, text);
        }

        Ok(response.json().await?)
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
            let text = response.text().await.unwrap_or_default();
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

                // MangaDex doesn't give chapter counts in search results easily without extra calls.
                // We'll just return 0 or a placeholder for now, or we could fetch statistics.
                // For simplicity, let's leave it as 0 or maybe try to fetch stats if needed,
                // but `available_chapters` in `MangaInfo` is `ChapterCounts`.
                // Let's just set it to 0 for now as it's expensive to fetch for all search results.
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
    ) -> Result<Vec<String>> {
        let languages = match translation {
            Translation::Sub => vec!["en"],
            Translation::Raw => vec!["ja"],
            Translation::Dub => vec!["en"], // Manga doesn't have dub, treat as sub/en
        };

        let mut chapters = Vec::new();
        let mut offset = 0;
        let limit = 500;

        loop {
            let feed = self
                .fetch_manga_feed(manga_id, limit, offset, &languages)
                .await?;
            let count = feed.data.len();

            for chapter in feed.data {
                if let Some(ch_num) = chapter.attributes.chapter {
                    chapters.push((ch_num, chapter.id));
                }
            }

            if count < limit {
                break;
            }
            offset += limit;
        }

        // We need to return Vec<String> which are chapter numbers.
        // However, `fetch_pages` takes a `chapter` string.
        // In AllAnime, the chapter string IS the identifier used to fetch pages.
        // In MangaDex, we have a chapter number (e.g. "1") and a chapter ID (UUID).
        // The `fetch_pages` method in the trait takes `chapter: &str`.
        // If we return just the number "1", `fetch_pages` will receive "1".
        // But MangaDex needs the UUID to fetch pages.
        //
        // Problem: The `MangaProvider` trait assumes the chapter string is sufficient to fetch pages.
        // For AllAnime, it seems the chapter string is enough (or it uses it to query).
        //
        // If I return the UUIDs as "chapters", the UI will show UUIDs to the user, which is bad.
        // The UI displays the strings returned by `fetch_chapters`.
        //
        // I might need to encode the ID in the string or change the trait to return a struct `Chapter { id: String, label: String }`.
        //
        // Let's look at `src/main.rs` usage of `fetch_chapters`.
        // It calls `client.fetch_chapters`, gets `Vec<String>`, displays them in a list.
        // Then user selects one, and it calls `client.fetch_pages(..., &chosen)`.
        //
        // If I change the trait, I break AllAnime.
        //
        // Hack: I can format the string as "ChapterNum|UUID" and parse it in `fetch_pages`.
        // But the UI will show "1|uuid...".
        //
        // Better approach: Refactor `MangaProvider` (and `AnimeProvider`?) to return objects for episodes/chapters.
        //
        // Let's check `src/types.rs` again.
        // `EpisodeCounts` and `ChapterCounts` are just counts.
        //
        // If I refactor the trait, I need to update `main.rs` and `allanime.rs`.
        // This seems like the right way to go to support different providers properly.
        //
        // Let's modify `src/types.rs` to include `Chapter` and `Episode` structs?
        // Or just change the return type of `fetch_chapters` to `Vec<Chapter>` where `Chapter` has `id` and `number`.
        //
        // Wait, `main.rs` uses `Select` on the strings.
        //
        // Let's try to keep it simple first.
        // Can I fetch the chapter by number in MangaDex?
        // Yes, I can query the feed filtering by chapter number.
        // So `fetch_pages` can take the chapter number, search for the chapter ID, then fetch pages.
        // This adds an extra API call but keeps the trait signature.
        //
        // `fetch_chapters` returns list of numbers ["1", "2", ...].
        // `fetch_pages` receives "1".
        // `fetch_pages` calls `GET /manga/{id}/feed?chapter=1&translatedLanguage[]=en`.
        // Gets the ID.
        // Calls `GET /at-home/server/{id}`.
        //
        // This works!

        // Sort chapters numerically and deduplicate
        chapters.sort_by(|a, b| {
            let a_num = a.0.parse::<f32>().unwrap_or(0.0);
            let b_num = b.0.parse::<f32>().unwrap_or(0.0);
            a_num
                .partial_cmp(&b_num)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let chapter_nums: Vec<String> = chapters.into_iter().map(|(num, _)| num).collect();
        // Dedup
        let mut unique_chapters = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for ch in chapter_nums {
            if seen.insert(ch.clone()) {
                unique_chapters.push(ch);
            }
        }

        Ok(unique_chapters)
    }

    async fn fetch_pages(
        &self,
        manga_id: &str,
        translation: Translation,
        chapter: &str,
    ) -> Result<Vec<Page>> {
        // 1. Find the chapter ID for this chapter number
        let languages = match translation {
            Translation::Sub => vec!["en"],
            Translation::Raw => vec!["ja"],
            Translation::Dub => vec!["en"],
        };

        let query = vec![("manga", manga_id), ("chapter", chapter), ("limit", "1")];
        // We need to filter by language too
        let url = format!("{}/chapter", MANGADEX_API_URL);
        let mut req = self.client.get(&url).query(&query);
        for lang in &languages {
            req = req.query(&[("translatedLanguage[]", lang)]);
        }

        let response = req.send().await?;
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            bail!("Failed to find chapter ID: {} - {}", status, text);
        }

        let feed: ChapterListResponse = response.json().await?;
        let chapter_id = feed.data.first().context("Chapter not found")?.id.clone();

        // 2. Get At-Home server URL
        let url = format!("{}/at-home/server/{}", MANGADEX_API_URL, chapter_id);
        let response = self.client.get(&url).send().await?;
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            bail!("Failed to get at-home server: {} - {}", status, text);
        }

        let at_home: AtHomeResponse = response.json().await?;

        // 3. Construct page URLs
        let base_url = at_home.base_url;
        let hash = at_home.chapter.hash;
        let filenames = at_home.chapter.data; // High quality

        let pages = filenames
            .into_iter()
            .map(|filename| {
                let url = format!("{}/data/{}/{}", base_url, hash, filename);
                Page {
                    url,
                    headers: HashMap::new(), // MangaDex images usually don't require special headers
                }
            })
            .collect();

        Ok(pages)
    }
}

// --- Structs ---

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
struct ChapterListResponse {
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
