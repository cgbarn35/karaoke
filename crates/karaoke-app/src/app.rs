use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::{
    self, Align, Color32, FontId, Galley, Key, Layout, Modifiers, Pos2, Rect, RichText, Sense,
    Stroke, TextFormat, Ui, Vec2, text::LayoutJob,
};
use karaoke_core::db;
use karaoke_core::model::{Line, Song, SongSummary, Word};
use karaoke_core::pinyin::strip_tones;
use rusqlite::Connection;

use crate::fonts;
use crate::playback::Playback;

const READING_PT: f32 = 18.0;
const HANZI_PT: f32 = 48.0;
const GLOSS_PT: f32 = 13.0;
const CELL_PAD: f32 = 6.0;
const GROUP_GAP: f32 = 16.0;
const SEEK_STEP_MS: i64 = 5000;
const OFFSET_STEP_MS: i32 = 100;

struct Toggles {
    pinyin: bool,
    char_gloss: bool,
    word_gloss: bool,
    translation: bool,
    sheet: bool,
    detail: bool,
}

impl Default for Toggles {
    fn default() -> Self {
        Self {
            pinyin: true,
            char_gloss: true,
            word_gloss: true,
            translation: true,
            sheet: false,
            detail: true,
        }
    }
}

/// `(line, word, char)` indices into the loaded song.
type Selection = (usize, usize, usize);

/// User intents collected while rendering, applied after the borrow of the song ends.
enum Action {
    /// Seek so that lyric time becomes this value.
    SeekLyric(i64),
    Select(Selection),
}

