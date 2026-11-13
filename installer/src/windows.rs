//! Windows installer: per-user, no admin rights needed.
//!
//! - `%LOCALAPPDATA%\Programs\tent\tent.exe` (+ `uninstall.exe`, a copy of this setup)
//! - Start menu shortcut
//! - "Apps & features" entry, `telnet:` URL handler, App Paths (Win+R `tent`)

use std::env;
use std::fs;
use std::io;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, IPersistFile,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegGetValueW, RegSetValueExW,
};
use windows::Win32::UI::Shell::{IShellLinkW, SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify, ShellLink};
use windows::Win32::UI::WindowsAndMessaging::{
    IDYES, MB_ICONERROR, MB_ICONINFORMATION, MB_ICONQUESTION, MB_OK, MB_YESNO, MESSAGEBOX_STYLE, MessageBoxW,
};
use windows::core::{HSTRING, Interface, PCWSTR, w};

use crate::{VERSION, unpack};

const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\tent";
const APP_PATHS_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\App Paths\tent.exe";
const URL_KEY: &str = r"Software\Classes\telnet";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

struct Ui {
    silent: bool,
}

impl Ui {
    fn ask(&self, text: &str) -> bool {
        self.silent || self.msg(text, MB_YESNO | MB_ICONQUESTION) == IDYES.0
    }

    fn info(&self, text: &str) {
        if !self.silent {
            self.msg(text, MB_OK | MB_ICONINFORMATION);
        }
    }

    fn fail(&self, text: &str) -> ! {
        if !self.silent {
            self.msg(text, MB_OK | MB_ICONERROR);
        }
        std::process::exit(1);
    }

    fn msg(&self, text: &str, style: MESSAGEBOX_STYLE) -> i32 {
        unsafe { MessageBoxW(None, &HSTRING::from(text), w!("tent setup"), style).0 }
    }
}

pub fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let flag = |names: &[&str]| args.iter().any(|a| names.iter().any(|n| a.eq_ignore_ascii_case(n)));
    let ui = Ui { silent: flag(&["--silent", "/S", "-s"]) };
    if flag(&["-h", "--help", "/?"]) {
        ui.info(&format!(
            "tent-setup {VERSION}\n\n\
             tent-setup [--silent]\t\tinstall for this user\n\
             tent-setup --uninstall [--silent]\tremove\n\n\
             Installs to %LOCALAPPDATA%\\Programs\\tent."
        ));
        return;
    }
    if let Some(bad) = args.iter().find(|a| {
        !["--silent", "/s", "-s", "--uninstall", "-u", "/u", "--confirmed"].iter().any(|n| a.eq_ignore_ascii_case(n))
    }) {
        ui.fail(&format!("Unknown option {bad:?}. Try --help."));
    }
    let Some(dir) = env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join(r"Programs\tent")) else {
        ui.fail("LOCALAPPDATA is not set.");
    };
    if flag(&["--uninstall", "-u", "/u"]) { uninstall(&ui, &dir, flag(&["--confirmed"])) } else { install(&ui, &dir) }
}

fn shortcut_path() -> Option<PathBuf> {
    env::var_os("APPDATA").map(|p| PathBuf::from(p).join(r"Microsoft\Windows\Start Menu\Programs\tent.lnk"))
}

