use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::text::{LayoutJob, TextFormat, TextWrapping};
use eframe::egui::{
    self, Align, Align2, Color32, FontData, FontDefinitions, FontFamily, FontId, Key, Layout, Modifiers, Painter, Pos2, Rect,
    Role, Sense, Shape, Stroke, Theme, UiBuilder, WidgetInfo, pos2, vec2,
};
use mp3deck::audio::{Deck, Transport, open_output};
use mp3deck::{EXTENSIONS, Meta, Repeat, collect_tracks, format_time, read_meta, wave};
use serde::{Deserialize, Serialize};

const WIDTH: f32 = 880.0;
const HEIGHT: f32 = 540.0;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("MP3deck")
            .with_inner_size([WIDTH, HEIGHT])
            .with_min_inner_size([800.0, 500.0])
            .with_max_inner_size([1400.0, 1000.0])
            .with_drag_and_drop(true)
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false)
            .with_icon(egui::IconData::default()),
        ..Default::default()
    };
    eframe::run_native("MP3deck", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

// One flat ground, ink, a dim tone for whatever is not there yet, and one orange for what is live:
// the playhead, the playing track, the arcs of the knobs.

struct Palette {
    ground: Color32,
    ink: Color32,
    dim: Color32,
    label: Color32,
    select: Color32,
    hover: Color32,
}

const LIGHT: Palette = Palette {
    ground: Color32::from_rgb(0xEF, 0xEE, 0xE9),
    ink: Color32::from_rgb(0x12, 0x12, 0x12),
    dim: Color32::from_rgb(0xD3, 0xD1, 0xC8),
    label: Color32::from_rgb(0x6C, 0x6A, 0x64),
    select: Color32::from_rgb(0xE3, 0xE1, 0xD9),
    hover: Color32::from_rgb(0xE9, 0xE8, 0xE1),
};

const DARK: Palette = Palette {
    ground: Color32::from_rgb(0x15, 0x15, 0x14),
    ink: Color32::from_rgb(0xEC, 0xEA, 0xE4),
    dim: Color32::from_rgb(0x3B, 0x3A, 0x37),
    label: Color32::from_rgb(0x8F, 0x8D, 0x86),
    select: Color32::from_rgb(0x26, 0x25, 0x23),
    hover: Color32::from_rgb(0x1E, 0x1D, 0x1C),
};

const LIVE: Color32 = Color32::from_rgb(0xEE, 0x4F, 0x17);

fn palette(ui: &egui::Ui) -> &'static Palette {
    if ui.visuals().dark_mode { &DARK } else { &LIGHT }
}

/// Helvetica Neue in three cuts. Its figures are tabular, so running times don't jitter.
fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}

fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("medium".into()))
}

fn regular(size: f32) -> FontId {
    FontId::proportional(size)
}

/// Text with extra letter spacing, for the small labels.
fn tracked(text: impl Into<String>, font: FontId, spacing: f32, color: Color32) -> LayoutJob {
    LayoutJob::single_section(text.into(), TextFormat { font_id: font, extra_letter_spacing: spacing, color, ..Default::default() })
}

/// Text cut to `rows` lines of `width` with an ellipsis.
fn fitted(mut job: LayoutJob, width: f32, rows: usize) -> LayoutJob {
    job.wrap = TextWrapping { max_width: width.max(10.0), max_rows: rows, break_anywhere: rows == 1, overflow_character: Some('…') };
    job
}

fn install_style(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let fallback = fonts.families[&FontFamily::Proportional].clone();
    for name in ["bold", "medium"] {
        fonts.families.insert(FontFamily::Name(name.into()), fallback.clone());
    }

    // One read of the collection, shared by the three cuts.
    if let Ok(bytes) = std::fs::read("/System/Library/Fonts/HelveticaNeue.ttc") {
        let bytes: &'static [u8] = Box::leak(bytes.into_boxed_slice());
        for (name, index, family) in [
            ("Helvetica Neue", 0, FontFamily::Proportional),
            ("Helvetica Neue Bold", 1, FontFamily::Name("bold".into())),
            ("Helvetica Neue Medium", 10, FontFamily::Name("medium".into())),
        ] {
            let mut data = FontData::from_static(bytes);
            data.index = index;
            fonts.font_data.insert(name.into(), Arc::new(data));
            fonts.families.entry(family).or_default().insert(0, name.into());
        }
    }
    ctx.set_fonts(fonts);

    for (theme, p) in [(Theme::Light, &LIGHT), (Theme::Dark, &DARK)] {
        ctx.style_mut_of(theme, |style| {
            style.spacing.scroll = egui::style::ScrollStyle::floating();
            style.interaction.tooltip_delay = 0.6;
            style.interaction.selectable_labels = false;
            let v = &mut style.visuals;
            v.panel_fill = p.ground;
            v.window_fill = p.ground;
            v.window_stroke = Stroke::new(1.0, p.dim);
            v.selection.bg_fill = LIVE;
            v.selection.stroke.color = p.ink;
            v.widgets.noninteractive.fg_stroke.color = p.ink;
            v.widgets.noninteractive.bg_stroke.color = p.dim;
            v.widgets.inactive.fg_stroke.color = p.ink;
            v.widgets.inactive.bg_fill = p.dim;
            v.widgets.hovered.fg_stroke.color = p.ink;
            v.widgets.hovered.weak_bg_fill = p.select;
            v.widgets.hovered.bg_fill = p.label;
            v.widgets.active.fg_stroke.color = p.ink;
            v.widgets.active.weak_bg_fill = p.dim;
            v.widgets.active.bg_fill = p.label;
        });
    }
}

/// Bumped when the default window size changes, so an old saved size is replaced once.
const LAYOUT: u32 = 4;

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Prefs {
    volume: f64,
    repeat: Repeat,
    shuffle: bool,
    playlist: Vec<PathBuf>,
    current: Option<usize>,

    /// Seconds into the current track when the app last saved, 0 when stopped.
    position: f64,
    layout: u32,
}

const DEFAULT_VOLUME: f64 = 80.0;