pub struct App {
    db_path: PathBuf,
    conn: Option<Connection>,
    songs: Vec<SongSummary>,
    search: String,
    central_msg: Option<String>,
    status: String,
    song: Option<Song>,
    playback: Playback,
    selection: Option<Selection>,
    toggles: Toggles,
    sheet_scrolled_line: Option<usize>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, db_path: PathBuf) -> Self {
        let mut app = Self {
            db_path,
            conn: None,
            songs: Vec::new(),
            search: String::new(),
            central_msg: None,
            status: String::new(),
            song: None,
            playback: Playback::sync(),
            selection: None,
            toggles: Toggles::default(),
            sheet_scrolled_line: None,
        };
        if let Err(e) = fonts::install(&cc.egui_ctx) {
            app.status = e;
        }
        app.open_db();
        app
    }

    fn open_db(&mut self) {
        if !self.db_path.exists() {
            self.central_msg = Some(format!(
                "No database at {}. Add songs with karaoke-ingest.",
                self.db_path.display()
            ));
            return;
        }
        let result = db::open(&self.db_path).and_then(|conn| {
            let songs = db::list_songs(&conn)?;
            Ok((conn, songs))
        });
        match result {
            Ok((conn, songs)) => {
                self.conn = Some(conn);
                self.songs = songs;
                if let Some(first) = self.songs.first() {
                    self.select_song(first.id);
                }
            }
            Err(e) => self.status = format!("Database error: {e:#}"),
        }
    }

    fn select_song(&mut self, id: i64) {
        self.playback.pause();
        self.playback = Playback::sync();
        self.song = None;
        self.selection = None;
        self.sheet_scrolled_line = None;
        let Some(conn) = &self.conn else { return };
        match db::load_song(conn, id, &db::db_dir(&self.db_path)) {
            Ok(song) => {
                self.status = self.make_playback(&song);
                self.song = Some(song);
            }
            Err(e) => self.status = format!("Failed to load song: {e:#}"),
        }
    }

    /// Sets `self.playback` for `song`; returns the status message.
    fn make_playback(&mut self, song: &Song) -> String {
        let reason = match &song.audio_path {
            None => {
                self.playback = Playback::sync();
                return "Sync mode: no audio file; start your external player and press Space together"
                    .into();
            }
            Some(path) if !path.exists() => format!("file not found: {}", path.display()),
            Some(path) => match Playback::audio(path) {
                Ok(p) => {
                    self.playback = p;
                    return String::new();
                }
                Err(e) => format!("{e:#}"),
            },
        };
        self.playback = Playback::sync();
        format!("Audio unavailable ({reason}); using sync mode")
    }

    fn offset(&self) -> i64 {
        self.song.as_ref().map_or(0, |s| i64::from(s.offset_ms))
    }

    fn duration(&self) -> i64 {
        self.song.as_ref().map_or(0, |s| i64::from(s.duration_ms))
    }

    /// Lyric time: playback clock plus the per-song offset.
    fn lyric_ms(&self) -> i64 {
        self.playback.now_ms() + self.offset()
    }

    fn seek_lyric(&mut self, t: i64) {
        let t = t.clamp(0, self.duration());
        if let Err(e) = self.playback.seek(t - self.offset()) {
            self.status = e;
        }
    }

    fn toggle_play(&mut self) {
        let Some(song) = &self.song else { return };
        if self.playback.is_playing() {
            self.playback.pause();
            return;
        }
        let duration = song.duration_ms;
        if self.playback.finished(duration) || self.lyric_ms() >= i64::from(duration) {
            if self.playback.is_audio() {
                // The consumed source cannot be rewound; rebuild the player.
                let song = song.clone();
                self.status = self.make_playback(&song);
            }
            self.seek_lyric(0);
        }
        self.playback.play();
    }

    fn nudge_offset(&mut self, delta: i32) {
        let (Some(song), Some(conn)) = (&mut self.song, &self.conn) else {
            return;
        };
        song.offset_ms += delta;
        if let Err(e) = db::set_offset(conn, song.summary.id, song.offset_ms) {
            self.status = format!("Failed to save offset: {e:#}");
        }
    }

    fn seek_line(&mut self, forward: bool) {
        let Some(song) = &self.song else { return };
        let t = self.lyric_ms();
        let target = match (forward, current_line(&song.lines, t)) {
            (true, cur) => song.lines.get(cur.map_or(0, |i| i + 1)),
            (false, Some(i)) => song.lines.get(i.saturating_sub(1)),
            (false, None) => song.lines.first(),
        };
        if let Some(line) = target {
            let start = i64::from(line.start_ms);
            self.seek_lyric(start);
        }
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() || self.song.is_none() {
            return;
        }
        let pressed = |key| ctx.input_mut(|i| i.consume_key(Modifiers::NONE, key));
        if pressed(Key::Space) {
            self.toggle_play();
        }
        if pressed(Key::ArrowLeft) {
            self.seek_lyric(self.lyric_ms() - SEEK_STEP_MS);
        }
        if pressed(Key::ArrowRight) {
            self.seek_lyric(self.lyric_ms() + SEEK_STEP_MS);
        }
        if pressed(Key::ArrowUp) {
            self.seek_line(false);
        }
        if pressed(Key::ArrowDown) {
            self.seek_line(true);
        }
        if pressed(Key::OpenBracket) {
            self.nudge_offset(-OFFSET_STEP_MS);
        }
        if pressed(Key::CloseBracket) {
            self.nudge_offset(OFFSET_STEP_MS);
        }
    }

    fn library_panel(&mut self, ui: &mut Ui) {
        ui.heading("Library");
        ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("Search"));
        ui.separator();
        let query = self.search.to_lowercase();
        let query_plain = plain(&self.search);
        let current = self.song.as_ref().map(|s| s.summary.id);
        let mut clicked = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for s in self
                .songs
                .iter()
                .filter(|s| matches(s, &query, &query_plain))
            {
                let label = format!("{} — {}", s.title, s.artist);
                if ui
                    .add(egui::Button::selectable(current == Some(s.id), label))
                    .clicked()
                {
                    clicked = Some(s.id);
                }
                ui.weak(format!("{} · {}", s.title_reading, s.title_en));
                ui.add_space(4.0);
            }
        });
        if let Some(id) = clicked {
            self.select_song(id);
        }
    }

    fn top_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let t = &mut self.toggles;
            ui.toggle_value(&mut t.pinyin, "Pinyin");
            ui.toggle_value(&mut t.char_gloss, "Char gloss");
            ui.toggle_value(&mut t.word_gloss, "Word gloss");
            ui.toggle_value(&mut t.translation, "Line translation");
            ui.toggle_value(&mut t.sheet, "Lyric sheet");
            ui.toggle_value(&mut t.detail, "Detail panel");
        });
    }

    fn transport(&mut self, ui: &mut Ui, t: i64) {
        ui.horizontal(|ui| {
            let enabled = self.song.is_some();
            let label = if self.playback.is_playing() {
                "⏸ Pause"
            } else {
                "▶ Play"
            };
            if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                self.toggle_play();
            }
            let duration = self.duration();
            let mut pos = t;
            let slider = egui::Slider::new(&mut pos, 0..=duration).show_value(false);
            if ui.add_enabled(enabled, slider).changed() {
                self.seek_lyric(pos);
            }
            ui.monospace(format!("{} / {}", mmss(t), mmss(duration)));
            ui.separator();
            ui.label(if self.playback.is_audio() {
                "Audio"
            } else {
                "Sync"
            });
            ui.separator();
            if ui
                .add_enabled(enabled, egui::Button::new("−100 ms"))
                .clicked()
            {
                self.nudge_offset(-OFFSET_STEP_MS);
            }
            ui.monospace(format!("offset {:+} ms", self.offset()));
            if ui
                .add_enabled(enabled, egui::Button::new("+100 ms"))
                .clicked()
            {
                self.nudge_offset(OFFSET_STEP_MS);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(&self.status);
            });
        });
    }

    fn detail_panel(&self, ui: &mut Ui) {
        ui.heading("Detail");
        ui.separator();
        let selected = self.selection.and_then(|(li, wi, ci)| {
            let word = self.song.as_ref()?.lines.get(li)?.words.get(wi)?;
            Some((word, word.chars.get(ci)?))
        });
        let Some((word, ch)) = selected else {
            ui.weak("Click a character for details");
            return;
        };
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.label(RichText::new(&ch.ch).size(72.0));
            ui.label(RichText::new(&ch.reading).size(24.0));
            ui.label(&ch.gloss);
            ui.add_space(8.0);
            ui.label(format!(
                "Word: {} {} — {}",
                word.text, word.reading, word.gloss
            ));
            ui.add_space(8.0);
            ui.strong("Dictionary (character)");
            dict_list(ui, &ch.dict);
            if word.chars.len() > 1 {
                ui.add_space(8.0);
                ui.strong("Dictionary (word)");
                dict_list(ui, &word.dict);
            }
            ui.add_space(12.0);
            ui.small("Dictionary data: CC-CEDICT (CC BY-SA 4.0), MDBG");
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_keys(&ctx);

        // End of song: pause and clamp.
        let duration = self.duration();
        if let Some(song) = &self.song
            && self.playback.is_playing()
            && (self.playback.finished(song.duration_ms) || self.lyric_ms() >= duration)
        {
            self.playback.pause();
        }
        let t = self.lyric_ms().clamp(0, duration);

        egui::Panel::top("toggles").show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("transport").show(ui, |ui| self.transport(ui, t));
        egui::Panel::left("library")
            .resizable(true)
            .default_size(260.0)
            .show(ui, |ui| self.library_panel(ui));
        if self.toggles.detail {
            egui::Panel::right("detail")
                .resizable(true)
                .default_size(280.0)
                .show(ui, |ui| self.detail_panel(ui));
        }

        let playing = self.playback.is_playing();
        let action = egui::CentralPanel::default()
            .show(ui, |ui| {
                let Some(song) = &self.song else {
                    ui.centered_and_justified(|ui| {
                        ui.heading(self.central_msg.as_deref().unwrap_or("Select a song"));
                    });
                    return None;
                };
                if self.toggles.sheet {
                    let cur = current_line(&song.lines, t);
                    let scroll = playing && cur.is_some() && cur != self.sheet_scrolled_line;
                    if scroll {
                        self.sheet_scrolled_line = cur;
                    }
                    lyric_sheet(ui, song, cur, scroll, &self.toggles)
                } else {
                    karaoke_view(ui, song, t, &self.toggles, self.selection)
                }
            })
            .inner;
        match action {
            Some(Action::SeekLyric(ms)) => self.seek_lyric(ms),
            Some(Action::Select(sel)) => self.selection = Some(sel),
            None => {}
        }

        if self.playback.is_playing() {
            ctx.request_repaint();
        }
    }
}

