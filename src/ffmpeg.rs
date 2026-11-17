//! Audio from containers rodio can't read, such as Opus in MKV, decoded by an `ffmpeg` process.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, TryRecvError, sync_channel};
use std::time::Duration;

use rodio::source::SeekError;
use rodio::{ChannelCount, Sample, SampleRate, Source};

const CHANNELS: u16 = 2;
const RATE: u32 = 48_000;

/// 2048 stereo frames, about 43 ms.
const CHUNK_BYTES: usize = 2048 * CHANNELS as usize * 4;

/// Chunks decoded ahead of playback, about two seconds.
const AHEAD: usize = 48;

/// Finds a Homebrew or system copy. Apps opened from Finder don't get the shell's PATH.
pub fn tool(name: &str) -> Option<PathBuf> {
    let dirs = ["/opt/homebrew/bin", "/usr/local/bin"].into_iter().map(PathBuf::from);
    let path = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default();
    dirs.chain(path).map(|d| d.join(name)).find(|p| p.is_file())
}

/// What `ffprobe` reports about a file's first audio stream.
#[derive(Clone, Debug, Default)]
pub struct Probe {
    pub duration: Option<Duration>,
    pub title: Option<String>,
    pub artist: Option<String>,
}

/// Reads the length and tags. Fails when there is no ffprobe or no audio stream.
pub fn probe(path: &Path) -> Result<Probe, String> {
    let ffprobe = tool("ffprobe").ok_or("ffmpeg isn't installed")?;
    let out = Command::new(ffprobe)
        .args(["-v", "error", "-select_streams", "a:0", "-show_entries", "stream=codec_name:format=duration:format_tags=title,artist"])
        .args(["-of", "default=noprint_wrappers=1"])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout);
    let field = |key: &str| {
        text.lines()
            .filter_map(|l| l.split_once('='))
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.trim().to_owned())
            .filter(|v| !v.is_empty() && v != "N/A")
    };
    if field("codec_name").is_none() {
        return Err("no audio track".into());
    }
    Ok(Probe {
        duration: field("duration").and_then(|d| d.parse::<f64>().ok()).filter(|d| *d > 0.0).map(Duration::from_secs_f64),
        title: field("TAG:title"),
        artist: field("TAG:artist"),
    })
}

/// The first audio stream as 48 kHz stereo. A reader thread keeps a couple of seconds decoded
/// ahead, so the audio thread never waits on the pipe.
pub struct FfmpegSource {
    path: PathBuf,
    duration: Option<Duration>,
    child: Child,
    chunks: Receiver<Vec<f32>>,
    chunk: Vec<f32>,
    at: usize,

    /// The next sample is a right channel.
    odd: bool,
}

impl FfmpegSource {
    pub fn open(path: &Path) -> Result<Self, String> {
        let duration = probe(path)?.duration;
        let (child, chunks) = spawn(path, Duration::ZERO)?;
        Ok(Self { path: path.to_owned(), duration, child, chunks, chunk: Vec::new(), at: 0, odd: false })
    }
}

