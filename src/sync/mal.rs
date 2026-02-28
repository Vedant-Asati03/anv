use std::{
    collections::HashMap,
    fs,
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    path::PathBuf,
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use dialoguer::{Confirm, theme::ColorfulTheme};
use dirs_next::data_dir;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::config::AppConfig;

use super::{SyncProvider, SyncUpdate};

const MAL_AUTH_URL: &str = "https://myanimelist.net/v1/oauth2/authorize";
const MAL_TOKEN_URL: &str = "https://myanimelist.net/v1/oauth2/token";
const MAL_API_BASE: &str = "https://api.myanimelist.net/v2";
const OAUTH_PORT: u16 = 11422;
const OAUTH_REDIRECT_URI: &str = "http://localhost:11422/callback";
const CODE_VERIFIER_LEN: usize = 64;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MalToken {
    pub access_token: String,
    pub refresh_token: String,
    /// UTC timestamp when the access token expires.
    pub expires_at: DateTime<Utc>,
}

impl MalToken {
    pub fn is_expired(&self) -> bool {
        // Treat as expired 60 s before actual expiry so we refresh proactively.
        Utc::now() >= self.expires_at - chrono::Duration::seconds(60)
    }

    pub fn token_path() -> Result<PathBuf> {
        let base = data_dir().ok_or_else(|| anyhow!("Could not determine data directory"))?;
        Ok(base.join("anv").join("mal_token.json"))
    }

    pub fn load() -> Result<Option<Self>> {
        let path = Self::token_path()?;
        if !path.exists() {
            return Ok(None);
        }
        let data = fs::read_to_string(&path)
            .with_context(|| format!("failed to read token file {}", path.display()))?;
        let token: MalToken = serde_json::from_str(&data)
            .with_context(|| format!("failed to parse token file {}", path.display()))?;
        Ok(Some(token))
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::token_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create data directory {}", parent.display()))?;
        }
        let data = serde_json::to_string_pretty(self).context("failed to serialize token")?;
        fs::write(&path, data)
            .with_context(|| format!("failed to write token to {}", path.display()))?;
        Ok(())
    }
}

/// Persistent cache that maps AllAnime show IDs to MAL anime IDs.
/// The confirmation dialog is shown only for IDs not yet in this cache.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct MalIdCache {
    entries: HashMap<String, u32>,
}

impl MalIdCache {
    fn cache_path() -> Result<PathBuf> {
        let base = data_dir().ok_or_else(|| anyhow!("Could not determine data directory"))?;
        Ok(base.join("anv").join("mal_id_cache.json"))
    }

    pub fn load() -> Result<Self> {
        let path = Self::cache_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let data = fs::read_to_string(&path)
            .with_context(|| format!("failed to read ID cache {}", path.display()))?;
        serde_json::from_str(&data)
            .with_context(|| format!("failed to parse ID cache {}", path.display()))
    }

    pub fn get(&self, allanime_id: &str) -> Option<u32> {
        self.entries.get(allanime_id).copied()
    }

    pub fn insert_and_save(&mut self, allanime_id: &str, mal_id: u32) -> Result<()> {
        self.entries.insert(allanime_id.to_string(), mal_id);
        let path = Self::cache_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create data directory {}", parent.display()))?;
        }
        let data = serde_json::to_string_pretty(self).context("failed to serialize ID cache")?;
        fs::write(&path, data)
            .with_context(|| format!("failed to write ID cache to {}", path.display()))?;
        Ok(())
    }
}

fn generate_code_verifier() -> String {
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    (0..CODE_VERIFIER_LEN)
        .map(|_| CHARSET[rand::random_range(0..CHARSET.len())] as char)
        .collect()
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: u64,
}

#[derive(Debug, Deserialize)]
struct AnimeSearchResponse {
    data: Vec<AnimeNode>,
}

#[derive(Debug, Deserialize)]
struct AnimeNode {
    node: AnimeDetail,
}

#[derive(Debug, Deserialize)]
struct AnimeDetail {
    id: u32,
    title: String,
    #[serde(default)]
    alternative_titles: AlternativeTitles,
}

#[derive(Debug, Default, Deserialize)]
struct AlternativeTitles {
    #[serde(default)]
    en: String,
    #[serde(default)]
    ja: String,
}

pub struct MalClient {
    client_id: String,
    http: Client,
    pub token: MalToken,
}

impl MalClient {
    /// Build a `MalClient` from an existing (possibly expired) token.
    /// Call `MalClient::authenticate` first if no token exists.
    pub fn from_token(client_id: String, mut token: MalToken) -> Result<Self> {
        let http = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .context("failed to build HTTP client")?;

        if token.is_expired() {
            token = Self::refresh_token_inner(&http, &client_id, &token.refresh_token)?;
            token.save()?;
        }

        Ok(Self {
            client_id,
            http,
            token,
        })
    }

