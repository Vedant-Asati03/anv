![logo.png](./assets/logo.png)

anv is a terminal-native anime launcher for people who think tmux panes and watchlists belong together. Point it at a title, pick your episode, and drop straight into `mpv` without touching a browser tab.

## Why terminal otaku dig it
- Curated for AllAnime streams – fast GraphQL search with zero spoiler thumbnails.
- Sub or dub on demand via `--dub`; switches the query and history tagging automatically.
- Episode selector behaves like a shell picker: arrow keys, `Enter`, `Esc` to bail.
- Remembers what you watched last night, including translation choice – `anv --history` drops you right back in.
- Fires up `mpv` (or whatever you export as `ANV_PLAYER`) with the highest-quality stream it can negotiate.

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

Jump back to last night’s cliffhanger:
```bash
anv --history
```

Set a custom player (e.g. tuned mpv build):
```bash
export ANV_PLAYER="/usr/bin/mpv --ytdl-format=best"
anv "naruto"
```

## How the flow feels

1. CLI asks AllAnime for matching series and shows you a clean list.
2. Pick a show; anv fetches available episode numbers for the chosen translation.
3. Episode picker highlights your last watched entry so Enter instantly resumes; Esc backs out like a prompt should.
4. Streams are resolved through AllAnime’s clock API and piped to `mpv` with the right headers and subtitles.
5. History gets updated in `~/.local/share/anv/history.json` (Linux; platform-specific on others) so the next session remembers everything.

## Tips and tweaks
- Keep `mpv` upgraded – some providers only serve DASH/HLS variants that older builds struggle with.
- If you want to experiment with custom players, `ANV_PLAYER` can be a full command string (add flags, wrappers, etc.).
- Use `cargo install anv --force` to update when new AllAnime quirks pop up.

## Troubleshooting
- `mpv` not found: install it or point `ANV_PLAYER` at your preferred binary.
- Streams empty: AllAnime occasionally throttles or shuffles providers; try again later or update anv.
- History file corrupted: delete the JSON under your data dir and anv recreates it on launch.

## License

Released under the [MIT License](LICENSE). Have fun, stay hydrated, and don’t skip the ending songs.
