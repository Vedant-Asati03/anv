pub mod allanime;
pub mod mangadex;
pub mod mangapill;

pub use crate::ports::providers::{AnimeProvider, MangaProvider};

pub const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/121.0 Safari/537.36";
