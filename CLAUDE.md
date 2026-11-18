# MP3deck

ARM-native macOS MP3 player, published at github.com/pfinette/mp3deck, in Rust and egui, styled after fm1-sampler's phone deck (https://github.com/ferluht/fm1-sampler): one flat ground, ink, a dim tone, and one orange for what is live. It also plays the audio of MP4 and MKV files.

Distributed as an ad-hoc-signed, unnotarized .app zip on GitHub Releases. Own code is MIT; Symphonia is MPL-2.0. See README for install steps and the license notice.

## Commands

```sh
./bundle.sh              # release build -> target/MP3deck.app (icon, Info.plist, ad-hoc codesign)
./bundle.sh --install    # also copies to /Applications
cargo test               # playlist logic, level meter, pause/resume, Opus-in-MKV decode and seek
cargo clippy --all-targets
uv run --with pillow --with numpy python scripts/icon.py assets/icon.png   # redraw the icon
```

## Layout

| Path | Contents |
|---|---|
| `src/lib.rs` | `Track`, `read_meta` (lofty, then ffprobe), `collect_tracks`, `EXTENSIONS`, `natural_cmp`, `format_time`, `Repeat`, `Playlist` (order, shuffle, next/prev) |
| `src/audio.rs` | `Levels` + `Metered` (level meter on the signal path), `open_output`, `open_track`, `Deck` (playlist + player + transport) |
| `src/ffmpeg.rs` | `FfmpegSource`: an `ffmpeg` process piping 48 kHz stereo f32, decoded ahead on a reader thread; `probe` (ffprobe) |
| `src/main.rs` | eframe app: palettes and fonts, title strip, then two columns (header, VU dials, transport / playlist, knobs) over a full-width waveform, persisted prefs |
| `src/wave.rs` | `analyze`: RMS of every 50 ms, mixed to mono, appended as it is read (native decoder, or ffmpeg at 8 kHz) |
| `scripts/icon.py` | Draws `assets/icon.png` |
| `bundle.sh` | Builds the .app; makes the .icns with `sips` + `iconutil`; copies LICENSE and THIRD_PARTY_LICENSES |
| `about.toml`, `about.hbs` | cargo-about config. `cargo about generate about.hbs -o THIRD_PARTY_LICENSES` after any dependency change |

## Stack

| Crate | Version | Notes |
|---|---|---|
| eframe / egui | 0.37 | `App::ui`, `egui::Panel::top/bottom`, `persistence` on |
| rodio | 0.23 | Default features off; `playback`, `mp3`, `mp4` (symphonia, MPL-2.0). `Player` was `Sink` in older versions |
| lofty | 0.25 | Title, artist, duration |
| rfd | 0.18 | Open dialog |

## How it works

- `Deck` sits in an `Arc<Mutex>`. The UI locks it once per frame. A watcher thread calls `Deck::tick` every 40 ms to start the next track, because macOS stops repainting covered egui windows.
- `Metered` wraps each decoder before `Player::append` and publishes per-channel RMS and peak every ~23 ms through atomics. The meters read the music before the volume knob.
- The knob's volume is squared before it reaches the player.
- `open_track` decodes MP3 and MP4/M4A natively with an extension hint. MKV, WebM and any MP4 symphonia rejects go to `FfmpegSource`. Seeking restarts ffmpeg with `-ss`.
- `FfmpegSource::next` never blocks. It plays silence while ffmpeg starts or falls behind, and ends when the reader thread hangs up.
- Video files keep their file name as the title; their tags are usually noise.
- The current track's waveform is read on a background thread into an `Arc<Wave>`, cached per path. Starting a new job bumps `wave_job`, which stops older ones. An hour of Opus takes about 7 s, so the bars fill in left to right.
- Each bar is the loudest 50 ms window it covers, scaled to the track's loudest window. Unread bars are dots.
- The VU dials show RMS with 0 VU at −10 dBFS on a −20 to +3 VU scale, eased with a 90 ms time constant. They are drawn like the knobs: a grey arc, an orange arc up to the level, an ink needle.
- A file that won't decode is marked `failed` and skipped.

## Gotchas

- `open_output` uses `open_sink_or_fallback`. AirPods accept buffers of at most 960 frames; rodio's default of 2048 fails on them with "Error opening the stream with the OS".
- Space is consumed before any widget runs, so it never also presses a focused button.
- Light and dark palettes follow the system. `LIVE` orange is shared and used only for the playhead, the playing track, knob arcs, the play underline, errors and the held peak.
- Fonts are Helvetica Neue Regular, Bold and Medium from `/System/Library/Fonts/HelveticaNeue.ttc` (indices 0, 1, 10). Its figures are tabular.
- `LAYOUT` in `main.rs` is saved in prefs. Bump it when the default window size changes, so an older saved size is replaced once.
- `ffmpeg` and `ffprobe` are looked up in `/opt/homebrew/bin`, `/usr/local/bin`, then `PATH`, because apps opened from Finder get a bare `PATH`.
- eframe stores state under `~/Library/Application Support/MP3deck/`. A different app name gets its own folder, with spaces turned into hyphens.
- Screenshots: build a scratch copy under another app name, seed its `app.ron` with `volume:0.0` so nothing is audible, and capture with `screencapture -l` on the 880-pt-wide main window (the process also owns a hidden 500×500 window). A covered window stops repainting, so a capture shows its last painted frame; wrapping the scratch build in a .app and launching it with `open` brings it to the front. Simulated keystrokes go to whatever app is frontmost, so avoid them.