/// Index of the last line with `start_ms <= t`.
fn current_line(lines: &[Line], t: i64) -> Option<usize> {
    lines
        .partition_point(|l| i64::from(l.start_ms) <= t)
        .checked_sub(1)
}

/// Lowercase, tone-stripped, whitespace-free form for reading search.
fn plain(s: &str) -> String {
    strip_tones(s)
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

fn matches(s: &SongSummary, query_lower: &str, query_plain: &str) -> bool {
    query_lower.is_empty()
        || [&s.title, &s.title_en, &s.artist, &s.artist_en]
            .iter()
            .any(|f| f.to_lowercase().contains(query_lower))
        || (!query_plain.is_empty()
            && (plain(&s.title_reading).contains(query_plain)
                || plain(&s.artist_reading).contains(query_plain)))
}

fn mmss(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    format!("{}:{:02}", s / 60, s % 60)
}

fn dict_list(ui: &mut Ui, dict: &[karaoke_core::model::DictEntry]) {
    if dict.is_empty() {
        ui.weak("(no entries)");
    }
    for e in dict {
        ui.label(format!("{} — {}", e.reading, e.defs.join("; ")));
    }
}

fn line_reading(line: &Line) -> String {
    line.words
        .iter()
        .filter(|w| w.is_han)
        .map(|w| w.reading.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

/// A dim, compact context line (hanzi + optional reading) at `scale` of full size.
fn dim_line(ui: &mut Ui, line: &Line, scale: f32, pinyin: bool) {
    if line.text.is_empty() {
        ui.weak(RichText::new("♪").size(HANZI_PT * scale));
        return;
    }
    if pinyin {
        ui.weak(RichText::new(line_reading(line)).size(READING_PT * scale));
    }
    ui.weak(RichText::new(&line.text).size(HANZI_PT * scale));
}

fn lyric_sheet(
    ui: &mut Ui,
    song: &Song,
    cur: Option<usize>,
    scroll: bool,
    toggles: &Toggles,
) -> Option<Action> {
    let mut action = None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (i, line) in song.lines.iter().enumerate() {
                let is_cur = cur == Some(i);
                let frame = if is_cur {
                    egui::Frame::new().fill(ui.visuals().selection.bg_fill.gamma_multiply(0.35))
                } else {
                    egui::Frame::new()
                };
                let inner = frame.inner_margin(6.0).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    if line.text.is_empty() {
                        ui.weak("♪");
                        return;
                    }
                    if toggles.pinyin {
                        ui.weak(line_reading(line));
                    }
                    ui.label(RichText::new(&line.text).size(24.0));
                    if toggles.translation {
                        ui.label(RichText::new(&line.translation).italics());
                    }
                });
                let resp = ui.interact(
                    inner.response.rect,
                    ui.scope_id().with(("sheet_line", i)),
                    Sense::click(),
                );
                if resp.clicked() {
                    action = Some(Action::SeekLyric(i64::from(line.start_ms)));
                }
                if is_cur && scroll {
                    resp.scroll_to_me(Some(Align::Center));
                }
            }
        });
    action
}

