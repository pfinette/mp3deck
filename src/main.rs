use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::text::{LayoutJob, TextFormat, TextWrapping};
use eframe::egui::{
    self, Align, Align2, Color32, FontData, FontDefinitions, FontFamily, FontId, Key, Layout, Modifiers, Pos2, Rect, Role,
    Sense, Stroke, StrokeKind, Theme, WidgetInfo, pos2, vec2,
};
use mp3deck::audio::{Deck, Transport, open_output};
use mp3deck::{Meta, Repeat, collect_mp3s, format_time, read_meta};
use serde::{Deserialize, Serialize};

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("MP3deck")
            .with_inner_size([860.0, 600.0])
            .with_min_inner_size([760.0, 480.0])
            .with_drag_and_drop(true)
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false),
        ..Default::default()
    };
    eframe::run_native("MP3deck", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

/// The one signal color: the music's level.
const ACCENT: Color32 = Color32::from_rgb(0xFF, 0xB0, 0x20);

/// A cream faceplate with recessed screens, after a tape deck.
struct Palette {
    face: Color32,
    groove: Color32,
    ink: Color32,
    dim: Color32,
    screen: Color32,
    screen_edge: Color32,
    screen_text: Color32,
    screen_dim: Color32,
    seg_off: Color32,
    seg_lit: Color32,
    row_hover: Color32,
    row_selected: Color32,
    error: Color32,
}

const LIGHT: Palette = Palette {
    face: Color32::from_rgb(0xE3, 0xE0, 0xCC),
    groove: Color32::from_rgb(0xC9, 0xC5, 0xAE),
    ink: Color32::from_rgb(0x1B, 0x1B, 0x1A),

    // Darker than the skill's default dim, which fails 4.5:1 on cream.
    dim: Color32::from_rgb(0x61, 0x5E, 0x57),
    screen: Color32::from_rgb(0x1A, 0x1B, 0x1D),
    screen_edge: Color32::from_rgb(0x9C, 0x98, 0x90),
    screen_text: Color32::from_rgb(0xE6, 0xE4, 0xDE),
    screen_dim: Color32::from_rgb(0x82, 0x84, 0x88),
    seg_off: Color32::from_rgb(0x2C, 0x2E, 0x32),
    seg_lit: Color32::from_rgb(0x9A, 0x9D, 0xA3),
    row_hover: Color32::from_rgb(0x22, 0x24, 0x28),
    row_selected: Color32::from_rgb(0x2A, 0x2C, 0x31),
    error: Color32::from_rgb(0xFF, 0x6B, 0x5B),
};

const DARK: Palette = Palette {
    face: Color32::from_rgb(0x2B, 0x2B, 0x2A),
    groove: Color32::from_rgb(0x44, 0x44, 0x42),
    ink: Color32::from_rgb(0xEC, 0xEA, 0xE4),
    dim: Color32::from_rgb(0x9A, 0x97, 0x8F),
    screen: Color32::from_rgb(0x17, 0x18, 0x1A),
    screen_edge: Color32::from_rgb(0x0E, 0x0E, 0x0F),
    ..LIGHT
};

fn palette(ui: &egui::Ui) -> &'static Palette {
    if ui.visuals().dark_mode { &DARK } else { &LIGHT }
}

/// DIN for printed labels, SF Mono for screens, SF Pro for menus and tooltips.
fn din(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("din".into()))
}

fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

/// Text with extra letter spacing, as printed on a faceplate.
fn tracked(text: impl Into<String>, font: FontId, spacing: f32, color: Color32) -> LayoutJob {
    LayoutJob::single_section(text.into(), TextFormat { font_id: font, extra_letter_spacing: spacing, color, ..Default::default() })
}