fn install(ui: &Ui, dir: &Path) {
    if !ui.ask(&format!(
        "Install tent {VERSION}?\n\nIt goes to {} and gets a Start menu shortcut. No admin rights needed.",
        dir.display()
    )) {
        return;
    }
    let exe = dir.join("tent.exe");
    let result = (|| -> io::Result<()> {
        let bin = unpack()?;
        fs::create_dir_all(dir)?;
        replace_file(&exe, &bin)?;
        // The uninstaller is this setup program; it drops its payload use at runtime.
        let me = fs::read(env::current_exe()?)?;
        replace_file(&dir.join("uninstall.exe"), &me)?;
        if let Some(lnk) = shortcut_path() {
            create_shortcut(&lnk, &exe).map_err(|e| io::Error::other(format!("Start menu shortcut: {e}")))?;
        }
        register(dir, &exe, (bin.len() + me.len()) as u32 / 1024)
            .map_err(|e| io::Error::other(format!("registry: {e}")))?;
        Ok(())
    })();
    if let Err(e) = result {
        ui.fail(&format!("Couldn't install tent:\n\n{e}"));
    }
    if !ui.silent && ui.msg("tent is installed. Open it now?", MB_YESNO | MB_ICONINFORMATION) == IDYES.0 {
        let _ = Command::new(&exe).current_dir(dir).spawn();
    }
}

/// Replace a file even while the old one is running: Windows lets you rename
/// a running .exe, just not overwrite it.
fn replace_file(path: &Path, data: &[u8]) -> io::Result<()> {
    let ctx = |e: io::Error| io::Error::new(e.kind(), format!("{}: {e}", path.display()));
    let new = path.with_extension("new");
    let old = path.with_extension("old");
    fs::write(&new, data).map_err(ctx)?;
    let _ = fs::remove_file(&old);
    if path.exists() {
        fs::rename(path, &old).map_err(ctx)?;
    }
    fs::rename(&new, path).map_err(ctx)?;
    let _ = fs::remove_file(&old); // fails if the old one is still running; cleaned up next time
    Ok(())
}

fn create_shortcut(lnk: &Path, exe: &Path) -> windows::core::Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(&HSTRING::from(exe.as_os_str()))?;
        link.SetDescription(w!("Telnet & serial terminal"))?;
        if let Some(d) = exe.parent() {
            link.SetWorkingDirectory(&HSTRING::from(d.as_os_str()))?;
        }
        if let Some(d) = lnk.parent() {
            let _ = fs::create_dir_all(d);
        }
        link.cast::<IPersistFile>()?.Save(&HSTRING::from(lnk.as_os_str()), true)
    }
}

fn register(dir: &Path, exe: &Path, size_kb: u32) -> windows::core::Result<()> {
    let exe_s = exe.display().to_string();
    let uninstaller = dir.join("uninstall.exe").display().to_string();

    let k = Key::create(UNINSTALL_KEY)?;
    k.set("DisplayName", "tent")?;
    k.set("DisplayVersion", VERSION)?;
    k.set("Publisher", "tent")?;
    k.set("DisplayIcon", &exe_s)?;
    k.set("InstallLocation", &dir.display().to_string())?;
    k.set("UninstallString", &format!("\"{uninstaller}\" --uninstall"))?;
    k.set("QuietUninstallString", &format!("\"{uninstaller}\" --uninstall --silent"))?;
    k.set_dword("NoModify", 1)?;
    k.set_dword("NoRepair", 1)?;
    k.set_dword("EstimatedSize", size_kb)?;

    let k = Key::create(APP_PATHS_KEY)?;
    k.set("", &exe_s)?;

    let k = Key::create(URL_KEY)?;
    k.set("", "URL:Telnet Protocol")?;
    k.set("URL Protocol", "")?;
    Key::create(&format!(r"{URL_KEY}\DefaultIcon"))?.set("", &format!("\"{exe_s}\",0"))?;
    Key::create(&format!(r"{URL_KEY}\shell\open\command"))?.set("", &url_command(&exe_s))?;
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
    Ok(())
}

fn url_command(exe: &str) -> String {
    format!("\"{exe}\" \"%1\"")
}

