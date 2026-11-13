//! A telnet session: socket + telnet protocol + Ghostty VT + GPUI renderer.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::{
    App, Bounds, Context, EventEmitter, FocusHandle, Focusable, Font,
    FontStyle, FontWeight, Hsla, InteractiveElement, IntoElement, KeyDownEvent, ParentElement,
    Pixels, Render, ScrollWheelEvent, SharedString, StrikethroughStyle, Styled, Task, TextRun,
    UnderlineStyle, Window, canvas, div, fill, font, point, prelude::*, px, size,
};
use libghostty_vt::{
    RenderState, Terminal, TerminalOptions,
    key::{self, Key},
    render::{CellIterator, CursorVisualStyle, RowIterator},
    screen::CellWide,
    style::{Palette, Underline},
    terminal::{Mode, ScrollViewport},
};

use crate::net::{Connection, NetEvent};
use crate::telnet::Telnet;
use crate::theme::{self, c};

const PAD: f32 = 10.;

#[derive(Clone, PartialEq)]
pub enum Status {
    Connecting,
    Connected,
    Closed(Option<String>),
}

pub enum SessionEvent {
    Back,
}

impl EventEmitter<SessionEvent> for Session {}

pub struct Session {
    pub host: String,
    pub port: u16,
    pub status: Status,
    focus: FocusHandle,
    conn: Option<Connection>,
    _reader: Option<Task<()>>,
    telnet: Telnet,
    term: Terminal<'static, 'static>,
    render: RenderState<'static>,
    rows_it: RowIterator<'static>,
    cells_it: CellIterator<'static>,
    keys: key::Encoder<'static>,
    key_ev: key::Event<'static>,
    /// Replies the VT wants sent back (DSR, DA, ...), filled by the callback.
    outbox: Rc<RefCell<Vec<u8>>>,
    title_dirty: Rc<Cell<bool>>,
    /// Local line buffer used when the server doesn't echo (NVT line mode).
    line: String,
    grid: (u16, u16),
    font_size: Pixels,
    scroll_acc: f32,
}

/// One shaped run of cells sharing a style.
struct Span {
    col: u16,
    text: String,
    fg: Hsla,
    bold: bool,
    italic: bool,
    underline: bool,
    curly: bool,
    strike: bool,
}

struct Frame {
    spans: Vec<Vec<Span>>,
    /// (row, col, width, color)
    bgs: Vec<(u16, u16, u16, Hsla)>,
    cursor: Option<(u16, u16, bool, CursorVisualStyle, Hsla)>,
    bg: Hsla,
}

impl Session {
    pub fn new(host: String, port: u16, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (cols, rows) = (80, 24);
        let outbox = Rc::new(RefCell::new(Vec::new()));
        let title_dirty = Rc::new(Cell::new(false));
        let mut term = Terminal::new(TerminalOptions { cols, rows, max_scrollback: 10_000 })
            .expect("create terminal");
        let mut palette = term.default_color_palette().expect("palette");
        for (i, hex) in theme::ANSI.iter().enumerate() {
            palette.0[i] = theme::rgbc(*hex);
        }
        let ob = outbox.clone();
        let td = title_dirty.clone();
        term.set_default_fg_color(Some(theme::rgbc(theme::FG)))
            .and_then(|t| t.set_default_bg_color(Some(theme::rgbc(theme::BG))))
            .and_then(|t| t.set_default_cursor_color(Some(theme::rgbc(theme::ACCENT))))
            .and_then(|t| t.set_default_color_palette(Some(Palette(palette.0))))
            .expect("theme terminal");
        term.on_pty_write(move |_, data| ob.borrow_mut().extend_from_slice(data))
            .and_then(|t| t.on_title_changed(move |_| td.set(true)))
            .and_then(|t| t.on_xtversion(|_| Some("tent")))
            .expect("terminal callbacks");

        let mut this = Self {
            host,
            port,
            status: Status::Connecting,
            focus: cx.focus_handle(),
            conn: None,
            _reader: None,
            telnet: Telnet::new(cols, rows),
            term,
            render: RenderState::new().expect("render state"),
            rows_it: RowIterator::new().expect("row iterator"),
            cells_it: CellIterator::new().expect("cell iterator"),
            keys: key::Encoder::new().expect("key encoder"),
            key_ev: key::Event::new().expect("key event"),
            outbox,
            title_dirty,
            line: String::new(),
            grid: (cols, rows),
            font_size: px(14.),
            scroll_acc: 0.,
        };
        this.connect(window, cx);
        window.focus(&this.focus);
        this
    }

