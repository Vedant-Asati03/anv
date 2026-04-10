use anyhow::{Context, Result, anyhow};
use config::{Config, Environment, File, FileFormat};
use dirs_next::config_dir;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

const FALLBACK_CONFIG_PATH: &str = "~/.config/anv/config.toml";

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AppConfig {
    #[serde(default = "default_player")]
    pub player: String,

    #[serde(default)]
    pub binge: bool,

    #[serde(default)]
    pub sync: SyncConfig,

    #[serde(skip, default = "config_path")]
    pub path: PathBuf,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct SyncConfig {
    #[serde(default)]
    pub enabled: bool,

    /// MAL API client ID from https://myanimelist.net/apiconfig
    #[serde(default)]
    pub client_id: String,
}

fn default_player() -> String {
    "mpv".to_string()
}

const CONFIG_HEADER: &str = "# anv configuration
# Docs: https://github.com/Vedant-Asati03/anv
#
# player — media player command (default: \"mpv\")
#           also overridable with ANV_PLAYER env var
#
# binge   — set to true to auto-play the next episode without prompting
#           (can also be enabled per-session with the --binge flag)
#
# [sync]
#   enabled — set to true to sync watch status to MAL after each episode
#   client_id — your MAL API client ID
#               register at https://myanimelist.net/apiconfig
#               redirect URI must be: http://localhost:11422/callback
";

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            player: default_player(),
            binge: false,
            sync: SyncConfig::default(),
            path: config_path(),
        }
    }
}

impl AppConfig {
    pub fn load(&self) -> Result<Self> {
        if !self.path.exists() {
            Self::write_defaults(self)?
        }

        let config = Config::builder()
            .add_source(File::new(
                self.path
                    .to_str()
                    .ok_or_else(|| anyhow!("Config path is not valid UTF-8"))?,
                FileFormat::Toml,
            ))
            .add_source(
                Environment::with_prefix("ANV")
                    .separator("__")
                    .try_parsing(true),
            )
            .build()
            .context("failed to build config")?;

        config
            .try_deserialize::<AppConfig>()
            .context("failed to deserialize config")
    }

    fn write_defaults(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create config dir {}", parent.display()))?;
        }
        let default_cfg = AppConfig::default();
        let toml_str =
            toml::to_string_pretty(&default_cfg).context("failed to serialize default config")?;
        fs::write(&self.path, format!("{CONFIG_HEADER}{toml_str}")).with_context(|| {
            format!("failed to write default config to {}", self.path.display())
        })?;
        println!("Created default config at {}", self.path.display());
        Ok(())
    }

    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create config dir {}", parent.display()))?;
        }
        let toml_str = toml::to_string_pretty(self).context("failed to serialize config")?;
        fs::write(&self.path, format!("{CONFIG_HEADER}{toml_str}"))
            .with_context(|| format!("failed to write config to {}", self.path.display()))?;
        Ok(())
    }
}

pub fn config_path() -> PathBuf {
    let base = config_dir().unwrap_or_else(|| PathBuf::from(FALLBACK_CONFIG_PATH));
    base.join("anv").join("config.toml")
}