/// One line of text cut to `width` with an ellipsis.
fn one_line(mut job: LayoutJob, width: f32) -> LayoutJob {
    job.wrap = TextWrapping { max_width: width.max(10.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    job
}

fn install_style(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();

    // Each family falls back to egui's own fonts when a system font is missing.
    let fallback = fonts.families[&FontFamily::Proportional].clone();
    fonts.families.insert(FontFamily::Name("din".into()), fallback);
    for (name, path, family) in [
        ("SF Pro", "/System/Library/Fonts/SFNS.ttf", FontFamily::Proportional),
        ("SF Mono", "/System/Library/Fonts/SFNSMono.ttf", FontFamily::Monospace),
        ("DIN", "/System/Library/Fonts/Supplemental/DIN Alternate Bold.ttf", FontFamily::Name("din".into())),
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert(name.into(), Arc::new(FontData::from_owned(bytes)));
            fonts.families.entry(family).or_default().insert(0, name.into());
        }
    }
    ctx.set_fonts(fonts);

    for (theme, p) in [(Theme::Dark, &DARK), (Theme::Light, &LIGHT)] {
        ctx.style_mut_of(theme, |style| {
            style.spacing.scroll = egui::style::ScrollStyle::floating();
            style.interaction.tooltip_delay = 0.6;
            style.interaction.selectable_labels = false;
            let v = &mut style.visuals;
            v.panel_fill = p.face;
            v.window_fill = p.face;
            v.selection.bg_fill = ACCENT;
            v.selection.stroke.color = p.ink;
            v.widgets.noninteractive.bg_stroke.color = p.groove;
            v.widgets.noninteractive.fg_stroke.color = p.ink;
            v.widgets.inactive.fg_stroke.color = p.ink;
        });
    }
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Prefs {
    volume: f64,
    repeat: Repeat,
    shuffle: bool,
    playlist: Vec<PathBuf>,
    current: Option<usize>,
}

const DEFAULT_VOLUME: f64 = 80.0;

impl Default for Prefs {
    fn default() -> Self {
        Self { volume: DEFAULT_VOLUME, repeat: Repeat::Off, shuffle: false, playlist: Vec::new(), current: None }
    }
}

/// Meter readings as shown, in dBFS. They read the music before the volume knob, as a deck's
/// meters read the tape, and rise at once and fall slowly, like a VU meter.
#[derive(Default)]
struct Meters {
    rms: [f32; 2],
    peak: [f32; 2],
    peak_age: [f32; 2],
    last: Option<Instant>,
}

const FLOOR_DB: f32 = -48.0;
const FALL_DB_PER_S: f32 = 26.0;
const PEAK_HOLD_S: f32 = 1.2;

fn to_db(amplitude: f32) -> f32 {
    if amplitude <= 1e-6 { FLOOR_DB } else { (20.0 * amplitude.log10()).clamp(FLOOR_DB, 0.0) }
}

impl Meters {
    fn new() -> Self {
        Self { rms: [FLOOR_DB; 2], peak: [FLOOR_DB; 2], ..Default::default() }
    }

    /// Moves toward the latest levels. Returns true while anything is still moving.
    fn update(&mut self, levels: [(f32, f32); 2], live: bool) -> bool {
        let now = Instant::now();
        let dt = self.last.map_or(0.0, |t| (now - t).as_secs_f32().min(0.1));
        self.last = Some(now);
        for (ch, (rms, peak)) in levels.into_iter().enumerate() {
            let (rms, peak) = if live { (to_db(rms), to_db(peak)) } else { (FLOOR_DB, FLOOR_DB) };
            self.rms[ch] = if rms > self.rms[ch] { rms } else { (self.rms[ch] - FALL_DB_PER_S * dt).max(rms) };
            if peak >= self.peak[ch] {
                self.peak[ch] = peak;
                self.peak_age[ch] = 0.0;
            } else {
                self.peak_age[ch] += dt;
                if self.peak_age[ch] > PEAK_HOLD_S {
                    self.peak[ch] = (self.peak[ch] - FALL_DB_PER_S * dt).max(FLOOR_DB);
                }
            }
        }
        live || self.rms.iter().chain(&self.peak).any(|&db| db > FLOOR_DB)
    }
}

struct App {
    deck: Arc<Mutex<Deck>>,

    /// Keeps the output device open. `None` when there is none.
    _output: Option<rodio::MixerDeviceSink>,

    meta_tx: Sender<(PathBuf, Meta)>,
    meta_rx: Receiver<(PathBuf, Meta)>,
    selected: Option<usize>,
    meters: Meters,

    /// Where the seek bar is being dragged to, as a fraction of the track.
    scrub: Option<f32>,
    volume: f64,
    shuffle: bool,

    /// Paths given on the command line, added on the first frame.
    launch_paths: Vec<PathBuf>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_style(&cc.egui_ctx);
        let prefs: Prefs = cc.storage.and_then(|s| eframe::get_value(s, eframe::APP_KEY)).unwrap_or_default();

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
            meters: Meters::new(),
            scrub: None,
            volume: prefs.volume,
            shuffle: prefs.shuffle,
            launch_paths: std::env::args_os().skip(1).map(PathBuf::from).collect(),
        }
    }

    /// Adds MP3s from files and folders. Starts playing when the deck was idle.
    fn add(&mut self, paths: &[PathBuf], deck: &mut Deck, ctx: &egui::Context) {
        let files = collect_mp3s(paths);
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
        if let Some(files) = rfd::FileDialog::new().set_title("Add MP3s").add_filter("MP3", &["mp3", "MP3"]).pick_files() {
            self.add(&files, deck, ctx);
        }
    }

    fn keys(&mut self, deck: &mut Deck, ctx: &egui::Context) {
        // Consumed first, so Space never also presses a focused button.
        let pressed = |key: Key, mods: Modifiers| ctx.input_mut(|i| i.consume_key(mods, key));
        let n = deck.playlist.tracks.len();

        if pressed(Key::Space, Modifiers::NONE) {
            match (deck.transport, self.selected) {
                (Transport::Stopped, Some(i)) if i < n => deck.play(i),
                _ => deck.toggle(),
            }
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

    /// Wordmark and status display, in the title bar beside the traffic lights.
    fn title_strip(&self, ui: &mut egui::Ui, deck: &Deck) {
        let p = palette(ui);

        // The strip is the title bar: drag to move, double-click to zoom.
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
            ("NO AUDIO OUTPUT".to_owned(), p.error)
        } else if deck.error.is_some() && deck.transport == Transport::Stopped {
            ("CAN'T PLAY".to_owned(), p.error)
        } else {
            let s = match (deck.transport, n) {
                (_, 0) => "READY".to_owned(),
                (Transport::Playing, _) => format!("PLAYING {at}/{n}"),
                (Transport::Paused, _) => format!("PAUSED {at}/{n}"),
                (Transport::Stopped, _) => format!("STOPPED {at}/{n}"),
            };
            (s, p.screen_text)
        };

        ui.horizontal_centered(|ui| {
            ui.label(tracked("MP3DECK", din(15.0), 3.0, p.ink));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| lcd_text(ui, &status, color, p));
        });
    }

    /// The upper screen: time, level meters, title and the seek bar.
    fn display(&mut self, ui: &mut egui::Ui, deck: &mut Deck) {
        let p = palette(ui);
        screen_frame(p, false).show(ui, |ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 112.0), Sense::hover());
            let painter = ui.painter_at(rect.expand(4.0));
            let track = deck.playlist.current.and_then(|c| deck.playlist.tracks.get(c)).cloned();
            let duration = deck.duration.filter(|d| !d.is_zero());
            let position = deck.position();

            // Time: elapsed large, then remaining and total.
            let shown = match (self.scrub, duration) {
                (Some(f), Some(d)) => d.mul_f32(f),
                _ => position,
            };
            let (time, time_color) = match track {
                Some(_) => (format_time(shown), p.screen_text),
                None => ("0:00".to_owned(), p.screen_dim),
            };
            painter.text(rect.left_top(), Align2::LEFT_TOP, time, mono(32.0), time_color);
            if let Some(d) = duration {
                let rest = format!("−{}  /  {}", format_time(d.saturating_sub(shown)), format_time(d));
                painter.text(rect.left_top() + vec2(2.0, 42.0), Align2::LEFT_TOP, rest, mono(11.0), p.screen_dim);
            }

            // Level meters, left and right.
            let meter_left = rect.left() + 190.0;
            let meter = Rect::from_min_max(pos2(meter_left + 18.0, rect.top() + 4.0), pos2(rect.right(), rect.top() + 44.0));
            for (ch, label) in ["L", "R"].into_iter().enumerate() {
                let y = meter.top() + ch as f32 * 18.0;
                painter.text(pos2(meter_left, y + 5.0), Align2::LEFT_CENTER, label, mono(10.0), p.screen_dim);
                let bar = Rect::from_min_size(pos2(meter.left(), y), vec2(meter.width(), 10.0));
                level_meter(&painter, bar, self.meters.rms[ch], self.meters.peak[ch], p);
            }
            for db in [-40, -30, -20, -10, -6, -3, 0] {
                let x = meter.left() + meter.width() * (db as f32 - FLOOR_DB) / -FLOOR_DB;
                let align = if db == 0 { Align2::RIGHT_TOP } else { Align2::CENTER_TOP };
                painter.text(pos2(x, meter.top() + 31.0), align, db.to_string(), mono(9.0), p.screen_dim);
            }

            // Title and artist.
            let title_y = rect.top() + 70.0;
            if let Some(t) = &track {
                let mut job = LayoutJob::default();
                job.append(&t.title, 0.0, TextFormat::simple(mono(13.0), p.screen_text));
                if let Some(a) = &t.artist {
                    job.append(a, 14.0, TextFormat::simple(mono(13.0), p.screen_dim));
                }
                let g = painter.layout_job(one_line(job, rect.width()));
                painter.galley(pos2(rect.left(), title_y), g, p.screen_text);
            } else {
                painter.text(pos2(rect.left(), title_y), Align2::LEFT_TOP, "NO TRACK", mono(13.0), p.screen_dim);
            }

            // Seek bar: drag or click to jump.
            let bar = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 9.0), pos2(rect.right(), rect.bottom()));
            let hit = ui.interact(bar.expand2(vec2(0.0, 6.0)), ui.scope_id().with("seek"), Sense::click_and_drag());
            let fraction_at = |pos: Pos2| ((pos.x - bar.left()) / bar.width()).clamp(0.0, 1.0);
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
            let played = match (self.scrub, duration) {
                (Some(f), _) => f,
                (None, Some(d)) => (position.as_secs_f32() / d.as_secs_f32()).clamp(0.0, 1.0),
                _ => 0.0,
            };
            seek_bar(&painter, bar, played, track.is_some(), p);
            let tip = duration.zip(hit.hover_pos()).map(|(d, pos)| format_time(d.mul_f32(fraction_at(pos))));
            let hit = match tip {
                Some(t) => hit.on_hover_text_at_pointer(t),
                None => hit,
            };
            hit.widget_info(|| WidgetInfo::slider(duration.is_some(), (played * 100.0).round() as f64, "Position"));
        });
    }

    /// The lower screen: the playlist as tracks, or what to do when it's empty.
    fn playlist(&mut self, ui: &mut egui::Ui, deck: &mut Deck, hovering: bool) {
        let p = palette(ui);
        screen_frame(p, hovering).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            let r = ui.max_rect();
            if deck.playlist.tracks.is_empty() {
                let title = if hovering { "RELEASE TO LOAD" } else { "DROP MP3S OR FOLDERS" };
                let color = if hovering { ACCENT } else { p.screen_text };
                let g = ui.painter().layout_job(tracked(title, mono(15.0), 2.0, color));
                ui.painter().galley(r.center() - vec2(g.size().x / 2.0, 22.0), g, color);
                let hint = if deck.has_output() { "Or press ⌘O to choose files. Space plays and pauses." } else { "No audio output was found." };
                ui.painter().text(r.center() + vec2(0.0, 14.0), Align2::CENTER_CENTER, hint, mono(11.0), p.screen_dim);
                return;
            }

            let longest = deck.playlist.tracks.iter().filter_map(|t| t.duration).max().unwrap_or(Duration::from_secs(1));
            let position = deck.position();
            let mut action = None;
            let n = deck.playlist.tracks.len();
            egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, ROW_HEIGHT, n, |ui, range| {
                for i in range {
                    let row = TrackRow {
                        index: i,
                        track: &deck.playlist.tracks[i],
                        current: deck.playlist.current == Some(i),
                        selected: self.selected == Some(i),
                        transport: deck.transport,
                        position,
                        longest,
                    };
                    if let Some(a) = track_row(ui, &row) {
                        action = Some((i, a));
                    }
                }
            });

            match action {
                Some((i, RowAction::Select)) => self.selected = Some(i),
                Some((i, RowAction::Play)) => {
                    self.selected = Some(i);
                    deck.play(i);
                }
                Some((i, RowAction::Remove)) => {
                    deck.remove(i);
                    self.selected = None;
                }
                Some((i, RowAction::Reveal)) => {
                    let _ = std::process::Command::new("open").arg("-R").arg(&deck.playlist.tracks[i].path).spawn();
                }
                None => {}
            }

            if hovering {
                let veil = Color32::from_rgba_unmultiplied(p.screen.r(), p.screen.g(), p.screen.b(), 220);
                ui.painter().rect_filled(r, 4.0, veil);
                let g = ui.painter().layout_job(tracked("RELEASE TO ADD", mono(15.0), 2.0, ACCENT));
                ui.painter().galley(r.center() - g.size() / 2.0, g, ACCENT);
            }
        });
    }

    /// Hardware controls along the bottom of the faceplate.
    fn control_row(&mut self, ui: &mut egui::Ui, deck: &mut Deck) {
        let has_tracks = !deck.playlist.tracks.is_empty();
        let mut load = false;
        let mut clear = false;

        ui.horizontal(|ui| {
            control(ui, TRANSPORT_WIDTH, "TRANSPORT", |ui| {
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = KEY_GAP;
                    if transport_key(ui, KeyIcon::Prev, false, has_tracks, "Previous (⌘←)") {
                        deck.prev();
                    }
                    let playing = deck.transport == Transport::Playing;
                    let icon = if playing { KeyIcon::Pause } else { KeyIcon::Play };
                    if transport_key(ui, icon, playing, has_tracks, if playing { "Pause (Space)" } else { "Play (Space)" }) {
                        match (deck.transport, self.selected) {
                            (Transport::Stopped, Some(i)) if i < deck.playlist.tracks.len() => deck.play(i),
                            _ => deck.toggle(),
                        }
                    }
                    if transport_key(ui, KeyIcon::Stop, false, deck.transport != Transport::Stopped, "Stop") {
                        deck.stop();
                    }
                    if transport_key(ui, KeyIcon::Next, false, has_tracks, "Next (⌘→)") {
                        deck.next();
                    }
                });
            });
            ui.add_space(24.0);
            control(ui, 3.0 * REPEAT_SEGMENT, "REPEAT", |ui| {
                ui.add_space(15.0);
                let mut repeat = deck.playlist.repeat;
                repeat_switch(ui, &mut repeat);
                deck.set_repeat(repeat);
            });
            ui.add_space(24.0);
            control(ui, 92.0, "SHUFFLE", |ui| {
                ui.add_space(17.0);
                if toggle_switch(ui, self.shuffle, "Shuffle") {
                    self.shuffle = !self.shuffle;
                    deck.playlist.set_shuffle(self.shuffle);
                }
            });
            ui.add_space(20.0);
            control(ui, 64.0, "VOLUME", |ui| {
                if knob(ui, &mut self.volume, 0.0..=100.0, DEFAULT_VOLUME) {
                    deck.set_volume((self.volume / 100.0) as f32);
                }
            });
            ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                control(ui, 64.0, "", |ui| {
                    ui.add_space(15.0);
                    clear = hardware_button(ui, "CLEAR", has_tracks, "Empty the playlist");
                });
                ui.add_space(12.0);
                control(ui, 64.0, "⌘O", |ui| {
                    ui.add_space(15.0);
                    load = hardware_button(ui, "LOAD", deck.has_output(), "Add MP3s to the playlist");
                });
            });
        });

        if load {
            self.open_dialog(deck, ui.ctx());
        }
        if clear {
            deck.clear();
            self.selected = None;
        }
    }
}

