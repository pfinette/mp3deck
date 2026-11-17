# MP3deck

Personal ARM-native macOS MP3 player in Rust and egui, styled with the `hardware-ui` skill. Not distributed or notarized.

## Commands

```sh
./bundle.sh              # release build -> target/MP3deck.app (icon, Info.plist, ad-hoc codesign)
./bundle.sh --install    # also copies to /Applications
cargo test               # playlist logic, level meter, pause/resume
cargo clippy --all-targets
uv run --with pillow --with numpy python scripts/icon.py assets/icon.png   # redraw the icon
```

## Layout

| Path | Contents |
|---|---|
| `src/lib.rs` | `Track`, `read_meta` (lofty), `collect_mp3s`, `natural_cmp`, `format_time`, `Repeat`, `Playlist` (order, shuffle, next/prev) |
| `src/audio.rs` | `Levels` + `Metered` (level meter on the signal path), `open_output`, `Deck` (playlist + player + transport) |
| `src/main.rs` | eframe app: palettes and fonts, title strip, display screen, playlist screen, control row, persisted prefs |
| `scripts/icon.py` | Draws `assets/icon.png` |
| `bundle.sh` | Builds the .app; makes the .icns with `sips` + `iconutil` |

## Stack

| Crate | Version | Notes |
|---|---|---|
| eframe / egui | 0.37 | `App::ui`, `egui::Panel::top/bottom`, `persistence` on |
| rodio | 0.23 | Default features off; `playback` + `mp3` (symphonia). `Player` was `Sink` in older versions |
| lofty | 0.25 | Title, artist, duration |
| rfd | 0.18 | Open dialog |

## How it works

- `Deck` sits in an `Arc<Mutex>`. The UI locks it once per frame. A watcher thread calls `Deck::tick` every 40 ms to start the next track, because macOS stops repainting covered egui windows.
- `Metered` wraps each decoder before `Player::append` and publishes per-channel RMS and peak every ~23 ms through atomics. The meters read the music before the volume knob.
- The knob's volume is squared before it reaches the player.
- A file that won't decode is marked `failed` and skipped.

## Gotchas

- `open_output` uses `open_sink_or_fallback`. AirPods accept buffers of at most 960 frames; rodio's default of 2048 fails on them with "Error opening the stream with the OS".
- Space is consumed before any widget runs, so it never also presses a focused button.
- eframe stores state under `~/Library/Application Support/MP3deck/`. A different app name gets its own folder, with spaces turned into hyphens.
- Screenshots: build a scratch copy under another app name, seed its `app.ron` with `volume:0.0` so nothing is audible, and capture with the `hardware-ui` skill's `shoot.sh`. Simulated keystrokes go to whatever app is frontmost, so avoid them.
