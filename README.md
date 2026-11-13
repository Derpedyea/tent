<img src="assets/icon.svg" width="64" height="64" alt="">

# tent

A small, pretty telnet client. UI in [GPUI](https://www.gpui.rs) (Zed's framework), terminal emulation by
[libghostty-vt](https://crates.io/crates/libghostty-vt) (Ghostty's VT core).

- Full xterm-256color/truecolor terminal: vim, htop, ANSI art, wide chars, 10k lines of scrollback
- Telnet options: ECHO, SGA, BINARY, TTYPE, NAWS (window size follows the window)
- Line mode with local echo for servers that don't echo (MUDs, old BBSes)
- Recent hosts, `telnet://` link handler

## Install (Linux x86_64)

Download `tent-setup` and run it. It's a single static executable with tent inside; no network needed.

```sh
./tent-setup                 # installs to ~/.local (bin, app menu entry, icon)
./tent-setup --uninstall
./tent-setup --prefix DIR    # elsewhere
```

Runtime needs Vulkan, Wayland or X11, xkbcommon and fontconfig (any desktop distro has these).

## Use

```sh
tent                         # host picker
tent towel.example.org       # port 23
tent host:4000  |  tent host 4000  |  tent telnet://host:4000
```

| Keys | |
|---|---|
| Ctrl+Shift+V | paste |
| Shift+PgUp / PgDn, wheel | scrollback |
| Ctrl+= / Ctrl+- / Ctrl+0 | font size |
| Enter / Esc (after disconnect) | reconnect / back to hosts |

## Build

Needs Rust, [Zig 0.15.x](https://ziglang.org/download/) on `PATH` (libghostty-vt builds Ghostty from source), and `zstd`.

```sh
cargo run -- host            # dev
./dist.sh                    # → dist/tent-setup (needs: rustup target add x86_64-unknown-linux-musl)
```

`patches/xattr` works around `xattr 0.2.3` (pulled in by gpui) not building against current `libc`.

Font: JetBrains Mono (OFL, `assets/fonts/OFL.txt`).
