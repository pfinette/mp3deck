//! Waveform overviews: how loud a track is every 50 ms, for drawing it.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Mutex;

use rodio::Source;

use crate::{audio, ffmpeg};

/// Windows per second.
pub const RATE: u32 = 20;

/// Appends the RMS of every 50 ms of the track, mixed to mono, to `into` as it goes, so a long
/// file can be drawn while it is read. `keep_going` is polled now and then; when it returns false
/// the work stops. Returns false when it stopped early. An unreadable file adds nothing.
pub fn analyze(path: &Path, into: &Mutex<Vec<f32>>, keep_going: &dyn Fn() -> bool) -> bool {
    match audio::open_native(path) {
        Some(Ok(decoder)) => from_source(decoder, into, keep_going),
        _ => from_ffmpeg(path, into, keep_going),
    }
}

/// Batches windows and hands them over a few hundred at a time.
struct Windows<'a> {
    size: usize,
    n: usize,
    sum: f32,
    batch: Vec<f32>,
    into: &'a Mutex<Vec<f32>>,
}

impl<'a> Windows<'a> {
    fn new(sample_rate: u32, into: &'a Mutex<Vec<f32>>) -> Self {
        Self { size: (sample_rate / RATE).max(1) as usize, n: 0, sum: 0.0, batch: Vec::new(), into }
    }

    fn push(&mut self, v: f32) {
        self.sum += v * v;
        self.n += 1;
        if self.n == self.size {
            self.batch.push((self.sum / self.n as f32).sqrt());
            self.n = 0;
            self.sum = 0.0;
            if self.batch.len() >= 400 {
                self.flush();
            }
        }
    }

    fn flush(&mut self) {
        if let Ok(mut v) = self.into.lock() {
            v.append(&mut self.batch);
        }
    }

    fn finish(mut self) {
        if self.n > 0 {
            self.batch.push((self.sum / self.n as f32).sqrt());
        }
        self.flush();
    }
}

fn from_source(src: impl Source, into: &Mutex<Vec<f32>>, keep_going: &dyn Fn() -> bool) -> bool {
    let channels = src.channels().get() as usize;
    let mut windows = Windows::new(src.sample_rate().get(), into);
    let (mut frame, mut ch, mut frames) = (0.0, 0, 0usize);
    for s in src {
        frame += s;
        ch += 1;
        if ch == channels {
            windows.push(frame / channels as f32);
            (frame, ch) = (0.0, 0);
            frames += 1;
            if frames % 65_536 == 0 && !keep_going() {
                return false;
            }
        }
    }
    windows.finish();
    true
}

/// Decodes at 8 kHz mono, which is plenty for an overview and keeps hour-long files quick.
fn from_ffmpeg(path: &Path, into: &Mutex<Vec<f32>>, keep_going: &dyn Fn() -> bool) -> bool {
    const RATE_HZ: u32 = 8_000;
    let Some(ffmpeg) = ffmpeg::tool("ffmpeg") else { return true };
    let Ok(mut child) = Command::new(ffmpeg)
        .args(["-nostdin", "-v", "error", "-i"])
        .arg(path)
        .args(["-map", "0:a:0", "-vn", "-ac", "1", "-ar", &RATE_HZ.to_string(), "-f", "f32le", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return true;
    };
    let Some(mut out) = child.stdout.take() else { return true };
    let mut windows = Windows::new(RATE_HZ, into);
    let mut buf = vec![0u8; 64 * 1024];
    let mut carry = Vec::new();
    loop {
        if !keep_going() {
            let _ = child.kill();
            let _ = child.wait();
            return false;
        }
        let n = match out.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        carry.extend_from_slice(&buf[..n]);
        let whole = carry.len() - carry.len() % 4;
        for b in carry[..whole].as_chunks::<4>().0 {
            windows.push(f32::from_le_bytes(*b));
        }
        carry.drain(..whole);
    }
    let _ = child.wait();
    windows.finish();
    true
}
