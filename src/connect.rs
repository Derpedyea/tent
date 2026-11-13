//! The start screen: one target field, recently used targets, detected serial ports.

use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    App, Context, EventEmitter, FocusHandle, ScrollHandle, Task, Focusable, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Render, Styled, Window, div, prelude::*, px,
};

use crate::session::hint;
use crate::target::{SerialCfg, Target, parse_target};
use crate::theme::{self, c};

const MAX_RECENT: usize = 8;

#[cfg(windows)]
const PLACEHOLDER: &str = "host:port or COM3 9600";
#[cfg(not(windows))]
const PLACEHOLDER: &str = "host:port or /dev/ttyUSB0 9600";

pub enum ConnectEvent {
    Open(Target),
}

impl EventEmitter<ConnectEvent> for Connect {}

pub struct Connect {
    focus: FocusHandle,
    input: String,
    recent: Vec<Target>,
    /// Detected serial ports that aren't already in `recent`.
    ports: Vec<String>,
    /// Highlighted entry (arrow keys) across recent + ports, if any.
    selected: Option<usize>,
    error: Option<String>,
    scroll: ScrollHandle,
    _poll: Task<()>,
}

impl Connect {
    pub fn new(cx: &mut Context<Self>) -> Self {
        // Serial ports come and go (USB adapters); rescan every couple of seconds.
        let poll = cx.spawn(async move |this, cx| {
            loop {
                let found = cx.background_executor().spawn(async { crate::net::serial_ports() }).await;
                if this.update(cx, |c, cx| c.set_ports(found, cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(Duration::from_secs(2)).await;
            }
        });
        Self {
            focus: cx.focus_handle(),
            input: String::new(),
            recent: load_recent(),
            ports: Vec::new(),
            selected: None,
            error: None,
            scroll: ScrollHandle::new(),
            _poll: poll,
        }
    }

    fn set_ports(&mut self, found: Vec<String>, cx: &mut Context<Self>) {
        let known = |p: &String| self.recent.iter().any(|t| matches!(t, Target::Serial(s) if &s.path == p));
        let ports: Vec<String> = found.into_iter().filter(|p| !known(p)).collect();
        if ports != self.ports {
            self.ports = ports;
            self.selected = self.selected.filter(|&i| i < self.len());
            cx.notify();
        }
    }

    /// Scroll the highlighted entry into view. Children of the list are the
    /// "Recent" header, recent rows, the "Serial ports" header, port rows.
    fn reveal(&self) {
        match self.selected {
            Some(i) => {
                let headers = if self.recent.is_empty() { 1 } else if i < self.recent.len() { 1 } else { 2 };
                self.scroll.scroll_to_item(i + headers);
            }
            None => self.scroll.scroll_to_item(0),
        }
    }

    fn len(&self) -> usize {
        self.recent.len() + self.ports.len()
    }

    fn item(&self, i: usize) -> Option<Target> {
        self.recent.get(i).cloned().or_else(|| {
            self.ports.get(i - self.recent.len()).map(|p| Target::Serial(SerialCfg::new(p.clone())))
        })
    }

    pub fn remember(&mut self, t: &Target) {
        self.recent.retain(|r| r != t);
        self.recent.insert(0, t.clone());
        self.recent.truncate(MAX_RECENT);
        if let Target::Serial(s) = t {
            self.ports.retain(|p| p != &s.path);
        }
        save_recent(&self.recent);
    }

    /// Recents are only saved once the session actually connects.
    fn open(&mut self, t: Target, cx: &mut Context<Self>) {
        self.input.clear();
        self.selected = None;
        self.error = None;
        cx.emit(ConnectEvent::Open(t));
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        // Typing clears the selection, so a selection always wins.
        let target = match self.selected {
            Some(i) => self.item(i),
            None => match parse_target(&self.input) {
                Ok(t) => Some(t),
                Err(e) => {
                    self.error = Some(e);
                    return cx.notify();
                }
            },
        };
        if let Some(t) = target {
            self.open(t, cx);
        }
    }

    fn on_key(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let m = &ks.modifiers;
        match ks.key.as_str() {
            "enter" => return self.submit(cx),
            "backspace" if m.control => {
                let trimmed = self.input.trim_end().len();
                let cut = self.input[..trimmed].rfind([' ', ':', '.']).map_or(0, |i| i + 1);
                self.input.truncate(cut);
            }
            "backspace" => {
                self.input.pop();
            }
            "u" if m.control => self.input.clear(),
            "v" if m.control => {
                if let Some(t) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    self.input.push_str(t.lines().next().unwrap_or("").trim());
                }
            }
            "escape" => {
                self.input.clear();
                self.selected = None;
            }
            // Tab copies the highlighted entry into the field, e.g. to change the baud rate.
            "tab" => match self.selected.and_then(|i| self.item(i)) {
                Some(t) => self.input = t.to_string(),
                None => return,
            },
            "down" if self.len() > 0 => {
                self.selected = Some(self.selected.map_or(0, |i| (i + 1).min(self.len() - 1)));
                self.reveal();
                return cx.notify();
            }
            "up" => {
                self.selected = self.selected.and_then(|i| i.checked_sub(1));
                self.reveal();
                return cx.notify();
            }
            "delete" if self.selected.is_some_and(|i| i < self.recent.len()) => {
                let i = self.selected.unwrap();
                self.recent.remove(i);
                save_recent(&self.recent);
                self.selected = if self.recent.is_empty() { None } else { Some(i.min(self.recent.len() - 1)) };
            }
            _ if m.control || m.alt || m.platform => return,
            _ => match &ks.key_char {
                Some(t) if !t.chars().any(char::is_control) => self.input.push_str(t),
                _ => return,
            },
        }
        // Typing deselects the recent list.
        if ks.key != "delete" {
            self.selected = None;
        }
        self.error = None;
        cx.notify();
    }
}

fn recent_path() -> Option<PathBuf> {
    #[cfg(windows)]
    let base = std::env::var_os("APPDATA").map(PathBuf::from)?;
    #[cfg(not(windows))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("tent").join("recent"))
}

fn load_recent() -> Vec<Target> {
    let Some(text) = recent_path().and_then(|p| std::fs::read_to_string(p).ok()) else { return Vec::new() };
    text.lines().filter_map(|l| parse_target(l).ok()).take(MAX_RECENT).collect()
}

fn save_recent(list: &[Target]) {
    let Some(path) = recent_path() else { return };
    let body: String = list.iter().map(|t| format!("{t}\n")).collect();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, body);
}

