use std::collections::HashMap;

use anyhow::{Result, bail};
use regex::Regex;
use reqwest::Client;

use super::MangaProvider;
use crate::types::{ChapterCounts, MangaInfo, Page, Translation};

const MANGAPILL_BASE_URL: &str = "https://mangapill.com";
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/121.0 Safari/537.36";

pub struct MangapillClient {
    client: Client,
}

impl MangapillClient {
    pub fn new() -> Result<Self> {
        let client = Client::builder().user_agent(USER_AGENT).build()?;
        Ok(Self { client })
    }
}

impl MangaProvider for MangapillClient {
    async fn search_mangas(
        &self,
        query: &str,
        _translation: Translation,
    ) -> Result<Vec<MangaInfo>> {
        let url = format!("{}/search", MANGAPILL_BASE_URL);
        let response = self.client.get(&url).query(&[("q", query)]).send().await?;

        if !response.status().is_success() {
            bail!("Mangapill error: {}", response.status());
        }

        let text = response.text().await?;

        // Regex to find manga in search results
        // <a href="/manga/2085/jujutsu-kaisen" class="mb-2">
        //     <div class="mt-3 font-black leading-tight line-clamp-2">Jujutsu Kaisen</div>
        let re = Regex::new(r#"href="/manga/(\d+)/([^"]+)"[^>]*>\s*<div[^>]*>([^<]+)</div>"#)?;

        let mut mangas = Vec::new();
        for cap in re.captures_iter(&text) {
            let id_num = &cap[1];
            let slug = &cap[2];
            let title = &cap[3];

            // We combine id_num and slug for the ID
            let id = format!("{}/{}", id_num, slug);

            mangas.push(MangaInfo {
                id,
                title: title.trim().to_string(),
                available_chapters: ChapterCounts::default(),
            });
        }

        Ok(mangas)
    }

    async fn fetch_chapters(
        &self,
        manga_id: &str,
        _translation: Translation,
    ) -> Result<Vec<String>> {
        // manga_id is like "2085/jujutsu-kaisen"
        let url = format!("{}/manga/{}", MANGAPILL_BASE_URL, manga_id);
        let response = self.client.get(&url).send().await?;

        if !response.status().is_success() {
            bail!("Mangapill error: {}", response.status());
        }

        let text = response.text().await?;

        // Regex for chapters
        // <a href="/chapters/2085-10271500/jujutsu-kaisen-chapter-271.5" ...>Chapter 271.5</a>
        let re = Regex::new(r#"href="/chapters/([^"]+)"[^>]*>([^<]+)</a>"#)?;

        let mut chapters = Vec::new();
        for cap in re.captures_iter(&text) {
            let _slug = &cap[1]; // e.g. 2085-10271500/jujutsu-kaisen-chapter-271.5
            let title = &cap[2]; // e.g. Chapter 271.5

            // We extract the number from the title
            // "Chapter 271.5" -> "271.5"
            let num = title.replace("Chapter ", "").trim().to_string();
            chapters.push(num);
        }

        // Sort numerically
        chapters.sort_by(|a, b| {
            let a_num = a.parse::<f32>().unwrap_or(0.0);
            let b_num = b.parse::<f32>().unwrap_or(0.0);
            a_num
                .partial_cmp(&b_num)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        chapters.dedup();

        Ok(chapters)
    }

    async fn fetch_pages(
        &self,
        manga_id: &str,
        _translation: Translation,
        chapter: &str,
    ) -> Result<Vec<Page>> {
        // We need to find the chapter slug again because we only stored the number.
        // This is inefficient but required by the trait signature.

        let url = format!("{}/manga/{}", MANGAPILL_BASE_URL, manga_id);
        let response = self.client.get(&url).send().await?;
        let text = response.text().await?;

        // Find the chapter link for this number
        // We look for >Chapter {chapter}<
        let pattern = format!(
            r#"href="/chapters/([^"]+)"[^>]*>Chapter {}</a>"#,
            regex::escape(chapter)
        );
        let re = Regex::new(&pattern)?;

        let chapter_slug = if let Some(cap) = re.captures(&text) {
            cap[1].to_string()
        } else {
            // Try fuzzy match or just fail
            bail!("Chapter {} not found", chapter);
        };

        let url = format!("{}/chapters/{}", MANGAPILL_BASE_URL, chapter_slug);
        let response = self.client.get(&url).send().await?;
        let text = response.text().await?;

        // Regex for images
        // <img class="js-page" data-src="([^"]+)"
        let re = Regex::new(r#"data-src="([^"]+)""#)?;

        let mut pages = Vec::new();
        for cap in re.captures_iter(&text) {
            let url = cap[1].to_string();
            pages.push(Page {
                url,
                headers: HashMap::new(),
            });
        }

        Ok(pages)
    }
}