fn karaoke_view(
    ui: &mut Ui,
    song: &Song,
    t: i64,
    toggles: &Toggles,
    selection: Option<Selection>,
) -> Option<Action> {
    let mut action = None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(24.0);
                let Some(cur) = current_line(&song.lines, t) else {
                    title_card(ui, song, toggles.pinyin);
                    return;
                };
                match cur.checked_sub(1).map(|p| &song.lines[p]) {
                    Some(prev) if !prev.text.is_empty() => dim_line(ui, prev, 0.6, toggles.pinyin),
                    _ => {
                        ui.add_space(HANZI_PT * 0.6);
                    }
                }
                ui.add_space(16.0);
                let line = &song.lines[cur];
                if line.text.is_empty() {
                    ui.label(RichText::new("♪").size(HANZI_PT));
                    if let Some(next) = song.lines.get(cur + 1) {
                        ui.add_space(12.0);
                        dim_line(ui, next, 0.6, toggles.pinyin);
                    }
                    return;
                }
                action = current_line_view(ui, line, cur, t, toggles, selection);
                if toggles.translation {
                    ui.add_space(8.0);
                    ui.label(RichText::new(&line.translation).size(20.0));
                }
                ui.add_space(24.0);
                for next in song.lines.iter().skip(cur + 1).take(2) {
                    dim_line(ui, next, 0.6, toggles.pinyin);
                    ui.add_space(8.0);
                }
            });
        });
    action
}

fn title_card(ui: &mut Ui, song: &Song, pinyin: bool) {
    let s = &song.summary;
    ui.label(RichText::new(&s.title).size(40.0));
    if pinyin {
        ui.label(RichText::new(&s.title_reading).size(READING_PT));
    }
    ui.label(RichText::new(&s.title_en).size(20.0));
    ui.add_space(8.0);
    ui.label(format!(
        "{} · {} · {}",
        s.artist, s.artist_reading, s.artist_en
    ));
    if let Some(first) = song.lines.iter().find(|l| !l.text.is_empty()) {
        ui.add_space(32.0);
        dim_line(ui, first, 0.6, pinyin);
    }
}