    pub fn connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.status = Status::Connecting;
        self.telnet = Telnet::new(self.grid.0, self.grid.1);
        self.line.clear();
        self.term.reset();
        let (conn, rx) = Connection::open(self.host.clone(), self.port);
        self.conn = Some(conn);
        self._reader = Some(cx.spawn_in(window, async move |this, cx| {
            while let Ok(ev) = rx.recv().await {
                if this.update_in(cx, |s, w, cx| s.on_net(ev, w, cx)).is_err() {
                    break;
                }
            }
        }));
        window.set_window_title(&format!("tent — {}", self.host));
        cx.notify();
    }

    fn on_net(&mut self, ev: NetEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            NetEvent::Connected => self.status = Status::Connected,
            NetEvent::Data(bytes) => {
                let r = self.telnet.receive(&bytes);
                self.send_raw(r.reply);
                self.term.vt_write(&r.data);
                let replies = std::mem::take(&mut *self.outbox.borrow_mut());
                if !replies.is_empty() {
                    self.send_raw(self.telnet.encode(&replies));
                }
                if self.title_dirty.replace(false) {
                    let t = self.term.title().unwrap_or_default();
                    let t = if t.is_empty() { self.host.clone() } else { t.to_string() };
                    window.set_window_title(&format!("tent — {t}"));
                }
            }
            NetEvent::Closed(reason) => {
                self.status = Status::Closed(reason);
                self.conn = None;
            }
        }
        cx.notify();
    }

    fn send_raw(&self, bytes: Vec<u8>) {
        if let Some(conn) = &self.conn {
            conn.send(bytes);
        }
    }

    /// Send keyboard-originated bytes, applying NVT line mode if the server
    /// doesn't echo.
    fn send_input(&mut self, bytes: &[u8]) {
        if self.telnet.remote_echo() || bytes.first() == Some(&0x1b) && !self.line.is_empty() {
            if !self.telnet.remote_echo() {
                return; // escape sequences mid-line would corrupt the line
            }
            self.send_raw(self.telnet.encode(bytes));
            return;
        }
        if bytes.first() == Some(&0x1b) {
            self.send_raw(self.telnet.encode(bytes));
            return;
        }
        let mut echo = Vec::new();
        for ch in String::from_utf8_lossy(bytes).chars() {
            match ch {
                '\r' | '\n' => {
                    let mut out = std::mem::take(&mut self.line).into_bytes();
                    out.extend_from_slice(b"\r\n");
                    self.send_raw(self.telnet.encode(&out));
                    echo.extend_from_slice(b"\r\n");
                }
                '\x7f' | '\x08' => {
                    if self.line.pop().is_some() {
                        echo.extend_from_slice(b"\x08 \x08");
                    }
                }
                c if (c as u32) < 0x20 => {
                    self.send_raw(self.telnet.encode(&[c as u8]));
                }
                c => {
                    self.line.push(c);
                    let mut b = [0; 4];
                    echo.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
                }
            }
        }
        self.term.vt_write(&echo);
    }

    fn resize(&mut self, cols: u16, rows: u16, cell: gpui::Size<Pixels>, cx: &mut Context<Self>) {
        if self.grid == (cols, rows) {
            return;
        }
        self.grid = (cols, rows);
        let _ = self.term.resize(cols, rows, f32::from(cell.width) as u32, f32::from(cell.height) as u32);
        if let Some(naws) = self.telnet.resize(cols, rows) {
            self.send_raw(naws);
        }
        cx.notify();
    }

    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let m = &ks.modifiers;
        let ctrl_shift = m.control && m.shift && !m.alt;
        match ks.key.as_str() {
            "v" if ctrl_shift => return self.paste(cx),
            "=" | "+" if m.control => return self.zoom(1., cx),
            "-" if m.control => return self.zoom(-1., cx),
            "0" if m.control => {
                self.font_size = px(14.);
                return cx.notify();
            }
            "pageup" if m.shift => {
                self.term.scroll_viewport(ScrollViewport::Delta(-(self.grid.1 as isize - 1)));
                return cx.notify();
            }
            "pagedown" if m.shift => {
                self.term.scroll_viewport(ScrollViewport::Delta(self.grid.1 as isize - 1));
                return cx.notify();
            }
            "escape" if !matches!(self.status, Status::Connected | Status::Connecting) => {
                return cx.emit(SessionEvent::Back);
            }
            "enter" if matches!(self.status, Status::Closed(_)) => return self.connect(window, cx),
            _ => {}
        }
        if self.status != Status::Connected {
            return;
        }

        let (k, unshifted) = map_key(&ks.key);
        let mut mods = key::Mods::empty();
        mods.set(key::Mods::SHIFT, m.shift);
        mods.set(key::Mods::CTRL, m.control);
        mods.set(key::Mods::ALT, m.alt);
        mods.set(key::Mods::SUPER, m.platform);
        let text = ks.key_char.clone().filter(|t| !t.chars().any(char::is_control));
        let mut consumed = key::Mods::empty();
        if text.is_some() && m.shift {
            consumed |= key::Mods::SHIFT;
        }
        self.key_ev
            .set_action(if ev.is_held { key::Action::Repeat } else { key::Action::Press })
            .set_key(k)
            .set_mods(mods)
            .set_consumed_mods(consumed)
            .set_unshifted_codepoint(unshifted)
            .set_utf8(text.clone());
        let mut out = Vec::new();
        let _ = self.keys.set_options_from_terminal(&self.term).encode_to_vec(&self.key_ev, &mut out);
        if out.is_empty()
            && let Some(t) = text
        {
            out = t.into_bytes();
        }
        if !out.is_empty() {
            self.term.scroll_viewport(ScrollViewport::Bottom);
            self.send_input(&out);
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) else { return };
        if self.status != Status::Connected {
            return;
        }
        let bracketed = self.term.mode(Mode::BRACKETED_PASTE).unwrap_or(false);
        let mut data = text.replace("\r\n", "\r").replace('\n', "\r").into_bytes();
        let mut buf = vec![0u8; data.len() + 16];
        let bytes = match libghostty_vt::paste::encode(&mut data, bracketed, &mut buf) {
            Ok(n) => buf[..n].to_vec(),
            Err(_) => text.into_bytes(),
        };
        self.send_input(&bytes);
        cx.notify();
    }

    fn zoom(&mut self, by: f32, cx: &mut Context<Self>) {
        self.font_size = px((f32::from(self.font_size) + by).clamp(9., 32.));
        cx.notify();
    }

    fn on_scroll(&mut self, ev: &ScrollWheelEvent, line_h: Pixels, cx: &mut Context<Self>) {
        self.scroll_acc += f32::from(ev.delta.pixel_delta(line_h).y) / f32::from(line_h);
        let lines = self.scroll_acc.trunc();
        if lines != 0. {
            self.scroll_acc -= lines;
            self.term.scroll_viewport(ScrollViewport::Delta(-(lines as isize) * 3));
            cx.notify();
        }
    }

    /// Snapshot the VT into plain data for painting.
    fn frame(&mut self, focused: bool) -> Frame {
        let mut frame = Frame { spans: Vec::new(), bgs: Vec::new(), cursor: None, bg: c(theme::BG) };
        let Ok(snap) = self.render.update(&self.term) else { return frame };
        let Ok(colors) = snap.colors() else { return frame };
        frame.bg = theme::hsla(colors.background);
        if let (Ok(true), Ok(Some(vp))) = (snap.cursor_visible(), snap.cursor_viewport()) {
            let style = snap.cursor_visual_style().unwrap_or(CursorVisualStyle::Block);
            let color = theme::hsla(colors.cursor.unwrap_or(theme::rgbc(theme::ACCENT)));
            frame.cursor = Some((vp.x, vp.y, focused, style, color));
        }
        let Ok(mut rows) = self.rows_it.update(&snap) else { return frame };
        let mut text = String::new();
        let mut y = 0u16;
        while let Some(row) = rows.next() {
            let mut spans: Vec<Span> = Vec::new();
            let Ok(mut cells) = self.cells_it.update(row) else { break };
            let mut x = 0u16;
            while let Some(cell) = cells.next() {
                let col = x;
                x += 1;
                let wide = cell.raw_cell().and_then(|c| c.wide()).unwrap_or(CellWide::Narrow);
                if wide == CellWide::SpacerTail {
                    continue;
                }
                let style = cell.style().ok().filter(|_| cell.has_styling().unwrap_or(false));
                let mut fg = cell.fg_color().ok().flatten().unwrap_or(colors.foreground);
                let mut bg = cell.bg_color().ok().flatten();
                if style.is_some_and(|s| s.inverse) {
                    let old = bg.unwrap_or(colors.background);
                    bg = Some(fg);
                    fg = old;
                }
                if cell.is_selected().unwrap_or(false) {
                    bg = Some(theme::rgbc(theme::SURFACE_HI));
                }
                let w = if wide == CellWide::Wide { 2 } else { 1 };
                if let Some(bg) = bg {
                    push_bg(&mut frame.bgs, y, col, w, theme::hsla(bg));
                }
                text.clear();
                if cell.graphemes_len().unwrap_or(0) > 0 && !style.is_some_and(|s| s.invisible) {
                    let _ = cell.graphemes_utf8(&mut text);
                }
                if text.is_empty() {
                    text.push(' ');
                }
                let mut fg = theme::hsla(fg);
                let s = style.unwrap_or_default();
                if s.faint {
                    fg.a = 0.6;
                }
                let span = Span {
                    col,
                    text: String::new(),
                    fg,
                    bold: s.bold,
                    italic: s.italic,
                    underline: s.underline != Underline::None,
                    curly: s.underline == Underline::Curly,
                    strike: s.strikethrough,
                };
                let joinable = w == 1
                    && spans.last().is_some_and(|l: &Span| {
                        l.fg == span.fg
                            && l.bold == span.bold
                            && l.italic == span.italic
                            && l.underline == span.underline
                            && l.curly == span.curly
                            && l.strike == span.strike
                            && l.col + l.text.chars().count() as u16 == col
                    });
                if joinable {
                    spans.last_mut().unwrap().text.push_str(&text);
                } else {
                    spans.push(Span { text: text.clone(), ..span });
                }
                if w == 2 {
                    // Next narrow cell must not join onto a wide glyph.
                    spans.push(Span { col: col + 2, text: String::new(), ..blank(fg) });
                }
            }
            // Trailing blanks don't need shaping.
            for s in &mut spans {
                if !s.underline && !s.strike {
                    s.text.truncate(s.text.trim_end_matches(' ').len());
                }
            }
            spans.retain(|s| !s.text.is_empty());
            frame.spans.push(spans);
            y += 1;
        }
        let _ = snap.set_dirty(libghostty_vt::render::Dirty::Clean);
        frame
    }
}

