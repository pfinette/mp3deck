//! MP3deck core: the playlist, file collection and the audio engine.

pub mod audio;
pub mod ffmpeg;
pub mod wave;

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// One file in the playlist.
#[derive(Clone, Debug)]
pub struct Track {
    pub path: PathBuf,
    pub title: String,
    pub artist: Option<String>,
    pub duration: Option<Duration>,

    /// The decoder refused the file.
    pub failed: bool,
}

impl Track {
    /// A track named after its file, until its tags are read.
    pub fn new(path: PathBuf) -> Self {
        let title = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        Self { path, title, artist: None, duration: None, failed: false }
    }

    pub fn apply(&mut self, meta: &Meta) {
        if let Some(t) = &meta.title {
            self.title.clone_from(t);
        }
        self.artist.clone_from(&meta.artist);
        if meta.duration.is_some() {
            self.duration = meta.duration;
        }
    }
}

/// Tags and length read from a file.
#[derive(Clone, Debug, Default)]
pub struct Meta {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub duration: Option<Duration>,
}

/// Reads the title, artist and length. Missing or unreadable tags give empty fields.
/// Video files keep their file name as the title, since their tags tend to be noise.
pub fn read_meta(path: &Path) -> Meta {
    use lofty::prelude::*;

    let video = is_video(path);
    let text = |v: Option<std::borrow::Cow<'_, str>>| v.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
    let mut meta = match lofty::read_from_path(path) {
        Ok(file) => {
            let tag = file.primary_tag().or_else(|| file.first_tag());
            Meta {
                title: text(tag.and_then(|t| t.title())),
                artist: text(tag.and_then(|t| t.artist())),
                duration: Some(file.properties().duration()).filter(|d| !d.is_zero()),
            }
        }
        Err(_) => Meta::default(),
    };
    if meta.duration.is_none()
        && let Ok(p) = ffmpeg::probe(path)
    {
        meta = Meta { title: p.title, artist: p.artist, duration: p.duration };
    }
    if video {
        meta.title = None;
        meta.artist = None;
    }
    meta
}

/// Playable files among the given paths. Folders are walked recursively; hidden entries are skipped.
/// Each folder's files come out in natural order, so "2 Song" sorts before "10 Song".
pub fn collect_tracks(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for p in paths {
        if p.is_dir() {
            let mut found = Vec::new();
            walk(p, &mut found);
            found.sort_by(|a, b| natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()));
            out.extend(found);
        } else if is_playable(p) {
            out.push(p.clone());
        }
    }
    out
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        match entry.file_type() {
            Ok(t) if t.is_dir() => walk(&path, out),
            Ok(t) if t.is_file() && is_playable(&path) => out.push(path),
            _ => {}
        }
    }
}

/// MP3s, plus the audio of MP4 and MKV files.
pub const EXTENSIONS: [&str; 7] = ["mp3", "m4a", "mp4", "m4v", "mov", "mkv", "webm"];

fn extension(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

pub fn is_playable(path: &Path) -> bool {
    EXTENSIONS.contains(&extension(path).as_str())
}

pub fn is_video(path: &Path) -> bool {
    ["mp4", "m4v", "mov", "mkv", "webm"].contains(&extension(path).as_str())
}

/// Compares strings with runs of digits compared as numbers.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars<'_>>| {
                    let mut n = String::new();
                    while let Some(c) = it.next_if(char::is_ascii_digit) {
                        n.push(c);
                    }
                    n
                };
                let (na, nb) = (take(&mut a), take(&mut b));
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                let ord = x.to_lowercase().cmp(y.to_lowercase());
                if ord != Ordering::Equal {
                    return ord;
                }
                a.next();
                b.next();
            }
        }
    }
}

/// "3:07", or "1:02:03" past an hour.
pub fn format_time(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Repeat {
    #[default]
    Off,
    One,
    All,
}

impl Repeat {
    pub const ALL: [Repeat; 3] = [Repeat::Off, Repeat::One, Repeat::All];

    pub fn label(self) -> &'static str {
        match self {
            Repeat::Off => "Off",
            Repeat::One => "One",
            Repeat::All => "All",
        }
    }
}

/// The tracks, which one is current, and the order they play in.
#[derive(Default)]
pub struct Playlist {
    pub tracks: Vec<Track>,
    pub current: Option<usize>,
    pub repeat: Repeat,

    /// Play order while shuffling. Always a permutation of the track indices.
    shuffle: Option<Vec<usize>>,
    rng: Rng,
}

impl Playlist {
    pub fn shuffled(&self) -> bool {
        self.shuffle.is_some()
    }