impl Focusable for Connect {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Connect {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus.is_focused(window);
        let empty = self.input.is_empty();

        let field = div()
            .flex()
            .items_center()
            .h(px(44.))
            .px_3()
            .rounded_lg()
            .bg(c(theme::SURFACE))
            .border_1()
            .border_color(c(if focused { theme::ACCENT } else { theme::BORDER }))
            .text_size(px(15.))
            .child(div().text_color(c(theme::MUTED)).mr_2().child("❯"))
            .when(!empty, |d| d.child(div().text_color(c(theme::FG)).child(self.input.clone())))
            .when(focused, |d| d.child(div().w(px(2.)).h(px(18.)).bg(c(theme::ACCENT))))
            .when(empty, |d| d.child(div().ml_1().text_color(c(theme::MUTED)).child(PLACEHOLDER)));

        let row = |i: usize, t: Target, name: String, detail: Option<String>, cx: &mut Context<Self>| {
            let sel = self.selected == Some(i);
            div()
                .id(i)
                .flex()
                .justify_between()
                .px_3()
                .py_1p5()
                .rounded_md()
                .cursor_pointer()
                .text_size(px(13.))
                .text_color(c(if sel { theme::FG } else { theme::MUTED }))
                .when(sel, |d| d.bg(c(theme::SURFACE_HI)))
                .hover(|s| s.bg(c(theme::SURFACE)).text_color(c(theme::FG)))
                .child(name)
                .when_some(detail, |d, x| d.child(div().text_color(c(theme::MUTED)).child(x)))
                .on_click(cx.listener(move |this, _, _, cx| this.open(t.clone(), cx)))
        };
        let recent: Vec<_> = self
            .recent
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let (name, detail) = match t {
                    Target::Tcp { host, port } => (host.clone(), (*port != 23).then(|| port.to_string())),
                    Target::Serial(s) => (s.path.clone(), Some(s.settings())),
                };
                row(i, t.clone(), name, detail, cx)
            })
            .collect();
        let ports: Vec<_> = self
            .ports
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let t = Target::Serial(SerialCfg::new(p.clone()));
                row(self.recent.len() + i, t, p.clone(), None, cx)
            })
            .collect();
        let header = |title: &'static str| div().px_3().pb_1().text_size(px(12.)).text_color(c(theme::MUTED)).child(title);
        let (has_recent, has_ports) = (!recent.is_empty(), !ports.is_empty());
        // One flat scroll list so `reveal` can address rows by child index.
        let list = div()
            .id("list")
            .track_scroll(&self.scroll)
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_0p5()
            .when(has_recent, |d| d.child(header("Recent")))
            .children(recent)
            .when(has_ports, |d| d.child(header("Serial ports").when(has_recent, |h| h.pt_5())))
            .children(ports);
        let has_items = self.len() > 0;

        div()
            .key_context("Connect")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .size_full()
            .bg(c(theme::BG))
            .flex()
            .flex_col()
            .items_center()
            .pt(px(110.))
            .pb_6()
            .child(
                div()
                    .w(px(400.))
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap_6()
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap_3()
                            .child(div().text_size(px(30.)).font_weight(gpui::FontWeight::BOLD).text_color(c(theme::ACCENT)).child("tent"))
                            .child(div().text_size(px(13.)).text_color(c(theme::MUTED)).child("telnet & serial")),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(field)
                            .child(
                                div()
                                    .h(px(16.))
                                    .px_1()
                                    .text_size(px(12.))
                                    .when_some(self.error.clone(), |d, e| d.text_color(c(theme::ERR)).child(e))
                                    .when(self.error.is_none(), |d| {
                                        d.flex()
                                            .gap_3()
                                            .text_color(c(theme::MUTED))
                                            .child(hint("↵", "connect"))
                                            .when(has_items, |d| d.child(hint("↑↓", "select")).child(hint("tab", "edit")))
                                            .when(!self.recent.is_empty(), |d| d.child(hint("del", "forget")))
                                    }),
                            ),
                    )
                    .child(list),
            )
    }
}