fn blank(fg: Hsla) -> Span {
    Span { col: 0, text: String::new(), fg, bold: false, italic: false, underline: false, curly: false, strike: false }
}

fn push_bg(bgs: &mut Vec<(u16, u16, u16, Hsla)>, row: u16, col: u16, w: u16, color: Hsla) {
    if let Some(last) = bgs.last_mut()
        && last.0 == row
        && last.1 + last.2 == col
        && last.3 == color
    {
        last.2 += w;
        return;
    }
    bgs.push((row, col, w, color));
}

fn map_key(name: &str) -> (Key, char) {
    let ch = |c: char| c;
    match name {
        "enter" => (Key::Enter, '\0'),
        "tab" => (Key::Tab, '\0'),
        "backspace" => (Key::Backspace, '\0'),
        "escape" => (Key::Escape, '\0'),
        "space" => (Key::Space, ' '),
        "up" => (Key::ArrowUp, '\0'),
        "down" => (Key::ArrowDown, '\0'),
        "left" => (Key::ArrowLeft, '\0'),
        "right" => (Key::ArrowRight, '\0'),
        "home" => (Key::Home, '\0'),
        "end" => (Key::End, '\0'),
        "pageup" => (Key::PageUp, '\0'),
        "pagedown" => (Key::PageDown, '\0'),
        "insert" => (Key::Insert, '\0'),
        "delete" => (Key::Delete, '\0'),
        "f1" => (Key::F1, '\0'),
        "f2" => (Key::F2, '\0'),
        "f3" => (Key::F3, '\0'),
        "f4" => (Key::F4, '\0'),
        "f5" => (Key::F5, '\0'),
        "f6" => (Key::F6, '\0'),
        "f7" => (Key::F7, '\0'),
        "f8" => (Key::F8, '\0'),
        "f9" => (Key::F9, '\0'),
        "f10" => (Key::F10, '\0'),
        "f11" => (Key::F11, '\0'),
        "f12" => (Key::F12, '\0'),
        "-" => (Key::Minus, '-'),
        "=" => (Key::Equal, '='),
        "[" => (Key::BracketLeft, '['),
        "]" => (Key::BracketRight, ']'),
        "\\" => (Key::Backslash, '\\'),
        ";" => (Key::Semicolon, ';'),
        "'" => (Key::Quote, '\''),
        "," => (Key::Comma, ','),
        "." => (Key::Period, '.'),
        "/" => (Key::Slash, '/'),
        "`" => (Key::Backquote, '`'),
        s if s.len() == 1 => {
            let c = s.chars().next().unwrap().to_ascii_lowercase();
            let k = match c {
                'a' => Key::A, 'b' => Key::B, 'c' => Key::C, 'd' => Key::D, 'e' => Key::E,
                'f' => Key::F, 'g' => Key::G, 'h' => Key::H, 'i' => Key::I, 'j' => Key::J,
                'k' => Key::K, 'l' => Key::L, 'm' => Key::M, 'n' => Key::N, 'o' => Key::O,
                'p' => Key::P, 'q' => Key::Q, 'r' => Key::R, 's' => Key::S, 't' => Key::T,
                'u' => Key::U, 'v' => Key::V, 'w' => Key::W, 'x' => Key::X, 'y' => Key::Y,
                'z' => Key::Z, '0' => Key::Digit0, '1' => Key::Digit1, '2' => Key::Digit2,
                '3' => Key::Digit3, '4' => Key::Digit4, '5' => Key::Digit5, '6' => Key::Digit6,
                '7' => Key::Digit7, '8' => Key::Digit8, '9' => Key::Digit9,
                _ => Key::Unidentified,
            };
            (k, ch(c))
        }
        _ => (Key::Unidentified, '\0'),
    }
}

