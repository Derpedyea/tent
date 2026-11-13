//! tent — a small telnet and serial terminal. GPUI for the UI, Ghostty's VT core for the terminal.

mod connect;
mod net;
mod session;
mod target;
mod telnet;
mod theme;

use std::borrow::Cow;

use gpui::{
    App, AppContext, Application, Bounds, Context, Entity, Focusable, IntoElement, ParentElement,
    Render, Styled, Subscription, TitlebarOptions, Window, WindowBounds, WindowDecorations,
    WindowOptions, div, px, size,
};

use connect::{Connect, ConnectEvent};
use session::{Session, SessionEvent};
use target::Target;

enum Screen {
    Connect,
    Session { view: Entity<Session>, _sub: Subscription },
}

struct Root {
    connect: Entity<Connect>,
    screen: Screen,
    _sub: Subscription,
}

impl Root {
    fn new(target: Option<Target>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let connect = cx.new(Connect::new);
        let sub = cx.subscribe_in(&connect, window, |this, _, ev: &ConnectEvent, window, cx| match ev {
            ConnectEvent::Open(t) => this.open(t.clone(), window, cx),
        });
        let mut this = Self { connect, screen: Screen::Connect, _sub: sub };
        match target {
            Some(t) => this.open(t, window, cx),
            None => this.show_connect(window, cx),
        }
        this
    }

    fn open(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        let session = cx.new(|cx| Session::new(target, window, cx));
        let sub = cx.subscribe_in(&session, window, |this, s, ev: &SessionEvent, window, cx| match ev {
            SessionEvent::Connected => {
                let t = s.read(cx).target.clone();
                this.connect.update(cx, |c, _| c.remember(&t));
            }
            SessionEvent::Back => this.show_connect(window, cx),
        });
        self.screen = Screen::Session { view: session, _sub: sub };
        cx.notify();
    }

    fn show_connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Dropping the session closes its socket.
        self.screen = Screen::Connect;
        window.set_window_title("tent");
        window.focus(&self.connect.focus_handle(cx));
        cx.notify();
    }
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let body = div().size_full().font_family(theme::FONT).bg(theme::c(theme::BG));
        match &self.screen {
            Screen::Connect => body.child(self.connect.clone()),
            Screen::Session { view, .. } => body.child(view.clone()),
        }
    }
}

const FONTS: [&[u8]; 4] = [
    include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Bold.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Italic.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-BoldItalic.ttf"),
];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("usage: tent [host[:port] | telnet://host:port | <serial port> [baud] [8N1]]");
        return;
    }
    if args.iter().any(|a| a == "-V" || a == "--version") {
        println!("tent {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let target = match args.join(" ").trim() {
        "" => None,
        t => match target::parse_target(t) {
            Ok(t) => Some(t),
            Err(e) => {
                eprintln!("tent: {e}");
                std::process::exit(2);
            }
        },
    };

    Application::new().run(move |cx: &mut App| {
        cx.text_system()
            .add_fonts(FONTS.iter().map(|f| Cow::Borrowed(*f)).collect())
            .expect("load bundled fonts");
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let bounds = Bounds::centered(None, size(px(960.), px(620.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions { title: Some("tent".into()), ..Default::default() }),
                app_id: Some("tent".into()),
                window_min_size: Some(size(px(360.), px(240.))),
                window_decorations: Some(WindowDecorations::Server),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| Root::new(target, window, cx)),
        )
        .expect("open window");
        cx.activate(true);
    });
}