impl Default for Prefs {
    fn default() -> Self {
        Self { volume: DEFAULT_VOLUME, repeat: Repeat::Off, shuffle: false, playlist: Vec::new(), current: None, position: 0.0, layout: 0 }
    }
}

/// 0 VU sits at −10 dBFS RMS, so loud modern masters swing the needles around zero.
const VU_REF_DBFS: f32 = -10.0;

/// Needle time constant. A VU needle takes about 300 ms to settle.
const VU_TAU_S: f32 = 0.09;

/// Where a level in VU falls on the scale, from 0 (−20 VU) to 1 (+3 VU). The scale is linear in
/// amplitude, which crowds the low end, as on a real VU meter.
fn vu_pos(vu: f32) -> f32 {
    let (lo, hi) = (0.1, 10f32.powf(3.0 / 20.0));
    ((10f32.powf(vu / 20.0) - lo) / (hi - lo)).clamp(0.0, 1.0)
}

/// Needle positions, left and right. They read the music before the volume knob.
#[derive(Default)]
struct Meters {
    needle: [f32; 2],
    last: Option<Instant>,
}

impl Meters {
    /// Moves toward the latest levels. Returns true while a needle is still moving.
    fn update(&mut self, levels: [(f32, f32); 2], live: bool) -> bool {
        let now = Instant::now();
        let dt = self.last.map_or(0.0, |t| (now - t).as_secs_f32().min(0.1));
        self.last = Some(now);
        let follow = 1.0 - (-dt / VU_TAU_S).exp();
        for (ch, (rms, _)) in levels.into_iter().enumerate() {
            let target = if live && rms > 1e-6 { vu_pos(20.0 * rms.log10() - VU_REF_DBFS) } else { 0.0 };
            self.needle[ch] += (target - self.needle[ch]) * follow;
        }
        live || self.needle.iter().any(|&n| n > 0.002)
    }
}

/// A track's waveform overview, filled in by a background thread.
#[derive(Default)]
struct Wave {
    windows: Mutex<Vec<f32>>,
    state: AtomicU8,
}

const READING: u8 = 0;
const READ: u8 = 1;
const ABANDONED: u8 = 2;

struct App {
    deck: Arc<Mutex<Deck>>,

    /// Keeps the output device open. `None` when there is none.
    _output: Option<rodio::MixerDeviceSink>,

    meta_tx: Sender<(PathBuf, Meta)>,
    meta_rx: Receiver<(PathBuf, Meta)>,
    selected: Option<usize>,
    meters: Meters,

    waves: HashMap<PathBuf, Arc<Wave>>,

    /// Bumped for each new waveform job; older jobs see the change and stop.
    wave_job: Arc<AtomicU64>,

    /// Where the waveform is being dragged to, as a fraction of the track.
    scrub: Option<f32>,

    /// Drag travel not yet spent on a step of a stepped knob.
    knob_travel: f32,
    volume: f64,
    shuffle: bool,

    /// Paths given on the command line, added on the first frame.
    launch_paths: Vec<PathBuf>,

    /// Set when the saved window size predates this layout.
    reset_size: bool,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_style(&cc.egui_ctx);
        let prefs: Option<Prefs> = cc.storage.and_then(|s| eframe::get_value(s, eframe::APP_KEY));
        let reset_size = prefs.as_ref().is_some_and(|p| p.layout < LAYOUT);
        let prefs = prefs.unwrap_or_default();

        let (output, player) = match open_output() {
            Ok((sink, player)) => (Some(sink), Some(player)),
            Err(_) => (None, None),
        };
        let mut deck = Deck::new(player);
        deck.set_volume((prefs.volume / 100.0) as f32);
        deck.set_repeat(prefs.repeat);

        let saved: Vec<PathBuf> = prefs.playlist.into_iter().filter(|p| p.is_file()).collect();
        deck.playlist.add(saved.clone());
        deck.playlist.current = prefs.current.filter(|&c| c < deck.playlist.tracks.len());
        deck.playlist.set_shuffle(prefs.shuffle);
        if let Some(c) = deck.playlist.current.filter(|_| prefs.position > 0.0) {
            deck.cue(c, Duration::from_secs_f64(prefs.position));
        }

        let deck = Arc::new(Mutex::new(deck));
        spawn_watcher(&deck, cc.egui_ctx.clone());

        let (meta_tx, meta_rx) = channel();
        read_meta_in_background(saved, meta_tx.clone(), cc.egui_ctx.clone());

