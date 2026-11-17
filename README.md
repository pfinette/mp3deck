# MP3deck

A small MP3 player for macOS that looks like a tape deck. Drop MP3s or folders on the window, press Space.

Personal use only. Not distributed or notarized.

## Use

| Input | Action |
|---|---|
| Drop files or folders | Add to the playlist. Plays at once when the deck was stopped |
| ⌘O or LOAD | Choose files |
| Space | Play or pause. When stopped, plays the selected track |
| ⌘← / ⌘→ | Previous / next. Previous restarts the track first after 3 s |
| ↑ / ↓, ⏎ | Select a track, play it |
| ⌫ | Remove the selected track |
| Double-click a track | Play it |
| Right-click a track | Play, Show in Finder, Remove from Playlist |
| Click or drag the seek bar | Jump |

- REPEAT is OFF, ONE or ALL. SHUFFLE plays the rest of the list at random, starting from the current track.
- The playlist, volume, repeat and shuffle persist between launches.
- Playback continues when the window is covered or minimised.

## Build

```sh
./bundle.sh            # target/MP3deck.app
./bundle.sh --install  # also copies it to /Applications
```
