//! Playback: the output device, a level meter on the signal path, and the deck that drives both.

use std::fs::File;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use rodio::source::SeekError;
use rodio::{ChannelCount, Decoder, DeviceSinkBuilder, MixerDeviceSink, Player, Sample, SampleRate, Source};

use crate::{Playlist, Repeat};

/// Loudness of the last few milliseconds per channel, written by the audio thread.
/// Values are linear amplitudes from 0 to 1, stored as f32 bits.
#[derive(Default)]
pub struct Levels {
    rms: [AtomicU32; 2],
    peak: [AtomicU32; 2],
}

impl Levels {
    /// `(rms, peak)` for left and right.
    pub fn get(&self) -> [(f32, f32); 2] {
        let load = |a: &AtomicU32| f32::from_bits(a.load(Ordering::Relaxed));
        [0, 1].map(|ch| (load(&self.rms[ch]), load(&self.peak[ch])))
    }

    fn set(&self, ch: usize, rms: f32, peak: f32) {
        self.rms[ch].store(rms.to_bits(), Ordering::Relaxed);
        self.peak[ch].store(peak.to_bits(), Ordering::Relaxed);
    }

    pub fn reset(&self) {
        for ch in 0..2 {
            self.set(ch, 0.0, 0.0);
        }
    }
}

/// Passes samples through untouched and publishes their level every ~23 ms.
pub struct Metered<S> {
    inner: S,
    levels: Arc<Levels>,
    channel: u16,
    frames: usize,
    window: usize,
    sum: [f32; 2],
    peak: [f32; 2],
}

impl<S: Source> Metered<S> {
    pub fn new(inner: S, levels: Arc<Levels>) -> Self {
        let window = (inner.sample_rate().get() as usize / 43).max(1);
        Self { inner, levels, channel: 0, frames: 0, window, sum: [0.0; 2], peak: [0.0; 2] }
    }

    fn publish(&mut self) {
        let mono = self.inner.channels().get() == 1;
        for ch in 0..2 {
            let src = if mono { 0 } else { ch };
            let rms = (self.sum[src] / self.frames as f32).sqrt();
            self.levels.set(ch, rms, self.peak[src]);
        }
        self.frames = 0;
        self.sum = [0.0; 2];
        self.peak = [0.0; 2];
    }
}

impl<S: Source> Iterator for Metered<S> {
    type Item = Sample;

    fn next(&mut self) -> Option<Sample> {
        let s = self.inner.next()?;

        // Channels past the second fold into the right meter.
        let ch = usize::from(self.channel.min(1));
        let v = s.abs();
        self.sum[ch] += v * v;
        self.peak[ch] = self.peak[ch].max(v);

        self.channel += 1;
        if self.channel >= self.inner.channels().get() {
            self.channel = 0;
            self.frames += 1;
            if self.frames >= self.window {
                self.publish();
            }
        }
        Some(s)
    }
}

impl<S: Source> Source for Metered<S> {
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }

    fn channels(&self) -> ChannelCount {
        self.inner.channels()
    }

    fn sample_rate(&self) -> SampleRate {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        self.channel = 0;
        self.inner.try_seek(pos)
    }
}

/// Opens the default output. The returned sink must stay alive for sound to play.
///
/// The fallback matters: AirPods accept buffers of at most 960 frames, and rodio's default of
/// 2048 fails on them, so the device's own supported configs are tried in turn.
pub fn open_output() -> Result<(MixerDeviceSink, Player), String> {
    let builder = DeviceSinkBuilder::from_default_device().map_err(|e| e.to_string())?;
    let mut sink = builder.open_sink_or_fallback().map_err(|e| e.to_string())?;
    sink.log_on_drop(false);
    let player = Player::connect_new(sink.mixer());
    Ok((sink, player))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    Stopped,
    Playing,
    Paused,
}

/// The playlist plus the player. The UI and a watcher thread share it behind a mutex.
pub struct Deck {
    pub playlist: Playlist,
    pub transport: Transport,
    pub levels: Arc<Levels>,

    /// Length of the current track, from its tags or the decoder.
    pub duration: Option<Duration>,

    /// Volume from 0 to 1, as set on the knob.
    pub volume: f32,

    /// Why the last track couldn't play, until the next one starts.
    pub error: Option<String>,

    /// `None` when there is no output device.
    player: Option<Player>,
}

impl Deck {
    pub fn new(player: Option<Player>) -> Self {
        Self {
            playlist: Playlist::default(),
            transport: Transport::Stopped,
            levels: Arc::default(),
            duration: None,
            volume: 0.8,
            error: None,
            player,
        }
    }

    pub fn has_output(&self) -> bool {
        self.player.is_some()
    }

    /// Plays track `i` from the start. A file that won't decode is marked and skipped.
    pub fn play(&mut self, i: usize) {
        let Some(player) = &self.player else { return };
        let mut next = Some(i);

        // Each attempt either plays or marks one more track as failed, so this ends.
        while let Some(k) = next {
            let Some(track) = self.playlist.tracks.get_mut(k) else { break };
            let decoded = File::open(&track.path).map_err(|e| e.to_string()).and_then(|f| Decoder::try_from(f).map_err(|e| e.to_string()));
            self.playlist.current = Some(k);
            match decoded {
                Ok(dec) => {
                    if track.duration.is_none() {
                        track.duration = dec.total_duration();
                    }
                    self.duration = track.duration;
                    player.clear();
                    self.levels.reset();
                    player.set_volume(gain(self.volume));
                    player.append(Metered::new(dec, self.levels.clone()));
                    player.play();
                    self.transport = Transport::Playing;
                    self.error = None;
                    return;
                }
                Err(e) => {
                    track.failed = true;
                    self.error = Some(e);
                    next = self.playlist.next_index(false).filter(|&n| !self.playlist.tracks[n].failed);
                }
            }
        }
        self.stop();
    }

