# MP3deck

A small, minimal MP3 player for macOS, styled after the fm1-sampler deck. Drop MP3s, MP4s, MKVs or folders on the window, press Space.

MP4 and MKV files play their audio only. MP3 and AAC decode in the app; anything else, such as Opus in MKV, needs `ffmpeg` (`brew install ffmpeg`).

Personal use only. Not distributed or notarized.

## Use

| Input | Action |
|---|---|
| Drop files or folders | Add to the playlist. Plays at once when the deck was stopped |
| ⌘O or the folder icon | Choose files |
| Space | Play or pause. When stopped, plays the selected track |
| ⌘← / ⌘→ | Previous / next. Previous restarts the track first after 3 s |
| ↑ / ↓, ⏎ | Select a track, play it |
| ⌫ | Remove the selected track |
| Double-click a track | Play it |
| Right-click a track | Play, Show in Finder, Remove from Playlist, Clear Playlist |
| Right-click the track list | Load Tracks, Clear Playlist |
| Click or drag the waveform | Jump |

- The waveform shows the whole track: ink for what has played, grey for what's to come, the orange line for now. Click or drag it to jump. Long files fill it in as they are read.
- Two VU dials show the left and right level, read before the volume knob.
- VOLUME is a knob; drag up or down. REPEAT and SHUFFLE are knobs too: click for the next setting, or drag.
- The window follows the system's light or dark mode.
- REPEAT is OFF, ONE or ALL. SHUFFLE plays the rest of the list at random, starting from the current track.
- The playlist, volume, repeat and shuffle persist between launches.
- Playback continues when the window is covered or minimised.

## Build

```sh
./bundle.sh            # target/MP3deck.app
./bundle.sh --install  # also copies it to /Applications
```