/// Advances to the next track when one ends, even while the window is hidden and not repainting.
fn spawn_watcher(deck: &Arc<Mutex<Deck>>, ctx: egui::Context) {
    let weak = Arc::downgrade(deck);
    std::thread::spawn(move || {
        while let Some(deck) = weak.upgrade() {
            let changed = deck.lock().is_ok_and(|mut d| d.tick());
            drop(deck);
            if changed {
                ctx.request_repaint();
            }
            std::thread::sleep(Duration::from_millis(40));
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

fn screen_frame(p: &Palette, hovering: bool) -> egui::Frame {
    let edge = if hovering { ACCENT } else { p.screen_edge };
    egui::Frame::new()
        .fill(p.screen)
        .corner_radius(6)
        .stroke(Stroke::new(if hovering { 2.0 } else { 1.0 }, edge))
        .inner_margin(egui::Margin::symmetric(14, 10))
}

/// LED segments.
const SEGMENT: f32 = 3.0;
const SEGMENT_GAP: f32 = 2.0;
const STEP: f32 = SEGMENT + SEGMENT_GAP;

/// A row of LED segments in `rect`; `color(k, count)` picks each segment's color.
fn segments(painter: &egui::Painter, rect: Rect, color: impl Fn(usize, usize) -> Color32) {
    let count = ((rect.width() + SEGMENT_GAP) / STEP).floor().max(1.0) as usize;
    for k in 0..count {
        let r = Rect::from_min_size(pos2(rect.left() + k as f32 * STEP, rect.top().round()), vec2(SEGMENT, rect.height()));
        painter.rect_filled(r, 0.0, color(k, count));
    }
}

/// Level in amber up to the RMS reading, with the held peak as one bright segment.
fn level_meter(painter: &egui::Painter, rect: Rect, rms_db: f32, peak_db: f32, p: &Palette) {
    let at = |db: f32, count: usize| (((db - FLOOR_DB) / -FLOOR_DB) * count as f32).round() as usize;
    segments(painter, rect, |k, count| {
        let (lit, peak) = (at(rms_db, count), at(peak_db, count));
        if k < lit {
            ACCENT
        } else if peak > 0 && k + 1 == peak {
            p.screen_text
        } else {
            p.seg_off
        }
    });
}

/// Played part lit grey, the playhead bright.
fn seek_bar(painter: &egui::Painter, rect: Rect, played: f32, active: bool, p: &Palette) {
    segments(painter, rect, |k, count| {
        let head = (played * count as f32) as usize;
        if !active {
            p.seg_off
        } else if k == head.min(count - 1) {
            p.screen_text
        } else if k < head {
            p.seg_lit
        } else {
            p.seg_off
        }
    });
}

const ROW_HEIGHT: f32 = 26.0;
const NUMBER_WIDTH: f32 = 30.0;
const LED_WIDTH: f32 = 16.0;
const TIME_WIDTH: f32 = 84.0;

struct TrackRow<'a> {
    index: usize,
    track: &'a mp3deck::Track,
    current: bool,
    selected: bool,
    transport: Transport,
    position: Duration,
    longest: Duration,
}

enum RowAction {
    Select,
    Play,
    Remove,
    Reveal,
}

/// One playlist entry: number, status LED, title and artist, a meter as long as the track, its length.
fn track_row(ui: &mut egui::Ui, row: &TrackRow<'_>) -> Option<RowAction> {
    let p = palette(ui);
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
    let painter = ui.painter_at(rect);
    let cy = rect.center().y;
    let t = row.track;

    if row.selected {
        painter.rect_filled(rect, 3.0, p.row_selected);
    } else if response.hovered() {
        painter.rect_filled(rect, 3.0, p.row_hover);
    }

    let mut x = rect.left() + 6.0;
    painter.text(pos2(x, cy), Align2::LEFT_CENTER, format!("{:02}", row.index + 1), mono(11.0), p.screen_dim);
    x += NUMBER_WIDTH;

    // The LED glows amber while this track plays.
    let led = match (row.current, row.transport) {
        (true, Transport::Playing) => ACCENT,
        (true, Transport::Paused) => p.screen_text,
        (true, Transport::Stopped) => p.seg_lit,
        _ => p.seg_off,
    };
    painter.circle_filled(pos2(x + 4.0, cy), 3.5, led);
    x += LED_WIDTH;

    let text_width = ((rect.width() - NUMBER_WIDTH - LED_WIDTH - TIME_WIDTH) * 0.55).max(140.0);
    let title_color = if t.failed { p.screen_dim } else { p.screen_text };
    let mut job = LayoutJob::default();
    job.append(&t.title, 0.0, TextFormat::simple(mono(12.0), title_color));
    if let Some(a) = &t.artist {
        job.append(a, 12.0, TextFormat::simple(mono(12.0), p.screen_dim));
    }
    let g = painter.layout_job(one_line(job, text_width - 12.0));
    painter.galley(pos2(x, cy - g.size().y / 2.0), g, title_color);
    x += text_width;

    let right = rect.right() - 6.0;
    let meter_right = right - TIME_WIDTH;
    if let Some(d) = t.duration.filter(|_| meter_right - x > 30.0) {
        let width = ((meter_right - x) * d.as_secs_f32() / row.longest.as_secs_f32().max(1.0)).max(STEP * 2.0);
        let played = if row.current && row.transport != Transport::Stopped {
            (row.position.as_secs_f32() / d.as_secs_f32().max(1.0)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        segments(&painter, Rect::from_min_size(pos2(x, cy - 5.0), vec2(width, 10.0)), |k, count| {
            if (k as f32) < played * count as f32 { p.seg_lit } else { p.seg_off }
        });
    }

    let (status, color) = match (t.failed, t.duration) {
        (true, _) => ("CAN'T PLAY".to_owned(), p.error),
        (false, Some(d)) => (format_time(d), p.screen_text),
        (false, None) => ("--:--".to_owned(), p.screen_dim),
    };
    painter.text(pos2(right, cy), Align2::RIGHT_CENTER, status, mono(12.0), color);

    let mut action = None;
    if response.double_clicked() && !t.failed {
        action = Some(RowAction::Play);
    } else if response.clicked() {
        action = Some(RowAction::Select);
    }
    response.on_hover_text(t.path.display().to_string()).context_menu(|ui| {
        if ui.add_enabled(!t.failed, egui::Button::new("Play")).clicked() {
            action = Some(RowAction::Play);
            ui.close();
        }
        if ui.button("Show in Finder").clicked() {
            action = Some(RowAction::Reveal);
            ui.close();
        }
        ui.separator();
        if ui.button("Remove from Playlist").clicked() {
            action = Some(RowAction::Remove);
            ui.close();
        }
    });
    action
}

const CONTROL_HEIGHT: f32 = 76.0;
const LABEL_HEIGHT: f32 = 14.0;
const KEY: f32 = 44.0;
const KEY_GAP: f32 = 6.0;
const TRANSPORT_WIDTH: f32 = 4.0 * KEY + 3.0 * KEY_GAP;
const REPEAT_SEGMENT: f32 = 54.0;

/// A control with its label printed under it; all labels share one baseline.
fn control(ui: &mut egui::Ui, width: f32, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    let p = palette(ui);
    ui.allocate_ui_with_layout(vec2(width, CONTROL_HEIGHT), Layout::top_down(Align::Center), |ui| {
        ui.set_min_size(vec2(width, CONTROL_HEIGHT));
        add(ui);
        if !label.is_empty() {
            let r = ui.max_rect();
            let g = ui.painter().layout_job(tracked(label, din(10.0), 1.4, p.dim));
            ui.painter().galley(pos2(r.center().x - g.size().x / 2.0, r.bottom() - LABEL_HEIGHT), g, p.dim);
        }
    });
}

#[derive(Clone, Copy)]
enum KeyIcon {
    Prev,
    Play,
    Pause,
    Stop,
    Next,
}

/// A transport key with its symbol printed on it. `lit` adds an amber lamp, as on a deck's play key.
fn transport_key(ui: &mut egui::Ui, icon: KeyIcon, lit: bool, enabled: bool, tip: &str) -> bool {
    let p = palette(ui);
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(vec2(KEY, 30.0), sense);
    let name = tip.split(" (").next().unwrap_or(tip);
    response.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, name));
    let color = if enabled { p.ink } else { p.groove };
    let painter = ui.painter();
    if response.is_pointer_button_down_on() {
        painter.rect_filled(rect, 3.0, p.groove);
    }
    painter.rect_stroke(rect, 3.0, Stroke::new(1.0, color), StrokeKind::Inside);

    let c = rect.center();
    let tri = |dir: f32, at: Pos2, w: f32, h: f32| {
        let pts = vec![at + vec2(-dir * w / 2.0, -h / 2.0), at + vec2(dir * w / 2.0, 0.0), at + vec2(-dir * w / 2.0, h / 2.0)];
        egui::Shape::convex_polygon(pts, color, Stroke::NONE)
    };
    let bar = |at: Pos2, h: f32| egui::Shape::rect_filled(Rect::from_center_size(at, vec2(2.5, h)), 0.0, color);
    let shapes = match icon {
        KeyIcon::Play => vec![tri(1.0, c + vec2(1.0, 0.0), 10.0, 12.0)],
        KeyIcon::Pause => vec![bar(c - vec2(3.0, 0.0), 11.0), bar(c + vec2(3.0, 0.0), 11.0)],
        KeyIcon::Stop => vec![egui::Shape::rect_filled(Rect::from_center_size(c, vec2(10.0, 10.0)), 0.0, color)],
        KeyIcon::Prev => vec![bar(c - vec2(6.0, 0.0), 10.0), tri(-1.0, c + vec2(1.5, 0.0), 9.0, 10.0)],
        KeyIcon::Next => vec![tri(1.0, c - vec2(1.5, 0.0), 9.0, 10.0), bar(c + vec2(6.0, 0.0), 10.0)],
    };
    painter.extend(shapes);
    if lit {
        painter.circle_filled(rect.right_top() + vec2(-6.0, 6.0), 2.5, ACCENT);
    }
    if response.has_focus() {
        painter.rect_stroke(rect.expand(2.0), 4.0, Stroke::new(1.5, ACCENT), StrokeKind::Outside);
    }
    let clicked = enabled && response.clicked();
    response.on_hover_text(tip);
    clicked
}

/// A three-position switch; the selected position lights up.
fn repeat_switch(ui: &mut egui::Ui, value: &mut Repeat) {
    let p = palette(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(3.0 * REPEAT_SEGMENT, 24.0), Sense::hover());
    for (i, option) in Repeat::ALL.into_iter().enumerate() {
        let r = Rect::from_min_size(rect.min + vec2(REPEAT_SEGMENT * i as f32, 0.0), vec2(REPEAT_SEGMENT, 24.0));
        let response = ui.interact(r, ui.scope_id().with(("repeat", i)), Sense::click());
        if response.clicked() {
            *value = option;
        }
        let on = *value == option;
        response.widget_info(|| WidgetInfo::selected(Role::RadioButton, true, on, format!("Repeat {}", option.label())));

        let painter = ui.painter();
        if on {
            painter.rect_filled(r.shrink(2.0), 2.0, ACCENT);
        } else if response.hovered() {
            painter.rect_filled(r.shrink(2.0), 2.0, p.groove);
        }
        if i > 0 {
            painter.vline(r.left(), r.y_range(), Stroke::new(1.0, p.ink));
        }
        let text_color = if on { LIGHT.ink } else { p.ink };
        let g = painter.layout_job(tracked(option.label().to_uppercase(), din(11.0), 1.0, text_color));
        painter.galley(r.center() - g.size() / 2.0, g, text_color);
        if response.has_focus() {
            painter.rect_stroke(r.shrink(1.0), 3.0, Stroke::new(2.0, ACCENT), StrokeKind::Inside);
        }
    }
    ui.painter().rect_stroke(rect, 3.0, Stroke::new(1.0, p.ink), StrokeKind::Inside);
}

/// OFF and ON printed either side of a sliding pill. Returns true when flipped.
fn toggle_switch(ui: &mut egui::Ui, on: bool, name: &str) -> bool {
    let p = palette(ui);
    let (rect, response) = ui.allocate_exact_size(vec2(92.0, 20.0), Sense::click());
    response.widget_info(|| WidgetInfo::selected(Role::Switch, true, on, name));
    let painter = ui.painter();
    let pill = Rect::from_center_size(rect.center(), vec2(36.0, 18.0));
    let word = |text: &str, x: f32, align: Align2, lit: bool| {
        painter.text(pos2(x, rect.center().y), align, text, din(10.0), if lit { p.ink } else { p.dim });
    };
    word("OFF", pill.left() - 8.0, Align2::RIGHT_CENTER, !on);
    word("ON", pill.right() + 8.0, Align2::LEFT_CENTER, on);
    painter.rect(pill, 9.0, p.groove, Stroke::new(1.0, p.ink), StrokeKind::Inside);
    let knob_x = if on { pill.right() - 9.0 } else { pill.left() + 9.0 };
    painter.circle_filled(pos2(knob_x, pill.center().y), 6.0, p.ink);
    if response.has_focus() {
        painter.rect_stroke(pill.expand(2.0), 11.0, Stroke::new(1.5, ACCENT), StrokeKind::Outside);
    }
    response.clicked()
}

/// An arc knob: drag up or down, arrow keys when focused, double-click to reset. Returns true on change.
fn knob(ui: &mut egui::Ui, value: &mut f64, range: std::ops::RangeInclusive<f64>, default: f64) -> bool {
    let p = palette(ui);
    let before = *value;
    let size = 42.0;
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), Sense::click_and_drag());
    if response.dragged() {
        *value -= response.drag_delta().y as f64 * 0.5;
    }
    if response.drag_stopped() {
        *value = value.round();
    }
    if response.double_clicked() {
        *value = default;
    }
    if response.has_focus() {
        ui.input(|i| {
            if i.key_pressed(Key::ArrowUp) || i.key_pressed(Key::ArrowRight) {
                *value += 5.0;
            }
            if i.key_pressed(Key::ArrowDown) || i.key_pressed(Key::ArrowLeft) {
                *value -= 5.0;
            }
        });
    }
    *value = value.clamp(*range.start(), *range.end());
    let t = ((*value - range.start()) / (range.end() - range.start())) as f32;
    let shown = format!("{:.0}", value.round());
    let response = response.on_hover_text("Volume. Drag up or down; double-click resets.");
    response.widget_info(|| WidgetInfo::slider(true, value.round(), "Volume"));

    // 270° sweep with the gap at the bottom, as on a hardware pot.
    let c = rect.center();
    let radius = size / 2.0 - 2.0;
    let start = 0.75 * std::f32::consts::PI;
    let sweep = 1.5 * std::f32::consts::PI;
    let arc = |from: f32, to: f32| -> Vec<Pos2> {
        (0..=32).map(|k| from + (to - from) * k as f32 / 32.0).map(|a| c + radius * vec2(a.cos(), a.sin())).collect()
    };
    let painter = ui.painter();
    painter.add(egui::Shape::line(arc(start, start + sweep), Stroke::new(3.0, p.groove)));
    if t > 0.0 {
        painter.add(egui::Shape::line(arc(start, start + sweep * t), Stroke::new(3.0, p.ink)));
    }
    let a = start + sweep * t;
    let dir = vec2(a.cos(), a.sin());
    let body = radius - 6.0;
    painter.circle_filled(c, body, p.ink);
    painter.line_segment([c + 0.3 * body * dir, c + (body - 2.5) * dir], Stroke::new(2.5, p.face));
    if response.has_focus() {
        painter.circle_stroke(c, radius + 3.0, Stroke::new(1.5, ACCENT));
    }
    ui.add_space(2.0);
    ui.label(egui::RichText::new(shown).font(mono(12.0)).color(p.ink));
    *value != before
}

/// A square hardware button with its legend printed on it.
fn hardware_button(ui: &mut egui::Ui, legend: &str, enabled: bool, tip: &str) -> bool {
    let p = palette(ui);
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(vec2(64.0, 24.0), sense);
    response.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, legend));
    let color = if enabled { p.ink } else { p.groove };
    let painter = ui.painter();
    if response.is_pointer_button_down_on() {
        painter.rect_filled(rect, 3.0, p.groove);
    }
    painter.rect_stroke(rect, 3.0, Stroke::new(1.0, color), StrokeKind::Inside);
    let g = painter.layout_job(tracked(legend, din(11.0), 1.2, color));
    painter.galley(rect.center() - g.size() / 2.0, g, color);
    if response.has_focus() {
        painter.rect_stroke(rect.expand(2.0), 4.0, Stroke::new(1.5, ACCENT), StrokeKind::Outside);
    }
    let clicked = enabled && response.clicked();
    response.on_hover_text(tip);
    clicked
}