    /// Adds tracks at the end and returns the index of the first new one.
    pub fn add(&mut self, paths: Vec<PathBuf>) -> usize {
        let start = self.tracks.len();
        self.tracks.extend(paths.into_iter().map(Track::new));
        let mut new: Vec<usize> = (start..self.tracks.len()).collect();
        if let Some(order) = &mut self.shuffle {
            self.rng.shuffle(&mut new);
            order.extend(new);
        }
        start
    }

    pub fn remove(&mut self, i: usize) {
        if i >= self.tracks.len() {
            return;
        }
        self.tracks.remove(i);
        self.current = match self.current {
            Some(c) if c == i => None,
            Some(c) if c > i => Some(c - 1),
            c => c,
        };
        if let Some(order) = &mut self.shuffle {
            order.retain(|&k| k != i);
            for k in order.iter_mut() {
                if *k > i {
                    *k -= 1;
                }
            }
        }
    }

    pub fn clear(&mut self) {
        self.tracks.clear();
        self.current = None;
        if let Some(order) = &mut self.shuffle {
            order.clear();
        }
    }

    /// Shuffling starts from the current track, so it keeps playing and the rest follow at random.
    pub fn set_shuffle(&mut self, on: bool) {
        if !on {
            self.shuffle = None;
            return;
        }
        let mut order: Vec<usize> = (0..self.tracks.len()).filter(|&k| Some(k) != self.current).collect();
        self.rng.shuffle(&mut order);
        if let Some(c) = self.current {
            order.insert(0, c);
        }
        self.shuffle = Some(order);
    }

    fn order(&self) -> Vec<usize> {
        self.shuffle.clone().unwrap_or_else(|| (0..self.tracks.len()).collect())
    }

    /// The track after the current one. `auto` is true when a track ran out on its own,
    /// which is when Repeat One replays it.
    pub fn next_index(&self, auto: bool) -> Option<usize> {
        let order = self.order();
        let Some(c) = self.current else { return order.first().copied() };
        if auto && self.repeat == Repeat::One {
            return Some(c);
        }
        let pos = order.iter().position(|&k| k == c)?;
        match order.get(pos + 1) {
            Some(&k) => Some(k),
            None if self.repeat == Repeat::All => order.first().copied(),
            None => None,
        }
    }

    pub fn prev_index(&self) -> Option<usize> {
        let order = self.order();
        let c = self.current?;
        let pos = order.iter().position(|&k| k == c)?;
        match pos.checked_sub(1) {
            Some(p) => Some(order[p]),
            None if self.repeat == Repeat::All => order.last().copied(),
            None => None,
        }
    }
}

/// xorshift64, seeded from the clock. Good enough to shuffle a playlist.
struct Rng(u64);

impl Default for Rng {
    fn default() -> Self {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
        Rng(nanos | 1)
    }
}

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            items.swap(i, (self.next() % (i as u64 + 1)) as usize);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(n: usize) -> Playlist {
        let mut p = Playlist::default();
        p.add((0..n).map(|i| PathBuf::from(format!("{i}.mp3"))).collect());
        p
    }

    #[test]
    fn natural_order() {
        let mut v = vec!["10 b.mp3", "2 a.mp3", "1 c.mp3", "Track 02.mp3", "track 1.mp3"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["1 c.mp3", "2 a.mp3", "10 b.mp3", "track 1.mp3", "Track 02.mp3"]);
    }

    #[test]
    fn times() {
        assert_eq!(format_time(Duration::from_secs(187)), "3:07");
        assert_eq!(format_time(Duration::from_secs(3723)), "1:02:03");
    }

    #[test]
    fn next_and_prev_follow_repeat() {
        let mut p = list(3);
        assert_eq!(p.next_index(false), Some(0));
        p.current = Some(2);
        assert_eq!(p.next_index(true), None);
        p.repeat = Repeat::All;
        assert_eq!(p.next_index(true), Some(0));
        p.repeat = Repeat::One;
        assert_eq!(p.next_index(true), Some(2));
        assert_eq!(p.next_index(false), None);
        p.current = Some(0);
        assert_eq!(p.prev_index(), None);
        p.repeat = Repeat::All;
        assert_eq!(p.prev_index(), Some(2));
    }

    #[test]
    fn shuffle_keeps_current_first_and_visits_all() {
        let mut p = list(20);
        p.current = Some(7);
        p.set_shuffle(true);
        let mut seen = vec![7];
        while let Some(n) = p.next_index(false) {
            seen.push(n);
            p.current = Some(n);
        }
        seen.sort_unstable();
        assert_eq!(seen, (0..20).collect::<Vec<_>>());
    }

    #[test]
    fn remove_shifts_current_and_order() {
        let mut p = list(4);
        p.set_shuffle(true);
        p.current = Some(3);
        p.remove(1);
        assert_eq!(p.current, Some(2));
        let mut order = p.order();
        order.sort_unstable();
        assert_eq!(order, [0, 1, 2]);
        p.remove(2);
        assert_eq!(p.current, None);
    }
}