        Self {
            deck,
            _output: output,
            meta_tx,
            meta_rx,
            selected: None,
            meters: Meters::default(),
            waves: HashMap::new(),
            wave_job: Arc::default(),
            scrub: None,
            knob_travel: 0.0,
            volume: prefs.volume,
            shuffle: prefs.shuffle,
            launch_paths: std::env::args_os().skip(1).map(PathBuf::from).collect(),
            reset_size,
        }
    }

    /// Adds files and folders. Starts playing when the deck was idle.
    fn add(&mut self, paths: &[PathBuf], deck: &mut Deck, ctx: &egui::Context) {
        let files = collect_tracks(paths);
        if files.is_empty() {
            return;
        }
        let first = deck.playlist.add(files.clone());
        read_meta_in_background(files, self.meta_tx.clone(), ctx.clone());
        if deck.transport == Transport::Stopped {
            deck.play(first);
            self.selected = Some(first);
        }
    }

    fn open_dialog(&mut self, deck: &mut Deck, ctx: &egui::Context) {
        let dialog = rfd::FileDialog::new().set_title("Load Tracks").add_filter("MP3, MP4 and MKV", &EXTENSIONS);
        if let Some(files) = dialog.pick_files() {
            self.add(&files, deck, ctx);
        }
    }

    fn clear(&mut self, deck: &mut Deck) {
        deck.clear();
        self.selected = None;
    }

    /// Space or the play key: start the selection when stopped, otherwise pause or resume.
    fn play_pause(&self, deck: &mut Deck) {
        match (deck.transport, self.selected) {
            (Transport::Stopped, Some(i)) if i < deck.playlist.tracks.len() => deck.play(i),
            _ => deck.toggle(),
        }
    }

    /// The waveform of `path`, starting a reader when there is none yet.
    fn wave(&mut self, path: &Path, ctx: &egui::Context) -> Arc<Wave> {
        if let Some(w) = self.waves.get(path).filter(|w| w.state.load(Ordering::Relaxed) != ABANDONED) {
            return w.clone();
        }
        let w = Arc::new(Wave::default());
        self.waves.insert(path.to_owned(), w.clone());
        let job = self.wave_job.fetch_add(1, Ordering::Relaxed) + 1;
        let (latest, ctx, path, out) = (self.wave_job.clone(), ctx.clone(), path.to_owned(), w.clone());
        std::thread::spawn(move || {
            let read = wave::analyze(&path, &out.windows, &|| latest.load(Ordering::Relaxed) == job);
            out.state.store(if read { READ } else { ABANDONED }, Ordering::Relaxed);
            ctx.request_repaint();
        });
        w
    }

    fn keys(&mut self, deck: &mut Deck, ctx: &egui::Context) {
        // Consumed first, so Space never also presses a focused control.
        let pressed = |key: Key, mods: Modifiers| ctx.input_mut(|i| i.consume_key(mods, key));
        let n = deck.playlist.tracks.len();

        if pressed(Key::Space, Modifiers::NONE) {
            self.play_pause(deck);
        }
        if pressed(Key::O, Modifiers::COMMAND) {
            self.open_dialog(deck, ctx);
        }
        if pressed(Key::W, Modifiers::COMMAND) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if pressed(Key::ArrowRight, Modifiers::COMMAND) {
            deck.next();
        }
        if pressed(Key::ArrowLeft, Modifiers::COMMAND) {
            deck.prev();
        }
        if n > 0 {
            if pressed(Key::ArrowDown, Modifiers::NONE) {
                self.selected = Some(self.selected.map_or(0, |s| (s + 1).min(n - 1)));
            }
            if pressed(Key::ArrowUp, Modifiers::NONE) {
                self.selected = Some(self.selected.map_or(0, |s| s.saturating_sub(1)));
            }
        }
        if let Some(i) = self.selected.filter(|&i| i < n) {
            if pressed(Key::Enter, Modifiers::NONE) {
                deck.play(i);
            }
            if pressed(Key::Backspace, Modifiers::NONE) || pressed(Key::Delete, Modifiers::NONE) {
                deck.remove(i);
                self.selected = (i < deck.playlist.tracks.len()).then_some(i).or(i.checked_sub(1));
            }
        }
    }

    /// The title bar: drag to move, double-click to zoom. The deck's state sits on the right.
    fn title_strip(&self, ui: &mut egui::Ui, deck: &Deck) {
        let p = palette(ui);
        let bar = Rect::from_min_max(pos2(0.0, 0.0), pos2(ui.ctx().content_rect().right(), ui.max_rect().bottom()));
        let background = ui.interact(bar, ui.scope_id().with("titlebar"), Sense::click_and_drag());
        if background.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if background.double_clicked() {
            let zoomed = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!zoomed));
        }

        let n = deck.playlist.tracks.len();
        let at = deck.playlist.current.map_or(0, |c| c + 1);
        let (status, color) = if !deck.has_output() {
            ("NO AUDIO OUTPUT".to_owned(), LIVE)
        } else if deck.error.is_some() && deck.transport == Transport::Stopped {
            ("CAN'T PLAY".to_owned(), LIVE)
        } else {
            match (deck.transport, n) {
                (_, 0) => ("READY".to_owned(), p.label),
                (Transport::Playing, _) => (format!("PLAYING {at}/{n}"), LIVE),
                (Transport::Paused, _) => (format!("PAUSED {at}/{n}"), p.ink),
                (Transport::Stopped, _) => (format!("STOPPED {at}/{n}"), p.label),
            }
        };
        let r = ui.max_rect();
        let g = ui.painter().layout_job(tracked(status, bold(10.0), 1.2, color));
        ui.painter().galley(pos2(r.right() - g.size().x, r.center().y - g.size().y / 2.0), g, color);
    }

    /// The track number in the gutter, the title in large bold type, and one line under it.
    fn header(&self, ui: &mut egui::Ui, deck: &Deck, hovering: bool) {
        let p = palette(ui);
        let width = ui.available_width() - GUTTER;
        let track = deck.playlist.current.and_then(|c| deck.playlist.tracks.get(c));

        let title = if hovering {
            "Release to load".to_owned()
        } else if let Some(t) = track {
            t.title.clone()
        } else {
            "Drop music here".to_owned()
        };
        let (detail, detail_color) = match (&deck.error, deck.transport, track) {
            (Some(e), Transport::Stopped, _) => (format!("Can't play: {e}"), LIVE),
            (_, _, None) => ("MP3, MP4 or MKV files and folders, or ⌘O to choose.".to_owned(), p.label),
            (_, _, Some(t)) => {
                let folder = || t.path.parent().and_then(|p| p.file_name()).map(|f| f.to_string_lossy().into_owned());
                (t.artist.clone().or_else(folder).unwrap_or_default(), p.label)
            }
        };
        let painter = ui.painter();
        let title = painter.layout_job(fitted(LayoutJob::simple(title, bold(TITLE), p.ink, width), width, 2));
        let detail = painter.layout_job(fitted(LayoutJob::simple_singleline(detail, regular(13.0), detail_color), width, 1));
        let height = title.size().y + 6.0 + detail.size().y;
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());

        let painter = ui.painter();
        if let (Some(c), false) = (deck.playlist.current, hovering) {
            painter.text(rect.left_top(), Align2::LEFT_TOP, (c + 1).to_string(), bold(TITLE), LIVE);
        }
        let text_left = rect.left() + GUTTER;
        let title_height = title.size().y;
        painter.galley(pos2(text_left, rect.top()), title, p.ink);
        painter.galley(pos2(text_left, rect.top() + title_height + 6.0), detail, detail_color);
    }

    /// The track's waveform: played in ink, the rest dim, the playhead in orange. Click or drag to jump.
    fn waveform(&mut self, ui: &mut egui::Ui, deck: &mut Deck, wave: Option<&Wave>) {
        let p = palette(ui);
        let (rect, hit) = ui.allocate_exact_size(vec2(ui.available_width(), WAVE_H), Sense::click_and_drag());
        let loaded = deck.playlist.current.is_some();
        let duration = deck.duration.filter(|d| !d.is_zero() && loaded);
        let fraction_at = |pos: Pos2| ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        if let (Some(d), Some(pos)) = (duration, hit.interact_pointer_pos()) {
            if hit.dragged() || hit.is_pointer_button_down_on() {
                self.scrub = Some(fraction_at(pos));
            }
            if hit.drag_stopped() || hit.clicked() {
                deck.seek(d.mul_f32(fraction_at(pos)));
                self.scrub = None;
            }
        }
        if !hit.is_pointer_button_down_on() && !hit.dragged() {
            self.scrub = None;
        }
        let position = match (self.scrub, duration) {
            (Some(f), Some(d)) => d.mul_f32(f),
            _ => deck.position(),
        };
        let played = duration.map_or(0.0, |d| (position.as_secs_f32() / d.as_secs_f32()).clamp(0.0, 1.0));

        // One bar per few points, each the loudest stretch it covers. Bars not read yet are dots.
        let guard = wave.map(|w| w.windows.lock().unwrap_or_else(|e| e.into_inner()));
        let windows: &[f32] = guard.as_deref().map_or(&[], Vec::as_slice);
        let expected = duration.map_or(windows.len(), |d| (d.as_secs_f32() * wave::RATE as f32).ceil() as usize).max(1);
        let loudest = windows.iter().copied().fold(0.0f32, f32::max).max(1e-4);
        let count = ((rect.width() + BAR_GAP) / (BAR + BAR_GAP)).floor() as usize;
        let painter = ui.painter_at(rect.expand(6.0));
        let cy = rect.center().y;
        for k in 0..count {
            let x = rect.left() + k as f32 * (BAR + BAR_GAP);
            let color = if loaded && (x + BAR / 2.0 - rect.left()) / rect.width() < played { p.ink } else { p.dim };
            let (from, to) = (k * expected / count, ((k + 1) * expected / count).max(k * expected / count + 1));
            if from < windows.len() {
                let level = windows[from..to.min(windows.len())].iter().copied().fold(0.0f32, f32::max) / loudest;
                let h = (level * rect.height()).max(2.0);
                painter.rect_filled(Rect::from_min_size(pos2(x, cy - h / 2.0), vec2(BAR, h)), 0.0, color);
            } else {
                painter.rect_filled(Rect::from_center_size(pos2(x + BAR / 2.0, cy), vec2(2.0, 2.0)), 0.0, color);
            }
        }
        if loaded {
            let x = rect.left() + rect.width() * played;
            painter.vline(x, rect.y_range().expand(5.0), Stroke::new(2.0, LIVE));
        }

        let tip = duration.zip(hit.hover_pos()).map(|(d, pos)| format_time(d.mul_f32(fraction_at(pos))));
        let hit = match tip {
            Some(t) => hit.on_hover_text_at_pointer(t),
            None => hit,
        };
        hit.widget_info(|| WidgetInfo::slider(duration.is_some(), (played * 100.0).round() as f64, "Position"));

        // Elapsed under the left end, time left under the right.
        ui.add_space(10.0);
        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 14.0), Sense::hover());
        let painter = ui.painter();
        if loaded {
            painter.text(row.left_center(), Align2::LEFT_CENTER, format_time(position), medium(12.0), p.ink);
        }
        if let Some(d) = duration {
            let rest = format!("−{}", format_time(d.saturating_sub(position)));
            painter.text(row.right_center(), Align2::RIGHT_CENTER, rest, medium(12.0), p.label);
        }
    }

    /// Left and right VU dials, side by side in the space they are given.
    fn vu_meters(&self, ui: &mut egui::Ui) {
        let rect = ui.available_rect_before_wrap();
        let w = rect.width() / 2.0;
        for (ch, name) in ["LEFT", "RIGHT"].into_iter().enumerate() {
            let r = Rect::from_min_size(rect.min + vec2(ch as f32 * w, 0.0), vec2(w, rect.height()));
            vu_dial(ui, r, self.meters.needle[ch], name);
        }
    }

    /// Transport icons. The deck's state is underlined: orange while playing.
    fn transport(&mut self, ui: &mut egui::Ui, deck: &mut Deck) {
        let has_tracks = !deck.playlist.tracks.is_empty();
        let mut load = false;
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ICON_BOX.y), Sense::hover());
        let mut ui = ui.new_child(UiBuilder::new().max_rect(rect).layout(Layout::left_to_right(Align::Min)));
        ui.spacing_mut().item_spacing.x = 4.0;
        // The first icon's glyph lines up with the gutter's left edge.
        ui.add_space(-(ICON_BOX.x - 18.0) / 2.0);

        let state = deck.transport;
        if icon_button(&mut ui, Icon::Prev, None, has_tracks, "Previous (⌘←)") {
            deck.prev();
        }
        let mark = (state == Transport::Playing).then_some(LIVE);
        if icon_button(&mut ui, Icon::Play, mark, has_tracks, "Play (Space)") && state != Transport::Playing {
            self.play_pause(deck);
        }
        let mark = (state == Transport::Paused).then_some(palette(&ui).ink);
        if icon_button(&mut ui, Icon::Pause, mark, state != Transport::Stopped, "Pause (Space)") {
            self.play_pause(deck);
        }
        if icon_button(&mut ui, Icon::Stop, None, state != Transport::Stopped, "Stop") {
            deck.stop();
        }
        if icon_button(&mut ui, Icon::Next, None, has_tracks, "Next (⌘→)") {
            deck.next();
        }
        ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
            ui.add_space(-(ICON_BOX.x - 20.0) / 2.0);
            load = icon_button(ui, Icon::Folder, None, deck.has_output(), "Load tracks (⌘O)");
        });
        if load {
            self.open_dialog(deck, ui.ctx());
        }
    }

    /// Three knobs, as on fm1's pages: volume, repeat and shuffle.
    fn knobs(&mut self, ui: &mut egui::Ui, deck: &mut Deck) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), KNOB_ROW_H), Sense::hover());
        let column = rect.width() / 3.0;
        let centers = [0, 1, 2].map(|k| pos2(rect.left() + column * (k as f32 + 0.5), rect.top() + KNOB_R + 4.0));

        // Volume: drag up or down, arrows when focused, double-click resets.
        let r = knob_hit(ui, centers[0], "volume");
        let before = self.volume;
        if r.dragged() {
            self.volume -= r.drag_delta().y as f64 * 0.5;
        }
        if r.drag_stopped() {
            self.volume = self.volume.round();
        }
        if r.double_clicked() {
            self.volume = DEFAULT_VOLUME;
        }
        self.volume += arrows(ui, &r) as f64 * 5.0;
        self.volume = self.volume.clamp(0.0, 100.0);
        if self.volume != before {
            deck.set_volume((self.volume / 100.0) as f32);
        }
        let shown = self.volume.round();
        r.widget_info(|| WidgetInfo::slider(true, shown, "Volume"));
        let r = r.on_hover_text("Drag up or down; double-click resets.");
        knob(ui, centers[0], (self.volume / 100.0) as f32, &format!("{shown:.0}%"), "VOLUME", r.has_focus());

        // Repeat: click for the next setting, or drag through them.
        let r = knob_hit(ui, centers[1], "repeat");
        let at = Repeat::ALL.iter().position(|&x| x == deck.playlist.repeat).unwrap_or(0) as i32;
        let mut step = arrows(ui, &r) + self.drag_steps(&r);
        if r.clicked() {
            step = if at == 2 { -2 } else { 1 };
        }
        let at = (at + step).clamp(0, 2);
        deck.set_repeat(Repeat::ALL[at as usize]);
        let repeat = deck.playlist.repeat;
        r.widget_info(|| WidgetInfo::slider(true, at as f64, format!("Repeat {}", repeat.label())));
        let r = r.on_hover_text("Off, one track, or the whole list. Click or drag.");
        knob(ui, centers[1], at as f32 / 2.0, &repeat.label().to_uppercase(), "REPEAT", r.has_focus());

        // Shuffle: click to flip, or drag up for on and down for off.
        let r = knob_hit(ui, centers[2], "shuffle");
        let step = arrows(ui, &r) + self.drag_steps(&r);
        let on = if r.clicked() { !self.shuffle } else if step != 0 { step > 0 } else { self.shuffle };
        if on != self.shuffle {
            self.shuffle = on;
            deck.playlist.set_shuffle(on);
        }
        r.widget_info(|| WidgetInfo::selected(Role::Switch, true, on, "Shuffle"));
        let r = r.on_hover_text("Plays the rest of the list in random order. Click or drag.");
        knob(ui, centers[2], if on { 1.0 } else { 0.0 }, if on { "ON" } else { "OFF" }, "SHUFFLE", r.has_focus());
    }

    /// Whole steps of drag on a stepped knob: up is +1.
    fn drag_steps(&mut self, r: &egui::Response) -> i32 {
        if r.drag_started() {
            self.knob_travel = 0.0;
        }
        if !r.dragged() {
            return 0;
        }
        self.knob_travel -= r.drag_delta().y;
        let steps = (self.knob_travel / STEP_TRAVEL).trunc();
        self.knob_travel -= steps * STEP_TRAVEL;
        steps as i32
    }

    /// The playlist: number in the gutter, title and artist, length or time left.
    fn playlist(&mut self, ui: &mut egui::Ui, deck: &mut Deck) {
        let p = palette(ui);
        let n = deck.playlist.tracks.len();
        let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
        let g = ui.painter().layout_job(tracked("PLAYLIST", bold(10.0), 1.2, p.label));
        ui.painter().galley(pos2(head.left(), head.center().y - g.size().y / 2.0), g, p.label);
        let total: Duration = deck.playlist.tracks.iter().filter_map(|t| t.duration).sum();
        let summary = match n {
            0 => "No tracks".to_owned(),
            1 => format!("1 track, {}", format_time(total)),
            _ => format!("{n} tracks, {}", format_time(total)),
        };
        ui.painter().text(head.right_center(), Align2::RIGHT_CENTER, summary, medium(12.0), p.label);
        ui.add_space(8.0);

        // Rows reach a little into the margin, so the selection tint frames the text.
        let rows = ui.available_rect_before_wrap().expand2(vec2(ROW_INSET, 0.0));
        let background = ui.allocate_rect(rows.shrink2(vec2(ROW_INSET, 0.0)), Sense::click());
        let mut action = None;
        background.context_menu(|ui| {
            if ui.button("Load Tracks…").clicked() {
                action = Some(ListAction::Load);
                ui.close();
            }
            if ui.add_enabled(n > 0, egui::Button::new("Clear Playlist")).clicked() {
                action = Some(ListAction::Clear);
                ui.close();
            }
        });

        let mut list = ui.new_child(UiBuilder::new().max_rect(rows).layout(Layout::top_down(Align::Min)));
        list.set_clip_rect(rows.intersect(ui.clip_rect().expand2(vec2(ROW_INSET, 0.0))));
        let position = deck.position();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(&mut list, ROW_H, n, |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for i in range {
                let row = TrackRow {
                    index: i,
                    track: &deck.playlist.tracks[i],
                    current: deck.playlist.current == Some(i),
                    selected: self.selected == Some(i),
                    transport: deck.transport,
                    position,
                };
                if let Some(a) = track_row(ui, &row) {
                    action = Some(a);
                }
            }
        });

        match action {
            Some(ListAction::Select(i)) => self.selected = Some(i),
            Some(ListAction::Play(i)) => {
                self.selected = Some(i);
                deck.play(i);
            }
            Some(ListAction::Remove(i)) => {
                deck.remove(i);
                self.selected = None;
            }
            Some(ListAction::Reveal(i)) => {
                let _ = std::process::Command::new("open").arg("-R").arg(&deck.playlist.tracks[i].path).spawn();
            }
            Some(ListAction::Load) => self.open_dialog(deck, ui.ctx()),
            Some(ListAction::Clear) => self.clear(deck),
            None => {}
        }
    }
}

