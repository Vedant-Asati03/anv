use anyhow::{Context, Result, anyhow};
use config::{Config, Environment, File, FileFormat};
use dirs_next::config_dir;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AppConfig {
    #[serde(default = "default_player")]
    pub player: String,

    #[serde(default)]
    pub mal: MalConfig,

    #[serde(default)]
    pub sync: SyncConfig,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct MalConfig {
    /// MAL API client ID from https://myanimelist.net/apiconfig
    #[serde(default)]
    pub client_id: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct SyncConfig {
    #[serde(default)]
    pub enabled: bool,
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
# [mal]
#   client_id — your MAL API client ID
#               register at https://myanimelist.net/apiconfig
#               redirect URI must be: http://localhost:11422/callback
#
# [sync]
#   enabled — set to true to sync watch status to MAL after each episode

";

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            player: default_player(),
            mal: MalConfig::default(),
            sync: SyncConfig::default(),
        }
    }
}

impl AppConfig {
    pub fn config_path() -> Result<PathBuf> {
        let base = config_dir().ok_or_else(|| anyhow!("Could not determine config directory"))?;
        Ok(base.join("anv").join("config.toml"))
    }

    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;

        if !path.exists() {
            Self::write_defaults(&path)?;
        }

        let cfg = Config::builder()
            .add_source(File::new(
                path.to_str()
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

        cfg.try_deserialize::<AppConfig>()
            .context("failed to deserialize config")
    }

    fn write_defaults(path: &PathBuf) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create config dir {}", parent.display()))?;
        }
        let default_cfg = AppConfig::default();
        let toml_str =
            toml::to_string_pretty(&default_cfg).context("failed to serialize default config")?;
        fs::write(path, format!("{CONFIG_HEADER}{toml_str}"))
            .with_context(|| format!("failed to write default config to {}", path.display()))?;
        println!("Created default config at {}", path.display());
        Ok(())
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::config_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create config dir {}", parent.display()))?;
        }
        let toml_str = toml::to_string_pretty(self).context("failed to serialize config")?;
        fs::write(&path, format!("{CONFIG_HEADER}{toml_str}"))
            .with_context(|| format!("failed to write config to {}", path.display()))?;
        Ok(())
    }
}