    /// Full OAuth PKCE flow: opens browser, spins up local callback server,
    /// exchanges code for tokens, and saves them to disk.
    pub fn authenticate(client_id: &str) -> Result<MalToken> {
        let verifier = generate_code_verifier();
        let challenge = verifier.clone();

        let auth_url = {
            let mut u = Url::parse(MAL_AUTH_URL).expect("MAL auth URL is valid");
            u.query_pairs_mut()
                .append_pair("response_type", "code")
                .append_pair("client_id", client_id)
                .append_pair("code_challenge", &challenge)
                .append_pair("code_challenge_method", "plain")
                .append_pair("redirect_uri", OAUTH_REDIRECT_URI);
            u.to_string()
        };

        println!("Opening MAL authorization page in your browser...");
        println!("If it doesn't open automatically, visit:\n  {auth_url}");
        let _ = open::that(&auth_url);

        let code =
            Self::wait_for_callback().context("Failed to receive OAuth callback from browser")?;

        println!("Authorization code received. Exchanging for token...");

        let http = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .context("failed to build HTTP client")?;

        let token = Self::exchange_code(&http, client_id, &code, &verifier)?;
        token.save()?;
        Ok(token)
    }

    /// Wait on localhost:11422 for MAL's redirect and return the `code` param.
    fn wait_for_callback() -> Result<String> {
        let listener = TcpListener::bind(("127.0.0.1", OAUTH_PORT)).with_context(|| {
            format!(
                "Failed to bind OAuth listener on port {OAUTH_PORT}. Is another process using it?"
            )
        })?;

        println!(
            "Waiting for authorization callback on http://localhost:{OAUTH_PORT}/callback ..."
        );

        let (mut stream, _) = listener
            .accept()
            .context("Failed to accept OAuth connection")?;
        let mut reader = BufReader::new(stream.try_clone().context("failed to clone stream")?);
        let mut request_line = String::new();
        reader
            .read_line(&mut request_line)
            .context("failed to read OAuth request line")?;

        // Parse "GET /callback?code=...&... HTTP/1.1"
        let path = request_line.split_whitespace().nth(1).unwrap_or_default();

        let dummy_base = "http://localhost";
        let full_url = format!("{dummy_base}{path}");
        let parsed = Url::parse(&full_url).context("failed to parse OAuth callback URL")?;

        let code = parsed
            .query_pairs()
            .find(|(k, _)| k == "code")
            .map(|(_, v)| v.into_owned())
            .ok_or_else(|| {
                anyhow!("No 'code' parameter in OAuth callback. MAL may have returned an error.")
            })?;

        // Send a small success page back to the browser
        let body = b"<html><body><h2>anv: Authorization successful!</h2><p>You can close this tab.</p></body></html>";
        let _ = write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(body);

        Ok(code)
    }

    fn exchange_code(
        http: &Client,
        client_id: &str,
        code: &str,
        verifier: &str,
    ) -> Result<MalToken> {
        let params = [
            ("client_id", client_id),
            ("code", code),
            ("code_verifier", verifier),
            ("grant_type", "authorization_code"),
            ("redirect_uri", OAUTH_REDIRECT_URI),
        ];

        let rt = tokio::runtime::Handle::current();
        let resp: TokenResponse = rt.block_on(async {
            http.post(MAL_TOKEN_URL)
                .form(&params)
                .send()
                .await
                .context("token exchange request failed")?
                .error_for_status()
                .context("MAL returned error on token exchange")?
                .json::<TokenResponse>()
                .await
                .context("failed to parse token response")
        })?;

        Ok(MalToken {
            access_token: resp.access_token,
            refresh_token: resp.refresh_token,
            expires_at: Utc::now() + chrono::Duration::seconds(resp.expires_in as i64),
        })
    }

    /// Refresh the access token using the stored client_id, if expired.
    fn refresh_token_inner(
        http: &Client,
        client_id: &str,
        refresh_token: &str,
    ) -> Result<MalToken> {
        let params = [
            ("client_id", client_id),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ];

        let rt = tokio::runtime::Handle::current();
        let resp: TokenResponse = rt.block_on(async {
            http.post(MAL_TOKEN_URL)
                .form(&params)
                .send()
                .await
                .context("token refresh request failed")?
                .error_for_status()
                .context("MAL returned error on token refresh")?
                .json::<TokenResponse>()
                .await
                .context("failed to parse refresh token response")
        })?;

        Ok(MalToken {
            access_token: resp.access_token,
            refresh_token: resp.refresh_token,
            expires_at: Utc::now() + chrono::Duration::seconds(resp.expires_in as i64),
        })
    }