/// Advances to the next track when one ends, even while the window is hidden and not repainting.
fn spawn_watcher(deck: &Arc<Mutex<Deck>>, ctx: egui::Context) {
    let weak = Arc::downgrade(deck);
    std::thread::spawn(move || {
        while let Some(deck) = weak.upgrade() {
            let (changed, playing) = deck.lock().map_or((false, false), |mut d| (d.tick(), d.transport == Transport::Playing));
            drop(deck);
            if changed {
                ctx.request_repaint();
            }

            // Only a playing track can run out, so the thread naps while the deck is idle.
            std::thread::sleep(Duration::from_millis(if playing { 40 } else { 500 }));
        }
    });
}

fn read_meta_in_background(paths: Vec<PathBuf>, tx: Sender<(PathBuf, Meta)>, ctx: egui::Context) {
    if paths.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        for (k, path) in paths.into_iter().enumerate() {
            let meta = read_meta(&path);
            if tx.send((path, meta)).is_err() {
                return;
            }
            if k % 16 == 0 {
                ctx.request_repaint();
            }
        }
        ctx.request_repaint();
    });
}

/// Redrawing is most of the app's CPU while playing; 20 fps keeps the meters smooth.
const FRAME: Duration = Duration::from_millis(50);

const MARGIN: f32 = 24.0;