    /// Space: pause, resume, or start from the current or first track.
    pub fn toggle(&mut self) {
        match self.transport {
            Transport::Playing => self.pause(),
            Transport::Paused => {
                if let Some(p) = &self.player {
                    p.play();
                }
                self.transport = Transport::Playing;
            }
            Transport::Stopped => {
                if let Some(i) = self.playlist.current.or_else(|| self.playlist.next_index(false)) {
                    self.play(i);
                }
            }
        }
    }

    pub fn pause(&mut self) {
        if self.transport == Transport::Playing {
            if let Some(p) = &self.player {
                p.pause();
            }
            self.transport = Transport::Paused;
        }
    }

    /// Stops and rewinds. The current track stays selected.
    pub fn stop(&mut self) {
        if let Some(p) = &self.player {
            p.clear();
        }
        self.levels.reset();
        self.transport = Transport::Stopped;
    }

    pub fn next(&mut self) {
        if let Some(i) = self.playlist.next_index(false) {
            self.start_or_cue(i);
        }
    }

    /// Past three seconds, previous restarts the current track first, as on a CD player.
    pub fn prev(&mut self) {
        if self.position() > Duration::from_secs(3) {
            self.seek(Duration::ZERO);
        } else if let Some(i) = self.playlist.prev_index() {
            self.start_or_cue(i);
        } else {
            self.seek(Duration::ZERO);
        }
    }

    /// Plays `i` when the deck is running; otherwise just makes it current.
    fn start_or_cue(&mut self, i: usize) {
        if self.transport == Transport::Stopped {
            self.playlist.current = Some(i);
            self.duration = self.playlist.tracks[i].duration;
        } else {
            let paused = self.transport == Transport::Paused;
            self.play(i);
            if paused {
                self.pause();
            }
        }
    }

    pub fn seek(&mut self, pos: Duration) {
        if self.transport == Transport::Stopped {
            return;
        }
        if let Some(p) = &self.player {
            let _ = p.try_seek(pos);
        }
    }

    pub fn position(&self) -> Duration {
        match (&self.player, self.transport) {
            (Some(p), Transport::Playing | Transport::Paused) => p.get_pos(),
            _ => Duration::ZERO,
        }
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
        if let Some(p) = &self.player {
            p.set_volume(gain(self.volume));
        }
    }

    pub fn remove(&mut self, i: usize) {
        if self.playlist.current == Some(i) {
            self.stop();
            self.duration = None;
        }
        self.playlist.remove(i);
    }

    pub fn clear(&mut self) {
        self.stop();
        self.playlist.clear();
        self.duration = None;
        self.error = None;
    }

    pub fn set_repeat(&mut self, repeat: Repeat) {
        self.playlist.repeat = repeat;
    }

    /// Called every few milliseconds off the UI thread. Starts the next track when one runs out.
    /// Returns true when something changed that the UI should show.
    pub fn tick(&mut self) -> bool {
        let finished = self.transport == Transport::Playing && self.player.as_ref().is_some_and(Player::empty);
        if !finished {
            return false;
        }
        match self.playlist.next_index(true) {
            Some(i) => self.play(i),
            // The end of the list: stop and cue the first track, as a CD player does.
            None => {
                self.stop();
                self.playlist.current = None;
                self.playlist.current = self.playlist.next_index(false);
                if let Some(c) = self.playlist.current {
                    self.duration = self.playlist.tracks[c].duration;
                }
            }
        }
        true
    }
}

/// Knob position to amplitude. Squaring makes the knob feel even to the ear.
fn gain(volume: f32) -> f32 {
    volume * volume
}

#[cfg(test)]
mod tests {
    use super::*;
    use rodio::source::SineWave;

    #[test]
    fn space_toggles_pause_and_resume() {
        // A player with no device behind it still tracks pause state.
        let (player, _output) = Player::new();
        let mut deck = Deck::new(Some(player));
        deck.transport = Transport::Playing;
        deck.toggle();
        assert_eq!(deck.transport, Transport::Paused);
        assert!(deck.player.as_ref().is_some_and(Player::is_paused));
        deck.toggle();
        assert_eq!(deck.transport, Transport::Playing);
        assert!(!deck.player.as_ref().is_some_and(Player::is_paused));
    }

    #[test]
    fn meter_reads_a_sine() {
        let levels = Arc::new(Levels::default());
        let sine = SineWave::new(440.0).take_duration(Duration::from_millis(200)).amplify(0.5);
        let n = Metered::new(sine, levels.clone()).count();
        assert!(n > 0);
        let [(rms, peak), (rms_r, _)] = levels.get();

        // A sine's RMS is its peak over √2.
        assert!((peak - 0.5).abs() < 0.01, "peak {peak}");
        assert!((rms - 0.5 / 2f32.sqrt()).abs() < 0.02, "rms {rms}");
        assert_eq!(rms, rms_r, "mono feeds both meters");
    }
}
