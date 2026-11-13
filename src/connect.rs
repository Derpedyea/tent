//! The start screen: one host field plus recently used hosts.

use std::path::PathBuf;

use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Render, Styled, Window, div, prelude::*, px,
};

use crate::session::hint;
use crate::theme::{self, c};

const MAX_RECENT: usize = 8;

pub enum ConnectEvent {
    Open(String, u16),
}

impl EventEmitter<ConnectEvent> for Connect {}

pub struct Connect {
    focus: FocusHandle,
    input: String,
    recent: Vec<(String, u16)>,
    /// Highlighted recent entry (arrow keys), if any.
    selected: Option<usize>,
    error: Option<String>,
}

impl Connect {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self { focus: cx.focus_handle(), input: String::new(), recent: load_recent(), selected: None, error: None }
    }

    pub fn remember(&mut self, host: &str, port: u16) {
        self.recent.retain(|(h, p)| !(h == host && *p == port));
        self.recent.insert(0, (host.to_string(), port));
        self.recent.truncate(MAX_RECENT);
        save_recent(&self.recent);
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        // Typing clears the selection, so a selection always wins.
        let target = match self.selected {
            Some(i) => self.recent.get(i).cloned(),
            None => match parse_target(&self.input) {
                Ok(t) => Some(t),
                Err(e) => {
                    self.error = Some(e);
                    return cx.notify();
                }
            },
        };
        if let Some((host, port)) = target {
            self.remember(&host, port);
            self.input.clear();
            self.selected = None;
            self.error = None;
            cx.emit(ConnectEvent::Open(host, port));
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
            "down" if !self.recent.is_empty() => {
                self.selected = Some(self.selected.map_or(0, |i| (i + 1).min(self.recent.len() - 1)));
                return cx.notify();
            }
            "up" => {
                self.selected = self.selected.and_then(|i| i.checked_sub(1));
                return cx.notify();
            }
            "delete" if self.selected.is_some() => {
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

/// Accepts `host`, `host:port`, `host port`, `[v6addr]:port` and `telnet://host:port/`.
pub fn parse_target(s: &str) -> Result<(String, u16), String> {
    let s = s.trim();
    let s = s.strip_prefix("telnet://").unwrap_or(s).trim_end_matches('/');
    let s = s.rsplit_once('@').map_or(s, |(_, h)| h); // drop user@
    if s.is_empty() {
        return Err("enter a host".into());
    }
    let (host, port) = if let Some((h, p)) = s.split_once(char::is_whitespace) {
        (h, Some(p.trim()))
    } else if let Some(rest) = s.strip_prefix('[') {
        let (h, tail) = rest.split_once(']').ok_or("missing ]")?;
        (h, tail.strip_prefix(':'))
    } else if s.matches(':').count() == 1 {
        let (h, p) = s.split_once(':').unwrap();
        (h, Some(p))
    } else {
        (s, None) // bare host, or bare IPv6 address
    };
    if host.is_empty() || host.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("invalid host".into());
    }
    let port = match port {
        None | Some("") => 23,
        Some(p) => p.parse::<u16>().ok().filter(|&p| p != 0).ok_or(format!("invalid port “{p}”"))?,
    };
    Ok((host.to_string(), port))
}

fn recent_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("tent").join("recent"))
}

fn load_recent() -> Vec<(String, u16)> {
    let Some(text) = recent_path().and_then(|p| std::fs::read_to_string(p).ok()) else { return Vec::new() };
    text.lines().filter_map(|l| parse_target(l).ok()).take(MAX_RECENT).collect()
}

fn save_recent(list: &[(String, u16)]) {
    let Some(path) = recent_path() else { return };
    let body: String = list.iter().map(|(h, p)| format!("{} {p}\n", h)).collect();
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
            .when(empty, |d| d.child(div().ml_1().text_color(c(theme::MUTED)).child("host or host:port")));

        let recent = div().flex().flex_col().gap_0p5().children(self.recent.iter().enumerate().map(|(i, (h, p))| {
            let sel = self.selected == Some(i);
            let (host, port) = (h.clone(), *p);
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
                .child(host.clone())
                .when(port != 23, |d| d.child(div().text_color(c(theme::MUTED)).child(port.to_string())))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.remember(&host, port);
                    cx.emit(ConnectEvent::Open(host.clone(), port));
                }))
        }));

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
            .child(
                div()
                    .w(px(400.))
                    .flex()
                    .flex_col()
                    .gap_6()
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap_3()
                            .child(div().text_size(px(30.)).font_weight(gpui::FontWeight::BOLD).text_color(c(theme::ACCENT)).child("tent"))
                            .child(div().text_size(px(13.)).text_color(c(theme::MUTED)).child("telnet client")),
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
                                        d.flex().gap_3().text_color(c(theme::MUTED)).child(hint("↵", "connect")).when(
                                            !self.recent.is_empty(),
                                            |d| d.child(hint("↑↓", "recent")).child(hint("del", "forget")),
                                        )
                                    }),
                            ),
                    )
                    .when(!self.recent.is_empty(), |d| {
                        d.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(div().px_3().text_size(px(12.)).text_color(c(theme::MUTED)).child("Recent"))
                                .child(recent),
                        )
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::parse_target;

    #[test]
    fn parses_targets() {
        let ok = |s: &str| parse_target(s).unwrap();
        assert_eq!(ok("towel.blinkenlights.nl"), ("towel.blinkenlights.nl".into(), 23));
        assert_eq!(ok("  host:4000 "), ("host".into(), 4000));
        assert_eq!(ok("host 4000"), ("host".into(), 4000));
        assert_eq!(ok("telnet://user@host:2323/"), ("host".into(), 2323));
        assert_eq!(ok("[::1]:24"), ("::1".into(), 24));
        assert_eq!(ok("::1"), ("::1".into(), 23));
        assert_eq!(ok("host:"), ("host".into(), 23));
        for bad in ["", "   ", "host:99999", "host:0", "host:abc", "[::1", ":23"] {
            assert!(parse_target(bad).is_err(), "{bad:?} should fail");
        }
    }
}