/// Track numbers hang in this gutter, in the header and in the list.
const GUTTER: f32 = 34.0;
const TITLE: f32 = 22.0;
const WAVE_H: f32 = 72.0;

/// Space between the two columns, and between them and the waveform.
const COLUMN_GAP: f32 = 44.0;
const BLOCK_GAP: f32 = 28.0;
const BAR: f32 = 2.0;
const BAR_GAP: f32 = 1.5;
const ICON_BOX: egui::Vec2 = vec2(40.0, 34.0);
const KNOB_R: f32 = 19.0;
const KNOB_ROW_H: f32 = 90.0;

/// Drag distance for one step of a stepped knob.
const STEP_TRAVEL: f32 = 24.0;
const ROW_H: f32 = 28.0;
const ROW_INSET: f32 = 8.0;

#[derive(Clone, Copy)]
enum Icon {
    Prev,
    Play,
    Pause,
    Stop,
    Next,
    Folder,
}

/// Solid glyphs on a 20 pt square.
fn glyph(painter: &Painter, icon: Icon, c: Pos2, color: Color32) {
    let tri = |dir: f32, at: Pos2, w: f32, h: f32| {
        let pts = vec![at + vec2(-dir * w / 2.0, -h / 2.0), at + vec2(dir * w / 2.0, 0.0), at + vec2(-dir * w / 2.0, h / 2.0)];
        Shape::convex_polygon(pts, color, Stroke::NONE)
    };
    let block = |at: Pos2, w: f32, h: f32| Shape::rect_filled(Rect::from_center_size(at, vec2(w, h)), 0.0, color);
    let shapes = match icon {
        Icon::Play => vec![tri(1.0, c + vec2(1.5, 0.0), 15.0, 17.0)],
        Icon::Pause => vec![block(c - vec2(4.5, 0.0), 4.5, 16.0), block(c + vec2(4.5, 0.0), 4.5, 16.0)],
        Icon::Stop => vec![block(c, 14.0, 14.0)],
        Icon::Prev => vec![block(c - vec2(7.0, 0.0), 3.0, 14.0), tri(-1.0, c + vec2(2.0, 0.0), 12.0, 14.0)],
        Icon::Next => vec![tri(1.0, c - vec2(2.0, 0.0), 12.0, 14.0), block(c + vec2(7.0, 0.0), 3.0, 14.0)],
        Icon::Folder => vec![block(c + vec2(0.0, 1.5), 20.0, 13.0), block(c + vec2(-6.0, -6.0), 8.0, 4.0)],
    };
    painter.extend(shapes);
}