    /// Search MAL for `title`, show the top result (English + Japanese titles)
    /// and ask the user to confirm it's the right anime.
    /// Returns `Some(mal_id)` if confirmed, `None` if user declines.
    pub async fn resolve_and_confirm_mal_id(&self, title: &str) -> Result<Option<u32>> {
        let resp = self
            .http
            .get(format!("{MAL_API_BASE}/anime"))
            .bearer_auth(&self.token.access_token)
            .query(&[
                ("q", title),
                ("limit", "5"),
                ("fields", "id,title,alternative_titles"),
            ])
            .send()
            .await
            .context("MAL anime search request failed")?
            .error_for_status()
            .context("MAL returned error on anime search")?
            .json::<AnimeSearchResponse>()
            .await
            .context("failed to parse MAL anime search response")?;

        let Some(first) = resp.data.into_iter().next() else {
            println!("  [sync] No MAL results found for \"{title}\". Skipping sync.");
            return Ok(None);
        };

        let detail = first.node;
        let en = if detail.alternative_titles.en.is_empty() {
            &detail.title
        } else {
            &detail.alternative_titles.en
        };
        let ja = &detail.alternative_titles.ja;

        let display = if ja.is_empty() {
            format!("\"{}\"", en)
        } else {
            format!("\"{}\" ({})", en, ja)
        };

        let confirmed = Confirm::with_theme(&ColorfulTheme::default())
            .with_prompt(format!(
                "[sync] Found on MAL: {display}\n  Is this the correct anime?"
            ))
            .default(false)
            .interact()?;

        if confirmed {
            Ok(Some(detail.id))
        } else {
            println!("  [sync] Skipping MAL sync for this show.");
            Ok(None)
        }
    }

    /// Update MAL anime list status.
    async fn update_list_status(&self, mal_id: u32, update: &SyncUpdate) -> Result<()> {
        let mut form: HashMap<&str, String> = HashMap::new();
        form.insert("status", update.status.as_str().to_string());
        form.insert("num_watched_episodes", update.episode.to_string());
        if let Some(ref d) = update.start_date {
            form.insert("start_date", d.clone());
        }
        if let Some(ref d) = update.finish_date {
            form.insert("finish_date", d.clone());
        }
        if let Some(score) = update.score {
            form.insert("score", score.to_string());
        }

        self.http
            .patch(format!("{MAL_API_BASE}/anime/{mal_id}/my_list_status"))
            .bearer_auth(&self.token.access_token)
            .form(&form)
            .send()
            .await
            .context("MAL update list status request failed")?
            .error_for_status()
            .context("MAL returned error on list status update")?;

        Ok(())
    }
}

impl SyncProvider for MalClient {
    fn name(&self) -> &'static str {
        "MyAnimeList"
    }

    async fn update_status(&self, _update: &SyncUpdate) -> Result<()> {
        // MAL requires a resolved MAL ID which isn't embedded in SyncUpdate.
        // Use MalClient::update_status_with_id instead.
        bail!("Use update_status_with_id for MAL sync")
    }
}

impl MalClient {
    /// The real sync entry point used by the anime playback loop.
    /// Caller must supply the resolved MAL ID.
    pub async fn update_status_with_id(&self, mal_id: u32, update: &SyncUpdate) -> Result<()> {
        self.update_list_status(mal_id, update).await
    }

    /// Fetch the anime's list status and planned episode count in one request.
    ///
    /// `anime_info.num_episodes == 0` means MAL doesn't know the total yet
    /// (still airing or not yet announced), so callers must not use it to
    /// infer completion.
    pub async fn get_anime_info(&self, mal_id: u32) -> Result<AnimeInfo> {
        #[derive(Deserialize)]
        struct AnimeWithStatus {
            my_list_status: Option<CurrentListStatus>,
            #[serde(default)]
            num_episodes: u32,
        }

        let resp = self
            .http
            .get(format!("{MAL_API_BASE}/anime/{mal_id}"))
            .bearer_auth(&self.token.access_token)
            .query(&[("fields", "my_list_status,num_episodes")])
            .send()
            .await
            .context("MAL get anime info request failed")?
            .error_for_status()
            .context("MAL returned error fetching anime info")?
            .json::<AnimeWithStatus>()
            .await
            .context("failed to parse MAL anime info response")?;

        Ok(AnimeInfo {
            list_status: resp.my_list_status,
            num_episodes: resp.num_episodes,
        })
    }
}

#[derive(Debug, Deserialize)]
pub struct CurrentListStatus {
    pub status: String,
    pub num_episodes_watched: u32,
}

/// Combined per-anime info returned by [`MalClient::get_anime_info`].
pub struct AnimeInfo {
    /// The user's current list entry, or `None` if not on their list.
    pub list_status: Option<CurrentListStatus>,
    /// MAL's planned total episode count. `0` means unknown / still airing.
    pub num_episodes: u32,
}

/// Build a MalClient only when sync is enabled and a stored token exists.
pub fn build_mal_client_if_enabled(cfg: &AppConfig) -> Option<MalClient> {
    if !cfg.sync.enabled {
        return None;
    }
    if cfg.mal.client_id.is_empty() {
        eprintln!("[sync] mal.client_id is not set in config — sync disabled.");
        return None;
    }
    match MalToken::load() {
        Ok(Some(token)) => match MalClient::from_token(cfg.mal.client_id.clone(), token) {
            Ok(client) => Some(client),
            Err(err) => {
                eprintln!("[sync] Failed to initialize MAL client: {err}");
                None
            }
        },
        Ok(None) => {
            eprintln!(
                "[sync] Sync is enabled but no MAL token found. Run `anv sync enable mal` first."
            );
            None
        }
        Err(err) => {
            eprintln!("[sync] Failed to load MAL token: {err}");
            None
        }
    }
}
