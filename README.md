![logo.png](./assets/logo.png)

anv is a terminal-native anime launcher for people who think tmux panes and watchlists belong together. Point it at a title, pick your episode, and drop straight into `mpv` without touching a browser tab.

## Why terminal otaku dig it
- Curated for AllAnime streams – fast GraphQL search with zero spoiler thumbnails.
- Sub or dub on demand via `--dub`; switches the query and history tagging automatically.
- Episode selector behaves like a shell picker: arrow keys, `Enter`, `Esc` to bail.
- Remembers what you watched last night, including translation choice – `anv history` drops you right back in.
- Reads manga too – `anv --manga` fetches chapters and pipes pages directly to your image viewer (mpv by default).
- Manga page cache supports custom location via `--cache-dir`.
- Jump directly to an episode with `-e` or `--episode` to skip the selection menu.
- Fires up `mpv` (or whatever you set as `player` in config) with the highest-quality stream it can negotiate.
- Syncs watch progress to MyAnimeList – sets start/finish dates, marks completed automatically.

## Install it

### Cargo (recommended)
```bash
cargo install anv
```

### From source
```bash
git clone https://github.com/Vedant-Asati03/anv.git
cd anv
cargo install --path .
```

## Quick start quests

Search and stream:
```bash
anv "bocchi the rock"
```

Prefer the dub:
```bash
anv --dub "demon slayer"
```

Read manga chapters:
```bash
anv --manga "one punch man"
```

Read manga with a custom cache directory:
```bash
anv --manga --cache-dir "/tmp/anv-cache" "one punch man"
```

Jump back to last night's cliffhanger:
```bash
anv history
```

Jump directly to an episode:
```bash
anv -e 12 "bocchi the rock"
```

Set a custom player (e.g. tuned mpv build):
```bash
# via environment variable
export ANV_PLAYER="/usr/bin/mpv --ytdl-format=best"
anv "frieren"

# or permanently in ~/.config/anv/config.toml
# player = "/usr/bin/mpv --ytdl-format=best"
```

## MAL sync

anv can automatically sync your watch progress to [MyAnimeList](https://myanimelist.net).

### Setup

**1. Create a MAL API application**

Go to [myanimelist.net/apiconfig](https://myanimelist.net/apiconfig), create a new app, and set:
- **App type:** `other`
- **Redirect URI:** `http://localhost:11422/callback`

Copy the **Client ID**.

**2. Add it to your config**

The config file lives at `~/.config/anv/config.toml` (Linux/macOS) or `%APPDATA%\anv\config.toml` (Windows).

```toml
[sync]
enabled = true
client_id = "<your-client-id>"
```

**3. Authenticate**

```bash
anv sync enable
```

This opens your browser to the MAL authorisation page. After you approve, the token is saved to your data directory and you're done.

## Troubleshooting
- `mpv` not found: install it or set `player` in your config (or `ANV_PLAYER` env var).
- Streams empty: AllAnime occasionally throttles or shuffles providers; try again later or update anv.
- History file corrupted: delete the JSON under your data dir and anv recreates it on launch.
- MAL sync not working: run `anv sync status` to check token state, then `anv sync enable` to re-authenticate if needed.

## License

Released under the [MIT License](LICENSE). Have fun, stay hydrated, and don't skip the ending songs.