impl Focusable for Session {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Session {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus.is_focused(window);
        let font_size = self.font_size;
        let base = font(theme::FONT);
        let font_id = window.text_system().resolve_font(&base);
        let cell_w = window
            .text_system()
            .advance(font_id, font_size, 'm')
            .map(|s| s.width)
            .unwrap_or(font_size * 0.6);
        let line_h = (font_size * 1.3).round();
        let cell = size(cell_w, line_h);
        let mut frame = self.frame(focused);
        if self.status != Status::Connected {
            frame.cursor = None;
        }
        let entity = cx.entity();

        let grid = canvas(
            move |bounds, _window, cx| {
                let inner = bounds.size.width - px(PAD * 2.);
                let cols = (inner / cell_w).floor().max(2.) as u16;
                let rows = ((bounds.size.height - px(PAD * 2.)) / line_h).floor().max(1.) as u16;
                entity.update(cx, |s, cx| s.resize(cols, rows, cell, cx));
            },
            move |bounds, (), window, cx| paint_frame(frame, bounds, base, font_size, cell, window, cx),
        )
        .size_full();

        let (dot, label): (u32, SharedString) = match &self.status {
            Status::Connecting => (theme::WARN, "connecting…".into()),
            Status::Connected => (theme::OK, "connected".into()),
            Status::Closed(None) => (theme::ERR, "connection closed".into()),
            Status::Closed(Some(e)) => (theme::ERR, e.clone().into()),
        };
        let mode = if self.telnet.remote_echo() { "char" } else { "line" };