/// An icon button. `mark` underlines it in that color to show the deck's state.
fn icon_button(ui: &mut egui::Ui, icon: Icon, mark: Option<Color32>, enabled: bool, tip: &str) -> bool {
    let p = palette(ui);
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(ICON_BOX, sense);
    let name = tip.split(" (").next().unwrap_or(tip);
    response.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, name));
    let painter = ui.painter();
    let c = pos2(rect.center().x, rect.top() + 12.0);
    let color = match (enabled, mark) {
        (false, _) => p.dim,
        (true, Some(m)) if m == LIVE => LIVE,
        _ => p.ink,
    };
    glyph(painter, icon, c, color);
    let line = Rect::from_center_size(pos2(c.x, rect.bottom() - 3.0), vec2(20.0, 2.5));
    let underline = match mark {
        Some(m) => Some(m),
        None if enabled && response.hovered() => Some(p.dim),
        None => None,
    };
    if let Some(color) = underline {
        painter.rect_filled(line, 0.0, color);
    }
    if response.has_focus() {
        painter.rect_stroke(rect.shrink(1.0), 4.0, Stroke::new(1.5, p.ink), egui::StrokeKind::Inside);
    }
    let clicked = enabled && response.clicked();
    response.on_hover_text(tip);
    clicked
}

fn knob_hit(ui: &mut egui::Ui, c: Pos2, id: &str) -> egui::Response {
    let rect = Rect::from_center_size(c, vec2(2.0 * KNOB_R + 16.0, 2.0 * KNOB_R + 16.0));
    ui.interact(rect, ui.scope_id().with(("knob", id)), Sense::click_and_drag())
}

