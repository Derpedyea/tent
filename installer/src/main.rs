//! tent-setup: a single static executable that installs tent for the current
//! user without network access. `tent-setup --help` for options.

use std::env;
use std::fs;
use std::io::{self, Read};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const PAYLOAD: &[u8] = include_bytes!(env!("TENT_PAYLOAD"));
const ICON: &[u8] = include_bytes!("../../assets/icon.svg");
const VERSION: &str = env!("CARGO_PKG_VERSION");

struct Layout {
    bin: PathBuf,
    desktop: PathBuf,
    icon: PathBuf,
}

impl Layout {
    fn new(prefix: &Path) -> Self {
        Self {
            bin: prefix.join("bin/tent"),
            desktop: prefix.join("share/applications/tent.desktop"),
            icon: prefix.join("share/icons/hicolor/scalable/apps/tent.svg"),
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let mut prefix = None;
    let mut uninstall = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => return help(),
            "-V" | "--version" => return println!("tent-setup {VERSION}"),
            "-u" | "--uninstall" => uninstall = true,
            "--prefix" => match it.next() {
                Some(p) => prefix = Some(PathBuf::from(p)),
                None => fail("--prefix needs a directory"),
            },
            other => fail(&format!("unknown option {other:?} (try --help)")),
        }
    }
    let prefix = prefix.unwrap_or_else(default_prefix);
    let prefix = if prefix.is_absolute() {
        prefix
    } else {
        env::current_dir().unwrap_or_default().join(prefix)
    };
    let layout = Layout::new(&prefix);
    let result = if uninstall { remove(&layout) } else { install(&layout) };
    if let Err(e) = result {
        fail(&e.to_string());
    }
    refresh_desktop_db(&prefix);
}

fn help() {
    println!(
        "tent-setup {VERSION} — offline installer for tent, a telnet client\n\n\
         usage: tent-setup [--prefix DIR] [--uninstall]\n\n\
         Installs into ~/.local by default:\n  \
           bin/tent\n  \
           share/applications/tent.desktop\n  \
           share/icons/hicolor/scalable/apps/tent.svg"
    );
}

fn fail(msg: &str) -> ! {
    eprintln!("tent-setup: {msg}");
    std::process::exit(1);
}

fn default_prefix() -> PathBuf {
    match env::var_os("HOME") {
        Some(h) if !h.is_empty() => PathBuf::from(h).join(".local"),
        _ => fail("HOME is not set; pass --prefix"),
    }
}

fn install(l: &Layout) -> io::Result<()> {
    let mut bin = Vec::with_capacity(32 << 20);
    ruzstd::decoding::StreamingDecoder::new(PAYLOAD)
        .map_err(|e| io::Error::other(format!("corrupt payload: {e}")))?
        .read_to_end(&mut bin)?;

    // Write to a temp file and rename, so a running tent isn't truncated.
    write_atomic(&l.bin, &bin, 0o755)?;
    write_atomic(&l.icon, ICON, 0o644)?;
    let desktop = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=tent\n\
         GenericName=Telnet Client\n\
         Comment=A small, pretty telnet client\n\
         Exec={} %u\n\
         Icon=tent\n\
         Terminal=false\n\
         Categories=Network;RemoteAccess;\n\
         Keywords=telnet;mud;bbs;terminal;\n\
         MimeType=x-scheme-handler/telnet;\n\
         StartupWMClass=tent\n",
        desktop_quote(&l.bin)
    );
    write_atomic(&l.desktop, desktop.as_bytes(), 0o644)?;

    println!("installed tent {VERSION}");
    for p in [&l.bin, &l.desktop, &l.icon] {
        println!("  {}", p.display());
    }
    if let Some(dir) = l.bin.parent()
        && !on_path(dir)
    {
        println!("note: {} is not on your PATH", dir.display());
    }
    let missing = missing_libs();
    if !missing.is_empty() {
        println!("warning: tent needs these system libraries, which weren't found: {}", missing.join(", "));
    }
    Ok(())
}

fn remove(l: &Layout) -> io::Result<()> {
    let mut removed = 0;
    for p in [&l.bin, &l.desktop, &l.icon] {
        match fs::remove_file(p) {
            Ok(()) => {
                println!("removed {}", p.display());
                removed += 1;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(io::Error::new(e.kind(), format!("{}: {e}", p.display()))),
        }
    }
    if removed == 0 {
        println!("tent is not installed under this prefix");
    }
    Ok(())
}

fn write_atomic(path: &Path, data: &[u8], mode: u32) -> io::Result<()> {
    let ctx = |e: io::Error| io::Error::new(e.kind(), format!("{}: {e}", path.display()));
    let dir = path.parent().expect("absolute path");
    fs::create_dir_all(dir).map_err(ctx)?;
    let tmp = dir.join(format!(".{}.tent-setup", path.file_name().unwrap().to_string_lossy()));
    fs::write(&tmp, data).map_err(ctx)?;
    fs::set_permissions(&tmp, fs::Permissions::from_mode(mode)).map_err(ctx)?;
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        ctx(e)
    })
}

/// Quote an Exec path per the Desktop Entry spec when it has reserved characters.
fn desktop_quote(p: &Path) -> String {
    let s = p.to_string_lossy();
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c)) {
        return s.into_owned();
    }
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' | '`' | '$' | '\\' => {
                out.push_str("\\\\");
                out.push(c);
            }
            '%' => out.push_str("%%"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

fn on_path(dir: &Path) -> bool {
    env::var_os("PATH").is_some_and(|p| env::split_paths(&p).any(|d| d == dir))
}

/// Best-effort check for the shared libraries tent links against.
fn missing_libs() -> Vec<&'static str> {
    const LIBS: [&str; 4] = ["libvulkan.so.1", "libxkbcommon.so.0", "libwayland-client.so.0", "libfontconfig.so.1"];
    const DIRS: [&str; 5] = ["/usr/lib", "/usr/lib64", "/lib/x86_64-linux-gnu", "/usr/lib/x86_64-linux-gnu", "/lib64"];
    LIBS.into_iter().filter(|lib| !DIRS.iter().any(|d| Path::new(d).join(lib).exists())).collect()
}

fn refresh_desktop_db(prefix: &Path) {
    let apps = prefix.join("share/applications");
    let _ = Command::new("update-desktop-database").arg(&apps).output();
    let icons = prefix.join("share/icons/hicolor");
    let _ = Command::new("gtk-update-icon-cache").args(["-q", "-t", "-f"]).arg(&icons).output();
}