        div()
            .key_context("Session")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .on_scroll_wheel(cx.listener(move |s, ev, _, cx| s.on_scroll(ev, line_h, cx)))
            .size_full()
            .flex()
            .flex_col()
            .bg(c(theme::BG))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(30.))
                    .px_3()
                    .bg(c(theme::SURFACE))
                    .border_b_1()
                    .border_color(c(theme::BORDER))
                    .text_size(px(12.))
                    .text_color(c(theme::MUTED))
                    .child(div().size(px(7.)).rounded_full().bg(c(dot)))
                    .child(
                        div()
                            .text_color(c(theme::FG))
                            .child(format!("{}:{}", self.host, self.port)),
                    )
                    .child(div().overflow_hidden().whitespace_nowrap().text_ellipsis().child(label))
                    .child(div().flex_1())
                    .when(self.status == Status::Connected, |d| {
                        d.child(
                            div()
                                .px_1p5()
                                .rounded_sm()
                                .border_1()
                                .border_color(c(theme::BORDER))
                                .child(format!("{mode} mode")),
                        )
                    })
                    .child(format!("{}×{}", self.grid.0, self.grid.1)),
            )
            .child(div().flex_1().relative().child(grid).when(
                matches!(self.status, Status::Closed(_)),
                |d| {
                    d.child(
                        div()
                            .absolute()
                            .bottom(px(16.))
                            .left_0()
                            .right_0()
                            .flex()
                            .justify_center()
                            .child(
                                div()
                                    .flex()
                                    .gap_3()
                                    .px_4()
                                    .py_2()
                                    .rounded_lg()
                                    .bg(c(theme::SURFACE_HI))
                                    .border_1()
                                    .border_color(c(theme::BORDER))
                                    .text_size(px(12.))
                                    .text_color(c(theme::MUTED))
                                    .child(div().text_color(c(theme::FG)).child("disconnected"))
                                    .child(hint("↵", "reconnect"))
                                    .child(hint("esc", "hosts")),
                            ),
                    )
                },
            ))
    }
}