/// Arrow-key steps on a focused control: up or right is +1.
fn arrows(ui: &egui::Ui, r: &egui::Response) -> i32 {
    if !r.has_focus() {
        return 0;
    }
    ui.input(|i| {
        let up = i.key_pressed(Key::ArrowUp) || i.key_pressed(Key::ArrowRight);
        let down = i.key_pressed(Key::ArrowDown) || i.key_pressed(Key::ArrowLeft);
        up as i32 - down as i32
    })
}

/// A ring with an orange arc up to `t`, an ink pointer, the value in bold under it and the name under that.
fn knob(ui: &egui::Ui, c: Pos2, t: f32, value: &str, name: &str, focused: bool) {
    let p = palette(ui);
    let painter = ui.painter();
    let start = 0.75 * std::f32::consts::PI;
    let sweep = 1.5 * std::f32::consts::PI;
    let arc = |from: f32, to: f32| -> Vec<Pos2> {
        let steps = ((to - from) * 24.0).ceil().max(2.0) as usize;
        (0..=steps).map(|k| from + (to - from) * k as f32 / steps as f32).map(|a| c + KNOB_R * vec2(a.cos(), a.sin())).collect()
    };
    painter.add(Shape::line(arc(start, start + sweep), Stroke::new(5.0, p.dim)));
    if t > 0.0 {
        painter.add(Shape::line(arc(start, start + sweep * t), Stroke::new(5.0, LIVE)));
    }
    let a = start + sweep * t;
    let dir = vec2(a.cos(), a.sin());
    painter.line_segment([c - dir * 2.0, c + dir * (KNOB_R - 7.0)], Stroke::new(4.0, p.ink));
    if focused {
        painter.circle_stroke(c, KNOB_R + 7.0, Stroke::new(1.5, p.ink));
    }
    painter.text(pos2(c.x, c.y + KNOB_R + 16.0), Align2::CENTER_CENTER, value, bold(15.0), p.ink);
    let g = painter.layout_job(tracked(name, bold(9.0), 1.2, p.label));
    painter.galley(pos2(c.x - g.size().x / 2.0, c.y + KNOB_R + 30.0), g, p.label);
}

/// A VU meter in the knobs' terms: a grey arc, an orange arc up to the level, an ink needle on a
/// pivot, and a sparse scale. The music turns it.
fn vu_dial(ui: &egui::Ui, r: Rect, needle: f32, name: &str) {
    let p = palette(ui);
    let painter = ui.painter();
    let half: f32 = 0.87;

    // Sized to leave room for the scale labels at the sides, then centered in the space.
    let (above, below) = (32.0, 30.0);
    let radius = ((r.width() / 2.0 - 44.0) / half.sin()).min(r.height() - above - below).max(20.0);
    let pivot = pos2(r.center().x, r.center().y + (radius + above - below) / 2.0);
    let name_y = pivot.y + below;
    let at = |pos: f32, rad: f32| {
        let a = -half + 2.0 * half * pos;
        pivot + rad * vec2(a.sin(), -a.cos())
    };
    let arc = |from: f32, to: f32| -> Vec<Pos2> {
        let steps = ((to - from) * 48.0).ceil().max(2.0) as usize;
        (0..=steps).map(|k| at(from + (to - from) * k as f32 / steps as f32, radius)).collect()
    };

    painter.add(Shape::line(arc(0.0, 1.0), Stroke::new(5.0, p.dim)));
    if needle > 0.002 {
        painter.add(Shape::line(arc(0.0, needle), Stroke::new(5.0, LIVE)));
    }
    for (vu, label) in [(-20.0, "−20"), (-10.0, "−10"), (-7.0, ""), (-5.0, "−5"), (-3.0, ""), (-2.0, ""), (-1.0, ""), (0.0, "0"), (1.0, ""), (2.0, ""), (3.0, "+3")] {
        let pos = vu_pos(vu);
        let color = if vu >= 0.0 { p.ink } else { p.label };
        let long = if label.is_empty() { 4.0 } else { 7.0 };
        painter.line_segment([at(pos, radius + 6.0), at(pos, radius + 6.0 + long)], Stroke::new(1.5, color));
        if !label.is_empty() {
            painter.text(at(pos, radius + 22.0), Align2::CENTER_CENTER, label, bold(9.0), color);
        }
    }

    painter.line_segment([pivot, at(needle, radius - 8.0)], Stroke::new(3.0, p.ink));
    painter.circle_filled(pivot, 4.5, p.ink);
    let g = painter.layout_job(tracked(name, bold(9.0), 1.2, p.label));
    painter.galley(pos2(r.center().x - g.size().x / 2.0, name_y - g.size().y), g, p.label);
}

struct TrackRow<'a> {
    index: usize,
    track: &'a mp3deck::Track,
    current: bool,
    selected: bool,
    transport: Transport,
    position: Duration,
}

enum ListAction {
    Select(usize),
    Play(usize),
    Remove(usize),
    Reveal(usize),
    Load,
    Clear,
}

