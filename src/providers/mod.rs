use crate::types::{MangaInfo, Page, ShowInfo, StreamOption, Translation};
use anyhow::Result;

pub mod allanime;
pub mod mangadex;
pub mod mangapill;

pub trait AnimeProvider {
    async fn search_shows(&self, query: &str, translation: Translation) -> Result<Vec<ShowInfo>>;
    async fn fetch_episodes(&self, show_id: &str, translation: Translation) -> Result<Vec<String>>;
    async fn fetch_streams(
        &self,
        show_id: &str,
        translation: Translation,
        episode: &str,
    ) -> Result<Vec<StreamOption>>;
}

pub trait MangaProvider {
    async fn search_mangas(&self, query: &str, translation: Translation) -> Result<Vec<MangaInfo>>;
    async fn fetch_chapters(&self, manga_id: &str, translation: Translation)
    -> Result<Vec<String>>;
    async fn fetch_pages(
        &self,
        manga_id: &str,
        translation: Translation,
        chapter: &str,
    ) -> Result<Vec<Page>>;
}
