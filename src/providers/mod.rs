use crate::types::{Chapter, MangaInfo, Page, ShowInfo, StreamOption, Translation};
use anyhow::Result;

pub mod allanime;
pub mod mangadex;
pub mod mangapill;

pub const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/121.0 Safari/537.36";

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
    async fn fetch_chapters(
        &self,
        manga_id: &str,
        translation: Translation,
    ) -> Result<Vec<Chapter>>;
    async fn fetch_pages(
        &self,
        manga_id: &str,
        translation: Translation,
        chapter_id: &str,
    ) -> Result<Vec<Page>>;
}