/// One playlist row.
fn track_row(ui: &mut egui::Ui, row: &TrackRow<'_>) -> Option<ListAction> {
    let p = palette(ui);
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
    let painter = ui.painter_at(rect);
    let cy = rect.center().y;
    let t = row.track;
    let i = row.index;

    if row.selected {
        painter.rect_filled(rect, 4.0, p.select);
    } else if response.hovered() {
        painter.rect_filled(rect, 4.0, p.hover);
    }

    let x = rect.left() + ROW_INSET;
    let number = format!("{:02}", i + 1);
    if row.current {
        let color = if row.transport == Transport::Playing { LIVE } else { p.ink };
        painter.text(pos2(x, cy), Align2::LEFT_CENTER, number, bold(12.0), color);
    } else {
        painter.text(pos2(x, cy), Align2::LEFT_CENTER, number, medium(12.0), p.label);
    }

    let right = rect.right() - ROW_INSET;
    let running = row.current && row.transport != Transport::Stopped;
    let (status, color) = match (t.failed, t.duration) {
        (true, _) => ("Can't play".to_owned(), LIVE),
        (false, Some(d)) if running => (format!("−{}", format_time(d.saturating_sub(row.position))), p.ink),
        (false, Some(d)) => (format_time(d), p.label),
        (false, None) => ("–:––".to_owned(), p.label),
    };
    let status = painter.layout_no_wrap(status, medium(12.0), color);
    let text_left = x + GUTTER;
    let text_width = right - status.size().x - 14.0 - text_left;
    painter.galley(pos2(right - status.size().x, cy - status.size().y / 2.0), status, color);

    let title_color = if t.failed { p.label } else { p.ink };
    let mut job = LayoutJob::default();
    job.append(&t.title, 0.0, TextFormat::simple(regular(14.0), title_color));
    if let Some(a) = &t.artist {
        job.append(a, 8.0, TextFormat::simple(regular(14.0), p.label));
    }
    let g = painter.layout_job(fitted(job, text_width, 1));
    painter.galley(pos2(text_left, cy - g.size().y / 2.0), g, title_color);

    let mut action = None;
    if response.double_clicked() && !t.failed {
        action = Some(ListAction::Play(i));
    } else if response.clicked() {
        action = Some(ListAction::Select(i));
    }
    response.on_hover_text(t.path.display().to_string()).context_menu(|ui| {
        if ui.add_enabled(!t.failed, egui::Button::new("Play")).clicked() {
            action = Some(ListAction::Play(i));
            ui.close();
        }
        if ui.button("Show in Finder").clicked() {
            action = Some(ListAction::Reveal(i));
            ui.close();
        }
        ui.separator();
        if ui.button("Remove from Playlist").clicked() {
            action = Some(ListAction::Remove(i));
            ui.close();
        }
        if ui.button("Clear Playlist").clicked() {
            action = Some(ListAction::Clear);
            ui.close();
        }
    });
    action
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if std::mem::take(&mut self.reset_size) {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(WIDTH, HEIGHT)));
        }
        let p = palette(ui);
        let shared = self.deck.clone();
        let mut deck = shared.lock().unwrap_or_else(|e| e.into_inner());

        while let Ok((path, meta)) = self.meta_rx.try_recv() {
            let current = deck.playlist.current;
            let mut current_duration = None;
            for (i, t) in deck.playlist.tracks.iter_mut().enumerate().filter(|(_, t)| t.path == path) {
                t.apply(&meta);
                if current == Some(i) {
                    current_duration = t.duration;
                }
            }
            if deck.duration.is_none() {
                deck.duration = current_duration;
            }
        }

        if !self.launch_paths.is_empty() {
            let paths = std::mem::take(&mut self.launch_paths);
            self.add(&paths, &mut deck, &ctx);
        }
        let (hovering, dropped) = ctx.input(|i| {
            let dropped: Vec<PathBuf> = i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect();
            (!i.raw.hovered_files.is_empty(), dropped)
        });
        if !dropped.is_empty() {
            self.add(&dropped, &mut deck, &ctx);
        }
        self.keys(&mut deck, &ctx);

        let live = deck.transport == Transport::Playing;
        let moving = self.meters.update(deck.levels.get(), live);
        let current = deck.playlist.current.and_then(|c| deck.playlist.tracks.get(c)).map(|t| t.path.clone());
        let wave = current.map(|path| self.wave(&path, &ctx));

        // Left margin clears the traffic lights; the 32 pt strip centers on them.
        let m = MARGIN as i8;
        let strip = egui::Frame::new().fill(p.ground).inner_margin(egui::Margin { left: 80, right: m, top: 4, bottom: 4 });
        egui::Panel::top("title").frame(strip).exact_size(32.0).show_separator_line(false).show(ui, |ui| self.title_strip(ui, &deck));

        let body = egui::Frame::new().fill(p.ground).inner_margin(egui::Margin { left: m, right: m, top: 14, bottom: 12 });
        egui::CentralPanel::default().frame(body).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;

            // Two columns over a full-width waveform: the track, its dials and the transport on the
            // left; the playlist and the knobs on the right.
            let full = ui.max_rect();
            let bottom = Rect::from_min_max(pos2(full.left(), full.bottom() - WAVE_H - 24.0), full.max);
            let top = Rect::from_min_max(full.min, pos2(full.right(), bottom.top() - BLOCK_GAP));
            let column = (top.width() - COLUMN_GAP) / 2.0;
            let left = Rect::from_min_size(top.min, vec2(column, top.height()));
            let right = Rect::from_min_max(pos2(left.right() + COLUMN_GAP, top.top()), top.max);
            let region = |ui: &mut egui::Ui, rect: Rect| {
                let mut child = ui.new_child(UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Min)));
                child.spacing_mut().item_spacing.y = 0.0;
                child
            };

            let mut l = region(ui, left);
            self.header(&mut l, &deck, hovering);
            let keys = Rect::from_min_max(pos2(left.left(), left.bottom() - ICON_BOX.y), left.max);
            let dials = Rect::from_min_max(pos2(left.left(), l.cursor().top() + 20.0), pos2(left.right(), keys.top() - 16.0));
            self.vu_meters(&mut region(ui, dials));
            self.transport(&mut region(ui, keys), &mut deck);

            let knobs = Rect::from_min_max(pos2(right.left(), right.bottom() - KNOB_ROW_H), right.max);
            self.playlist(&mut region(ui, right.with_max_y(knobs.top() - 20.0)), &mut deck);
            self.knobs(&mut region(ui, knobs), &mut deck);

            self.waveform(&mut region(ui, bottom), &mut deck, wave.as_deref());
        });

        let reading = wave.is_some_and(|w| w.state.load(Ordering::Relaxed) == READING);
        if live || moving {
            ctx.request_repaint_after(FRAME);
        } else if reading {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        let deck = self.deck.lock().unwrap_or_else(|e| e.into_inner());
        let prefs = Prefs {
            volume: self.volume,
            repeat: deck.playlist.repeat,
            shuffle: self.shuffle,
            playlist: deck.playlist.tracks.iter().map(|t| t.path.clone()).collect(),
            current: deck.playlist.current,
            position: deck.position().as_secs_f64(),
            layout: LAYOUT,
        };
        eframe::set_value(storage, eframe::APP_KEY, &prefs);
    }
}