/// A small screen in the title strip.
fn lcd_text(ui: &mut egui::Ui, text: &str, color: Color32, p: &Palette) {
    let g = ui.painter().layout_job(tracked(text, mono(11.0), 1.0, color));
    let (rect, _) = ui.allocate_exact_size(g.size() + vec2(18.0, 8.0), Sense::hover());
    ui.painter().rect(rect, 3.0, p.screen, Stroke::new(1.0, p.screen_edge), StrokeKind::Inside);
    ui.painter().galley(rect.center() - g.size() / 2.0, g, color);
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
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

        // Left margin clears the traffic lights; the 32 pt strip centers on them.
        let strip = egui::Frame::new().fill(p.face).inner_margin(egui::Margin { left: 80, right: 14, top: 4, bottom: 4 });
        egui::Panel::top("title").frame(strip).exact_size(32.0).show(ui, |ui| self.title_strip(ui, &deck));

        let controls = egui::Frame::new().fill(p.face).inner_margin(egui::Margin { left: 16, right: 16, top: 12, bottom: 10 });
        egui::Panel::bottom("controls").frame(controls).show(ui, |ui| self.control_row(ui, &mut deck));

        let body = egui::Frame::new().fill(p.face).inner_margin(egui::Margin { left: 14, right: 14, top: 10, bottom: 4 });
        egui::CentralPanel::default().frame(body).show(ui, |ui| {
            self.display(ui, &mut deck);
            ui.add_space(10.0);
            self.playlist(ui, &mut deck, hovering);
        });

        if moving {
            ctx.request_repaint_after(Duration::from_millis(33));
        } else if deck.transport != Transport::Stopped {
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
        };
        eframe::set_value(storage, eframe::APP_KEY, &prefs);
    }
}
