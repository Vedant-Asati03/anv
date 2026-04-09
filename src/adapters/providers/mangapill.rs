use std::collections::HashMap;

use anyhow::{Result, bail};
use reqwest::Client;
use scraper::{Html, Selector};

use super::{MangaProvider, USER_AGENT};
use crate::types::{Chapter, ChapterCounts, MangaInfo, Page, Translation};

const MANGAPILL_BASE_URL: &str = "https://mangapill.com";

pub struct MangapillClient {
    client: Client,
}

impl MangapillClient {
    pub fn new() -> Result<Self> {
        let client = Client::builder().user_agent(USER_AGENT).build()?;
        Ok(Self { client })
    }
}

impl Default for MangapillClient {
    fn default() -> Self {
        Self::new().expect("failed to build HTTP client")
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
        let doc = Html::parse_document(&text);

        let link_sel = Selector::parse(r#"a[href^="/manga/"]"#).expect("valid CSS selector");
        let title_sel = Selector::parse("div.font-black, div.mt-3").expect("valid CSS selector");

        let mut mangas = Vec::new();
        for link in doc.select(&link_sel) {
            let href = link.value().attr("href").unwrap_or_default();
            let trimmed = href.trim_start_matches("/manga/");
            if trimmed.is_empty() {
                continue;
            }
            let title = link
                .select(&title_sel)
                .next()
                .map(|el| el.text().collect::<String>())
                .unwrap_or_default();
            let title = title.trim().to_string();
            if title.is_empty() {
                continue;
            }
            mangas.push(MangaInfo {
                id: trimmed.to_string(),
                title,
                available_chapters: ChapterCounts::default(),
            });
        }

        Ok(mangas)
    }

    async fn fetch_chapters(
        &self,
        manga_id: &str,
        _translation: Translation,
    ) -> Result<Vec<Chapter>> {
        // manga_id is like "2085/jujutsu-kaisen"
        let url = format!("{}/manga/{}", MANGAPILL_BASE_URL, manga_id);
        let response = self.client.get(&url).send().await?;

        if !response.status().is_success() {
            bail!("Mangapill error: {}", response.status());
        }

        let text = response.text().await?;
        let doc = Html::parse_document(&text);

        let sel = Selector::parse(r#"a[href^="/chapters/"]"#).expect("valid CSS selector");

        let mut chapters: Vec<Chapter> = Vec::new();
        for el in doc.select(&sel) {
            let href = el.value().attr("href").unwrap_or_default();
            let slug = href.trim_start_matches("/chapters/").to_string();
            let text: String = el.text().collect();
            let label = text.replace("Chapter ", "").trim().to_string();
            if label.is_empty() || slug.is_empty() {
                continue;
            }
            chapters.push(Chapter { id: slug, label });
        }

        chapters.sort_by(|a, b| {
            let a_num = a.label.parse::<f64>().unwrap_or(0.0);
            let b_num = b.label.parse::<f64>().unwrap_or(0.0);
            a_num
                .partial_cmp(&b_num)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        chapters.dedup_by(|a, b| a.label == b.label);

        Ok(chapters)
    }

    async fn fetch_pages(
        &self,
        _manga_id: &str,
        _translation: Translation,
        chapter_id: &str,
    ) -> Result<Vec<Page>> {
        let url = format!("{}/chapters/{}", MANGAPILL_BASE_URL, chapter_id);
        let response = self.client.get(&url).send().await?;

        if !response.status().is_success() {
            bail!("Mangapill chapter error: {}", response.status());
        }

        let text = response.text().await?;
        let doc = Html::parse_document(&text);

        let sel = Selector::parse("img.js-page[data-src]").expect("valid CSS selector");

        let pages = doc
            .select(&sel)
            .filter_map(|el| el.value().attr("data-src"))
            .map(|src| Page {
                url: src.to_string(),
                headers: HashMap::new(),
            })
            .collect();

        Ok(pages)
    }
}
