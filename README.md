<img src="assets/icon.svg" width="64" height="64" alt="">

# tent

A small, pretty telnet and serial terminal for Linux and Windows. UI in [GPUI](https://www.gpui.rs) (Zed's framework), terminal emulation by
[libghostty-vt](https://crates.io/crates/libghostty-vt) (Ghostty's VT core).

- Full xterm-256color/truecolor terminal: vim, htop, ANSI art, wide chars, 10k lines of scrollback
- Telnet options: ECHO, SGA, BINARY, TTYPE, NAWS (window size follows the window)
- Line mode with local echo for servers that don't echo (MUDs, old BBSes)
- Serial ports (USB adapters, COM ports) with baud/framing, auto-listed in the picker
- Recent hosts, `telnet://` link handler

## Install

**Windows:** download `tent-setup.exe` from [Releases](../../releases) and double-click it. It installs for your user only (no admin) to `%LOCALAPPDATA%\Programs\tent`, adds a Start menu entry and a `telnet:` handler. Remove it from Settings → Apps. Silent: `tent-setup.exe --silent`, `--uninstall`.

**Linux x86_64:** download `tent-setup` and run it. It's a single static executable with tent inside; no network needed.

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
tent /dev/ttyUSB0            # serial, 115200 8N1
tent COM3 9600 7E1           # baud and framing in any order
```

On Linux, serial ports need the device's group (`uucp` on Arch, `dialout` on Debian/Ubuntu); tent tells you which.

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

Windows builds run in CI (`.github/workflows/windows.yml`): gpui compiles its shaders with `fxc.exe` at build time, so release builds need a Windows host.

`patches/xattr` works around `xattr 0.2.3` (pulled in by gpui) not building against current `libc`.

Font: JetBrains Mono (OFL, `assets/fonts/OFL.txt`).
