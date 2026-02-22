![logo.png](./assets/logo.png)

anv is a terminal-native anime launcher for people who think tmux panes and watchlists belong together. Point it at a title, pick your episode, and drop straight into `mpv` without touching a browser tab.

## Why terminal otaku dig it
- Curated for AllAnime streams – fast GraphQL search with zero spoiler thumbnails.
- Sub or dub on demand via `--dub`; switches the query and history tagging automatically.
- Episode selector behaves like a shell picker: arrow keys, `Enter`, `Esc` to bail.
- Remembers what you watched last night, including translation choice – `anv --history` drops you right back in.
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
anv --history
```

Jump directly to an episode:
```bash
anv -e 12 "bocchi the rock"
```

Set a custom player (e.g. tuned mpv build):
```bash
# via environment variable
export ANV_PLAYER="/usr/bin/mpv --ytdl-format=best"
anv "naruto"

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
[mal]
client_id = "<your-client-id>"

[sync]
enabled = true
```

**3. Authenticate**

```bash
anv sync enable mal
```

This opens your browser to the MAL authorisation page. After you approve, the token is saved to your data directory and you're done.

### Sync commands

| Command | What it does |
|---|---|
| `anv sync enable mal` | Authenticate with MAL (runs OAuth flow if no token stored) |
| `anv sync status` | Show whether sync is enabled, token validity, and expiry |
| `anv sync disable` | Disable sync (`sync.enabled = false` in config) |

### How sync works

After each episode finishes playing:

1. **First time seeing a show** — anv searches MAL for the title and shows you the English and Japanese names to confirm it found the right one. The match is cached locally so it never asks again for that series.
2. **Progress update** — if the status on MAL is already `watching` and only the episode count changes, anv updates silently with no prompt. If the status is changing (e.g. adding to list for the first time, or reaching the final episode), anv asks for confirmation first.
3. **Dates are set automatically:**
   - `start_date` is sent when you first start watching (not on list, or previously `plan_to_watch`).
   - `finish_date` is sent when anv marks the show as `completed`.

## How the flow feels

1. CLI asks AllAnime for matching series and shows you a clean list.
2. Pick a show; anv fetches available episode numbers for the chosen translation.
3. Episode picker highlights your last watched entry so Enter instantly resumes; Esc backs out like a prompt should.
4. Streams are resolved through AllAnime's clock API and piped to `mpv` with the right headers and subtitles.
5. History gets updated in `~/.local/share/anv/history.json` (Linux; platform-specific on others) so the next session remembers everything.
6. If MAL sync is enabled, watch progress is synced silently or with a brief confirmation depending on what changed.

## Tips and tweaks
- Keep `mpv` upgraded – some providers only serve DASH/HLS variants that older builds struggle with.
- If you want to experiment with custom players, set `player` in `~/.config/anv/config.toml` or use the `ANV_PLAYER` environment variable (env overrides config).
- Use `--cache-dir <DIR>` if you want manga page cache files somewhere specific (faster disk, larger partition, etc.).
- Use `-e <EP>` to skip the interactive episode selector and start playing a specific episode immediately.
- Use `cargo install anv --force` to update when new AllAnime quirks pop up.
- Run `anv sync status` to quickly check if your MAL token is still valid before a long watch session.

## Troubleshooting
- `mpv` not found: install it or set `player` in your config (or `ANV_PLAYER` env var).
- Streams empty: AllAnime occasionally throttles or shuffles providers; try again later or update anv.
- History file corrupted: delete the JSON under your data dir and anv recreates it on launch.
- MAL sync not working: run `anv sync status` to check token state, then `anv sync enable mal` to re-authenticate if needed.

## License

Released under the [MIT License](LICENSE). Have fun, stay hydrated, and don't skip the ending songs.