/// One char cell: galleys laid out with placeholder color so they can be repainted in any color.
struct Cell {
    hanzi: Arc<Galley>,
    reading: Option<Arc<Galley>>,
    gloss: Option<Arc<Galley>>,
    width: f32,
}

enum GroupKind {
    Han {
        cells: Vec<Cell>,
        word_gloss: Option<Arc<Galley>>,
    },
    /// Non-Han text on the hanzi row; `None` for whitespace (a plain gap).
    Plain(Option<Arc<Galley>>),
}

struct Group {
    word_idx: usize,
    kind: GroupKind,
    width: f32,
}

fn layout_group(painter: &egui::Painter, word: &Word, word_idx: usize, toggles: &Toggles) -> Group {
    let ph = Color32::PLACEHOLDER;
    let text =
        |s: &str, pt: f32| painter.layout_no_wrap(s.to_owned(), FontId::proportional(pt), ph);
    if !word.is_han {
        if word.text.trim().is_empty() {
            return Group {
                word_idx,
                kind: GroupKind::Plain(None),
                width: GROUP_GAP,
            };
        }
        let g = text(&word.text, HANZI_PT);
        let width = g.size().x;
        return Group {
            word_idx,
            kind: GroupKind::Plain(Some(g)),
            width,
        };
    }
    let cells: Vec<Cell> = word
        .chars
        .iter()
        .map(|c| {
            let hanzi = text(&c.ch, HANZI_PT);
            let reading = toggles.pinyin.then(|| text(&c.reading, READING_PT));
            let gloss = toggles.char_gloss.then(|| text(&c.gloss, GLOSS_PT));
            let width = [Some(&hanzi), reading.as_ref(), gloss.as_ref()]
                .into_iter()
                .flatten()
                .map(|g| g.size().x)
                .fold(0.0, f32::max)
                + CELL_PAD;
            Cell {
                hanzi,
                reading,
                gloss,
                width,
            }
        })
        .collect();
    let word_gloss = (toggles.word_gloss && word.chars.len() > 1).then(|| {
        let format = TextFormat {
            font_id: FontId::proportional(GLOSS_PT),
            color: ph,
            italics: true,
            ..Default::default()
        };
        painter.layout_job(LayoutJob::single_section(word.gloss.clone(), format))
    });
    let cells_w: f32 = cells.iter().map(|c| c.width).sum();
    let width = word_gloss
        .as_ref()
        .map_or(cells_w, |g| cells_w.max(g.size().x));
    Group {
        word_idx,
        kind: GroupKind::Han { cells, word_gloss },
        width,
    }
}

/// Paints `galley` at `pos` in `base`, then repaints the part left of `wipe_x` in `accent`.
fn paint_wiped(
    painter: &egui::Painter,
    pos: Pos2,
    galley: &Arc<Galley>,
    base: Color32,
    accent: Color32,
    wipe_x: Option<f32>,
) {
    painter.galley(pos, galley.clone(), base);
    if let Some(x) = wipe_x {
        let mut clip = painter.clip_rect();
        clip.max.x = clip.max.x.min(x);
        if clip.width() > 0.0 {
            painter
                .with_clip_rect(clip)
                .galley(pos, galley.clone(), accent);
        }
    }
}

/// Fraction of `[start, end)` elapsed at `t`, clamped to `0..=1`.
fn progress(t: i64, start: u32, end: u32) -> f32 {
    let (start, end) = (i64::from(start), i64::from(end));
    if t >= end {
        1.0
    } else if t < start {
        0.0
    } else {
        (t - start) as f32 / (end - start) as f32
    }
}