pub fn hint(key: &'static str, what: &'static str) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_1()
        .child(
            div()
                .px_1()
                .rounded_sm()
                .bg(c(theme::BORDER))
                .text_color(c(theme::FG))
                .child(key),
        )
        .child(what)
}

fn paint_frame(
    frame: Frame,
    bounds: Bounds<Pixels>,
    base: Font,
    font_size: Pixels,
    cell: gpui::Size<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    window.paint_quad(fill(bounds, frame.bg));
    let origin = bounds.origin + point(px(PAD), px(PAD));
    let at = |col: u16, row: u16| origin + point(cell.width * col as f32, cell.height * row as f32);

    for &(row, col, w, color) in &frame.bgs {
        window.paint_quad(fill(Bounds::new(at(col, row), size(cell.width * w as f32, cell.height)), color));
    }

    // Block cursor goes under the text so the glyph stays readable.
    let cursor_rect = frame.cursor.map(|(x, y, focused, style, color)| {
        let b = Bounds::new(at(x, y), cell);
        (b, focused, style, color)
    });
    if let Some((b, true, CursorVisualStyle::Block, color)) = cursor_rect {
        window.paint_quad(fill(b, color));
    }

    let ts = window.text_system().clone();
    for (row, spans) in frame.spans.iter().enumerate() {
        for s in spans {
            let mut f = base.clone();
            if s.bold {
                f.weight = FontWeight::BOLD;
            }
            if s.italic {
                f.style = FontStyle::Italic;
            }
            let mut color = s.fg;
            if let Some((b, true, CursorVisualStyle::Block, _)) = cursor_rect {
                // Text under the block cursor is drawn in the background color.
                let cur_col = ((b.origin.x - origin.x) / cell.width).round() as u16;
                let cur_row = ((b.origin.y - origin.y) / cell.height).round() as usize;
                if cur_row == row && s.col == cur_col && s.text.chars().count() == 1 {
                    color = frame.bg;
                }
            }
            let run = TextRun {
                len: s.text.len(),
                font: f,
                color,
                background_color: None,
                underline: s.underline.then(|| UnderlineStyle {
                    thickness: px(1.),
                    color: Some(color),
                    wavy: s.curly,
                }),
                strikethrough: s.strike.then(|| StrikethroughStyle { thickness: px(1.), color: Some(color) }),
            };
            let shaped = ts.shape_line(s.text.clone().into(), font_size, &[run], None);
            let _ = shaped.paint(at(s.col, row as u16), cell.height, window, cx);
        }
    }

    if let Some((b, focused, style, color)) = cursor_rect {
        match (focused, style) {
            (true, CursorVisualStyle::Block) => {}
            (true, CursorVisualStyle::Bar) => {
                window.paint_quad(fill(Bounds::new(b.origin, size(px(2.), b.size.height)), color))
            }
            (true, CursorVisualStyle::Underline) => window.paint_quad(fill(
                Bounds::new(b.origin + point(px(0.), b.size.height - px(2.)), size(b.size.width, px(2.))),
                color,
            )),
            _ => window.paint_quad(gpui::outline(b, color, gpui::BorderStyle::Solid)),
        }
    }
}