fn spawn(path: &Path, from: Duration) -> Result<(Child, Receiver<Vec<f32>>), String> {
    let ffmpeg = tool("ffmpeg").ok_or("ffmpeg isn't installed")?;
    let mut child = Command::new(ffmpeg)
        .args(["-nostdin", "-v", "error", "-ss", &format!("{:.3}", from.as_secs_f64()), "-i"])
        .arg(path)
        .args(["-map", "0:a:0", "-vn", "-sn", "-dn", "-ac", &CHANNELS.to_string(), "-ar", &RATE.to_string()])
        .args(["-f", "f32le", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut out = child.stdout.take().ok_or("no pipe")?;
    let (tx, rx) = sync_channel(AHEAD);

    // Ends at the end of the stream, or when the source is dropped or seeks.
    std::thread::spawn(move || {
        let mut buf = vec![0u8; CHUNK_BYTES];
        loop {
            let mut filled = 0;
            while filled < buf.len() {
                match out.read(&mut buf[filled..]) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => filled += n,
                }
            }

            // Whole frames only, so channels stay in step.
            let whole = filled - filled % (4 * CHANNELS as usize);
            let samples = buf[..whole].as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
            if whole > 0 && tx.send(samples).is_err() {
                return;
            }
            if filled < buf.len() {
                return;
            }
        }
    });
    Ok((child, rx))
}

impl Drop for FfmpegSource {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Iterator for FfmpegSource {
    type Item = Sample;

    fn next(&mut self) -> Option<Sample> {
        if let Some(&s) = self.chunk.get(self.at) {
            self.at += 1;
            self.odd = !self.odd;
            return Some(s);
        }

        // Chunks hold whole frames, so a new one always starts on a left sample.
        if !self.odd {
            match self.chunks.try_recv() {
                Ok(chunk) => {
                    self.chunk = chunk;
                    self.at = 0;
                    return self.next();
                }
                Err(TryRecvError::Disconnected) => return None,
                Err(TryRecvError::Empty) => {}
            }
        }

        // ffmpeg is still starting or catching up: play silence rather than block the audio thread.
        self.odd = !self.odd;
        Some(0.0)
    }
}

impl Source for FfmpegSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> ChannelCount {
        ChannelCount::new(CHANNELS).expect("non-zero")
    }

    fn sample_rate(&self) -> SampleRate {
        SampleRate::new(RATE).expect("non-zero")
    }

    fn total_duration(&self) -> Option<Duration> {
        self.duration
    }

    /// Restarts ffmpeg at `pos`.
    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        let (child, chunks) = spawn(&self.path, pos).map_err(|e| SeekError::Other(std::sync::Arc::new(std::io::Error::other(e))))?;
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.child = child;
        self.chunks = chunks;
        self.chunk.clear();
        self.at = 0;
        self.odd = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One second of a 440 Hz tone as Opus in MKV. ffmpeg's sine is at 1/8 scale; −6 dB and the
    /// mono-to-stereo upmix's −3 dB bring its peak to about 0.044.
    fn opus_mkv(dir: &Path) -> Option<PathBuf> {
        let out = dir.join("tone.mkv");
        let ok = Command::new(tool("ffmpeg")?)
            .args(["-nostdin", "-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000", "-t", "1"])
            .args(["-af", "volume=-6dB", "-ac", "2", "-c:a", "libopus"])
            .arg(&out)
            .status()
            .ok()?
            .success();
        ok.then_some(out)
    }

    /// Samples until the stream ends, leaving out the silence played while ffmpeg starts.
    fn heard(src: &mut FfmpegSource) -> Vec<f32> {
        src.by_ref().filter(|s| *s != 0.0).collect()
    }

    #[test]
    fn decodes_opus_in_mkv_and_seeks() {
        let dir = std::env::temp_dir().join(format!("mp3deck-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let Some(path) = opus_mkv(&dir) else {
            eprintln!("skipped: no ffmpeg with libopus");
            return;
        };

        let mut src = FfmpegSource::open(&path).unwrap();
        let d = src.total_duration().unwrap().as_secs_f32();
        assert!((d - 1.0).abs() < 0.05, "duration {d}");
        let all = heard(&mut src);
        let peak = all.iter().fold(0f32, |m, s| m.max(s.abs()));
        assert!((all.len() as f32 / 96_000.0 - 1.0).abs() < 0.05, "{} samples", all.len());
        assert!((peak - 0.044).abs() < 0.01, "peak {peak}");

        src.try_seek(Duration::from_millis(500)).unwrap();
        let rest = heard(&mut src).len() as f32 / 96_000.0;
        assert!((rest - 0.5).abs() < 0.05, "{rest} s after seeking");
        std::fs::remove_dir_all(&dir).ok();
    }
}
