use crate::types::StreamOption;

use anyhow::{Result, anyhow, bail};
use dialoguer::{Select, theme::ColorfulTheme};
use tokio::process::Command;

pub const PLAYER_ENV_KEY: &str = "ANV_PLAYER";

pub struct DefaultPlayerGateway;

pub fn detect_player() -> String {
    std::env::var(PLAYER_ENV_KEY)
        .ok()
        .filter(|val| !val.trim().is_empty())
        .unwrap_or_else(|| "mpv".to_string())
}

pub(crate) fn build_command(player: &str) -> Result<Command> {
    let parts = shlex::split(player)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| anyhow!("Invalid player command: '{}'", player))?;
    let (bin, args) = parts
        .split_first()
        .ok_or_else(|| anyhow!("Player command is empty"))?;
    let mut cmd = Command::new(bin);
    cmd.args(args);
    Ok(cmd)
}

pub fn choose_stream(mut options: Vec<StreamOption>) -> Result<Option<StreamOption>> {
    if options.len() == 1 {
        return Ok(Some(options.remove(0)));
    }
    let labels: Vec<String> = options.iter().map(StreamOption::label).collect();
    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Select a stream")
        .items(&labels)
        .default(0)
        .interact_opt()?;
    let Some(idx) = selection else {
        return Ok(None);
    };
    Ok(Some(options.remove(idx)))
}

pub async fn launch_player(
    stream: &StreamOption,
    title: &str,
    episode: &str,
    player: &str,
) -> Result<()> {
    let mut cmd = build_command(player)?;
    let media_title = format!("{title} - Episode {episode}");
    cmd.arg("--quiet");
    cmd.arg("--terminal=no");
    cmd.arg(format!("--force-media-title={media_title}"));
    if let Some(sub) = &stream.subtitle {
        cmd.arg(format!("--sub-file={sub}"));
    }
    for (key, value) in &stream.headers {
        if key.eq_ignore_ascii_case("user-agent") {
            cmd.arg(format!("--user-agent={value}"));
        } else if key.eq_ignore_ascii_case("referer") {
            cmd.arg(format!("--referrer={value}"));
            cmd.arg(format!("--http-header-fields=Referer: {value}"));
        } else {
            cmd.arg(format!("--http-header-fields={}: {value}", key));
        }
    }
    cmd.arg(&stream.url);

    let status = match cmd.status().await {
        Ok(status) => status,
        Err(err) => {
            if err.kind() == std::io::ErrorKind::NotFound {
                let bin = shlex::split(player)
                    .and_then(|v| v.into_iter().next())
                    .unwrap_or_else(|| player.to_string());
                return Err(anyhow!(
                    "Player binary '{}' not found. Install it or set {} to a valid command.",
                    bin,
                    PLAYER_ENV_KEY
                ));
            }
            return Err(anyhow!(err).context(format!("failed to launch player '{player}'")));
        }
    };

    if !status.success() {
        bail!("player exited with status {status}");
    }
    Ok(())
}

impl DefaultPlayerGateway {
    pub fn choose_stream(&self, options: Vec<StreamOption>) -> Result<Option<StreamOption>> {
        choose_stream(options)
    }

    pub async fn launch_player(
        &self,
        stream: &StreamOption,
        title: &str,
        episode: &str,
        player: &str,
    ) -> Result<()> {
        launch_player(stream, title, episode, player).await
    }
}