fn current_line_view(
    ui: &mut Ui,
    line: &Line,
    line_idx: usize,
    t: i64,
    toggles: &Toggles,
    selection: Option<Selection>,
) -> Option<Action> {
    let painter = ui.painter().clone();
    let groups: Vec<Group> = line
        .words
        .iter()
        .enumerate()
        .map(|(i, w)| layout_group(&painter, w, i, toggles))
        .collect();

    // Wrap whole groups into rows.
    let avail = ui.available_width();
    let mut rows: Vec<(usize, usize, f32)> = Vec::new(); // (start, end, width)
    let mut start = 0;
    let mut width = 0.0;
    for (i, g) in groups.iter().enumerate() {
        let add = if i > start {
            GROUP_GAP + g.width
        } else {
            g.width
        };
        if i > start && width + add > avail {
            rows.push((start, i, width));
            start = i;
            width = g.width;
        } else {
            width += add;
        }
    }
    if start < groups.len() {
        rows.push((start, groups.len(), width));
    }

    let measure = |pt: f32| {
        painter
            .layout_no_wrap("汉".into(), FontId::proportional(pt), Color32::PLACEHOLDER)
            .size()
            .y
    };
    let reading_h = if toggles.pinyin {
        measure(READING_PT)
    } else {
        0.0
    };
    let hanzi_h = measure(HANZI_PT);
    let gloss_h = if toggles.char_gloss {
        measure(GLOSS_PT)
    } else {
        0.0
    };
    let has_word_gloss = groups.iter().any(|g| {
        matches!(
            g.kind,
            GroupKind::Han {
                word_gloss: Some(_),
                ..
            }
        )
    });
    let word_gloss_h = if has_word_gloss {
        measure(GLOSS_PT)
    } else {
        0.0
    };
    let row_h = reading_h + hanzi_h + gloss_h + word_gloss_h + 8.0;

    let visuals = ui.visuals();
    let base = visuals.text_color();
    let weak = visuals.weak_text_color();
    let accent = visuals.selection.bg_fill.to_opaque();
    let select_stroke = Stroke::new(1.5, accent);

    let mut action = None;
    for (r_start, r_end, r_width) in rows {
        let (row_rect, _) = ui.allocate_exact_size(Vec2::new(avail, row_h), Sense::hover());
        let mut x = row_rect.left() + (avail - r_width).max(0.0) / 2.0;
        let top = row_rect.top();
        for (gi, g) in groups[r_start..r_end].iter().enumerate() {
            if gi > 0 {
                x += GROUP_GAP;
            }
            let word = &line.words[g.word_idx];
            match &g.kind {
                GroupKind::Plain(None) => {}
                GroupKind::Plain(Some(galley)) => {
                    let done = word.chars.first().is_some_and(|c| t >= i64::from(c.end_ms));
                    let color = if done { accent } else { base };
                    painter.galley(Pos2::new(x, top + reading_h), galley.clone(), color);
                }
                GroupKind::Han { cells, word_gloss } => {
                    let cells_w: f32 = cells.iter().map(|c| c.width).sum();
                    let mut cx = x + (g.width - cells_w) / 2.0;
                    for (ci, (cell, ch)) in cells.iter().zip(&word.chars).enumerate() {
                        let cell_rect = Rect::from_min_size(
                            Pos2::new(cx, top),
                            Vec2::new(cell.width, reading_h + hanzi_h + gloss_h),
                        );
                        let p = progress(t, ch.start_ms, ch.end_ms);
                        let wipe_x = (p > 0.0).then(|| cell_rect.left() + p * cell.width);
                        let centered = |galley: &Arc<Galley>, y: f32| {
                            Pos2::new(cell_rect.center().x - galley.size().x / 2.0, y)
                        };
                        if let Some(r) = &cell.reading {
                            paint_wiped(&painter, centered(r, top), r, base, accent, wipe_x);
                        }
                        let hy = top + reading_h;
                        paint_wiped(
                            &painter,
                            centered(&cell.hanzi, hy),
                            &cell.hanzi,
                            base,
                            accent,
                            wipe_x,
                        );
                        if let Some(gl) = &cell.gloss {
                            painter.galley(centered(gl, hy + hanzi_h), gl.clone(), weak);
                        }
                        let sel = (line_idx, g.word_idx, ci);
                        if selection == Some(sel) {
                            painter.rect_stroke(
                                cell_rect,
                                4.0,
                                select_stroke,
                                egui::StrokeKind::Outside,
                            );
                        }
                        let resp =
                            ui.interact(cell_rect, ui.scope_id().with(("cell", sel)), Sense::click());
                        if resp.clicked() {
                            action = Some(Action::Select(sel));
                        }
                        cx += cell.width;
                    }
                    if let Some(wg) = word_gloss {
                        let pos = Pos2::new(
                            x + (g.width - wg.size().x) / 2.0,
                            top + reading_h + hanzi_h + gloss_h,
                        );
                        painter.galley(pos, wg.clone(), weak);
                    }
                }
            }
            x += g.width;
        }
    }
    action
}