fn uninstall(ui: &Ui, dir: &Path, confirmed: bool) {
    let me = env::current_exe().unwrap_or_default();
    if !confirmed && !ui.ask("Remove tent?") {
        return;
    }
    // A running exe can't delete itself: hand off to a temp copy and exit so
    // it can remove the install dir, including this uninstall.exe.
    if me.parent() == Some(dir) {
        let tmp = env::temp_dir().join(format!("tent-uninstall-{}.exe", std::process::id()));
        if fs::copy(&me, &tmp).is_ok() {
            let mut cmd = Command::new(&tmp);
            cmd.args(["--uninstall", "--confirmed"]);
            if ui.silent {
                cmd.arg("--silent");
            }
            if cmd.spawn().is_ok() {
                return;
            }
        }
        // Couldn't hand off: remove what we can from here.
    }

    let exe = dir.join("tent.exe");
    let mut errors = Vec::new();
    // tent.exe first: if tent is running this fails and we stop before
    // leaving a half-removed install behind.
    match fs::remove_file(&exe) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => ui.fail(&format!("Couldn't remove {}: {e}\n\nIs tent still running?", exe.display())),
    }
    if let Some(lnk) = shortcut_path() {
        let _ = fs::remove_file(lnk);
    }
    for key in [UNINSTALL_KEY, APP_PATHS_KEY] {
        if let Err(e) = Key::delete(key) {
            errors.push(format!("{key}: {e}"));
        }
    }
    // Only drop the telnet: handler if it still points at us.
    let ours = Key::get(&format!(r"{URL_KEY}\shell\open\command"), "").is_some_and(|v| v == url_command(&exe.display().to_string()));
    if ours && let Err(e) = Key::delete(URL_KEY) {
        errors.push(format!("{URL_KEY}: {e}"));
    }
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };

    // uninstall.exe may still be exiting (it launched us); retry briefly.
    for _ in 0..50 {
        match fs::remove_dir_all(dir) {
            Ok(()) => break,
            Err(e) if e.kind() == io::ErrorKind::NotFound => break,
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(100)),
        }
    }
    // Delete this temp copy after we exit.
    if me.parent() != Some(dir) && me.starts_with(env::temp_dir()) {
        let _ = Command::new("cmd")
            .args(["/C", "ping -n 3 127.0.0.1 >NUL & del /F /Q"])
            .arg(&me)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
    if !errors.is_empty() {
        ui.fail(&format!("tent was removed, with errors:\n\n{}", errors.join("\n")));
    }
    ui.info("tent was removed.");
}

/// A key under HKEY_CURRENT_USER.
struct Key(HKEY);

impl Key {
    fn create(path: &str) -> windows::core::Result<Key> {
        let mut h = HKEY::default();
        unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                &HSTRING::from(path),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_READ | KEY_WRITE,
                None,
                &mut h,
                None,
            )
            .ok()?;
        }
        Ok(Key(h))
    }

    fn set(&self, name: &str, value: &str) -> windows::core::Result<()> {
        let wide: Vec<u16> = value.encode_utf16().chain([0]).collect();
        let bytes = unsafe { std::slice::from_raw_parts(wide.as_ptr().cast::<u8>(), wide.len() * 2) };
        unsafe { RegSetValueExW(self.0, &HSTRING::from(name), None, REG_SZ, Some(bytes)).ok() }
    }

    fn set_dword(&self, name: &str, value: u32) -> windows::core::Result<()> {
        unsafe { RegSetValueExW(self.0, &HSTRING::from(name), None, REG_DWORD, Some(&value.to_le_bytes())).ok() }
    }

    fn get(path: &str, name: &str) -> Option<String> {
        let mut buf = vec![0u16; 1024];
        let mut len = (buf.len() * 2) as u32;
        let r = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                &HSTRING::from(path),
                &HSTRING::from(name),
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr().cast()),
                Some(&mut len),
            )
        };
        r.is_ok().then(|| String::from_utf16_lossy(&buf[..(len as usize / 2).saturating_sub(1)]))
    }

    fn delete(path: &str) -> windows::core::Result<()> {
        let r = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(path)) };
        if r == ERROR_FILE_NOT_FOUND { Ok(()) } else { r.ok() }
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}
