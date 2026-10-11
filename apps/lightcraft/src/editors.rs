//! Starting external editors for Photo ▸ Edit In: PhotoCraft, found where its installers put it,
//! or any other application the user named in Settings ▸ External Editing.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The name the UI asks for when it means PhotoCraft (`lightcraft_ui_egui::state::PHOTOCRAFT`).
const PHOTOCRAFT: &str = lightcraft_ui_egui::state::PHOTOCRAFT;
/// PhotoCraft's application id (Flatpak, desktop entry).
const PHOTOCRAFT_ID: &str = "ai.storyteller.photocraft";

/// How an editor is started.
#[derive(Debug, PartialEq)]
enum Launcher {
    /// The system's default application for the file.
    Default,
    /// A program file, given the photo as its argument.
    Program(PathBuf),
    /// macOS: `open -a <app>` (an application name or bundle path).
    MacApp(String),
    /// The PhotoCraft Flatpak, with the file forwarded into its sandbox.
    Flatpak,
}

/// Whether `app` can be started (Settings ▸ External Editing names it; "" = the system default):
/// for PhotoCraft, that it is installed. The error says what to do instead.
pub fn check(app: &str) -> Result<(), String> {
    launcher(app).map(|_| ())
}

/// Open `path` in `app` (see [`check`]). Spawned, never waited for: the editor runs alongside.
pub fn open_with(path: &str, app: &str) -> Result<(), String> {
    let mut c = match launcher(app)? {
        Launcher::Default if cfg!(target_os = "macos") => {
            let mut c = Command::new("open");
            c.arg(path);
            c
        }
        Launcher::Default if cfg!(target_os = "windows") => windows_default_command(path),
        Launcher::Default => {
            let mut c = Command::new("xdg-open");
            c.arg(path);
            c
        }
        Launcher::MacApp(app) => {
            let mut c = Command::new("open");
            c.args(["-a", &app, path]);
            c
        }
        Launcher::Program(p) => {
            let mut c = Command::new(p);
            c.arg(path);
            c
        }
        Launcher::Flatpak => {
            let mut c = Command::new("flatpak");
            c.args(["run", "--file-forwarding", PHOTOCRAFT_ID, "@@", path, "@@"]);
            c
        }
    };
    c.spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Windows: open `path` in its default application through Explorer, given the path as its one
/// argument. Not `cmd /C start`: cmd.exe would read `&`, `^`, `%` or `|` in a file name as its own
/// syntax (running what follows an `&` as a command).
fn windows_default_command(path: &str) -> Command {
    let mut c = Command::new("explorer.exe");
    c.arg(path.replace('/', "\\"));
    c
}

fn launcher(app: &str) -> Result<Launcher, String> {
    let app = app.trim();
    if app.is_empty() {
        return Ok(Launcher::Default);
    }
    if !app.eq_ignore_ascii_case(PHOTOCRAFT) {
        return Ok(if cfg!(target_os = "macos") { Launcher::MacApp(app.to_string()) } else { Launcher::Program(PathBuf::from(app)) });
    }
    find_photocraft().ok_or_else(|| {
        "PhotoCraft isn't installed. Get it from getartcraft.com/apps/photocraft, or give where it is in Settings ▸ External Editing.".to_string()
    })
}

fn find_photocraft() -> Option<Launcher> {
    let env = |k: &str| std::env::var_os(k).map(|v| v.to_string_lossy().to_string());
    if cfg!(target_os = "macos") {
        // LaunchServices finds the app wherever it is; these are only the usual places
        let home = env("HOME").map(|h| Path::new(&h).join("Applications/PhotoCraft.app"));
        let found = [Some(PathBuf::from("/Applications/PhotoCraft.app")), home].into_iter().flatten().find(|p| p.is_dir());
        return Some(Launcher::MacApp(found.map_or_else(|| PHOTOCRAFT.to_string(), |p| p.to_string_lossy().to_string())));
    }
    if cfg!(target_os = "windows") {
        let install_dir = reg_query(r"HKLM\Software\PhotoCraft", Some("InstallDir"));
        let app_path = reg_query(r"HKLM\Software\Microsoft\Windows\CurrentVersion\App Paths\photocraft.exe", None);
        return windows_candidates(install_dir, app_path, env).into_iter().find(|p| p.is_file()).map(Launcher::Program);
    }
    // Linux, FreeBSD: on the PATH (.deb, .rpm, tarball), a desktop entry (AppImage, which
    // registers one on first run), else the Flatpak
    let on_path =
        env("PATH").into_iter().flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).map(|d| d.join("photocraft")).find(|p| p.is_file());
    let from_entry = || {
        desktop_entry_dirs(env).into_iter().find_map(|d| {
            let text = std::fs::read_to_string(d.join(format!("{PHOTOCRAFT_ID}.desktop"))).ok()?;
            desktop_exec(&text).filter(|p| p.is_file())
        })
    };
    if let Some(p) = on_path.or_else(from_entry) {
        return Some(Launcher::Program(p));
    }
    let flatpak = Command::new("flatpak").args(["info", PHOTOCRAFT_ID]).output().is_ok_and(|o| o.status.success());
    flatpak.then_some(Launcher::Flatpak)
}

/// Where the Windows installers put `photocraft.exe`, most specific first: the folder the MSI
/// recorded (`HKLM\Software\PhotoCraft\InstallDir`), its App Paths entry, then the default
/// folders (per machine, then per user).
fn windows_candidates(install_dir: Option<String>, app_path: Option<String>, env: impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    out.extend(install_dir.map(|d| Path::new(&d).join("photocraft.exe")));
    out.extend(app_path.map(|p| PathBuf::from(p.trim_matches('"'))));
    for var in ["ProgramW6432", "ProgramFiles"] {
        out.extend(env(var).map(|d| Path::new(&d).join("PhotoCraft").join("photocraft.exe")));
    }
    out.extend(env("LOCALAPPDATA").map(|d| Path::new(&d).join("Programs").join("PhotoCraft").join("photocraft.exe")));
    out
}

/// A string value from the Windows registry (`None` = the key's default value).
///
/// Read through `reg export`: the file it writes is UTF-16 whatever the system's code page,
/// while `reg query` prints in the console's code page (GBK on a Chinese system, say), in
/// which a folder name outside ASCII does not survive being read as UTF-8.
fn reg_query(key: &str, name: Option<&str>) -> Option<String> {
    let reg = std::env::var_os("SystemRoot").map(|r| PathBuf::from(r).join("System32").join("reg.exe")).filter(|p| p.is_file());
    // (a name of its own each time: a file an earlier run could not delete must not be in the way)
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let file = std::env::temp_dir().join(format!("lightcraft-reg-{}-{stamp}.reg", std::process::id()));
    let mut c = Command::new(reg.as_deref().unwrap_or(Path::new("reg")));
    c.args(["export", key]).arg(&file).arg("/y");
    c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    let bytes = if finishes_in_time(&mut c, HELPER_DEADLINE) { std::fs::read(&file).ok() } else { None };
    let _ = std::fs::remove_file(&file);
    reg_value(&bytes?, name)
}

/// How long the registry helper may take. It is asked on the UI thread and answers in a few
/// tens of milliseconds; one that hangs (a temporary folder on a dead network share) is given
/// up on.
const HELPER_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);

/// Run `command` and say whether it succeeded within `deadline`; one still running then is
/// killed.
fn finishes_in_time(command: &mut Command, deadline: std::time::Duration) -> bool {
    let Ok(mut child) = command.spawn() else { return false };
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if started.elapsed() < deadline => std::thread::sleep(std::time::Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

/// The string value `name` (`None` = the key's default value, written `@`) in a `reg export`
/// file: UTF-16LE text with one `"name"="value"` line per value, `\` and `"` escaped with a
/// backslash. Other kinds of value (`hex(2):…`, `dword:…`) are not read.
fn reg_value(bytes: &[u8], name: Option<&str>) -> Option<String> {
    let units: Vec<u16> = bytes.chunks_exact(2).filter_map(|c| <[u8; 2]>::try_from(c).ok()).map(u16::from_le_bytes).collect();
    let text = String::from_utf16_lossy(&units);
    text.lines().map(|l| l.trim_start_matches('\u{feff}').trim()).find_map(|line| {
        let rest = match name {
            None => line.strip_prefix("@=")?,
            Some(name) => {
                let (key, rest) = line.strip_prefix('"')?.split_once("\"=")?;
                key.eq_ignore_ascii_case(name).then_some(rest)?
            }
        };
        let body = rest.strip_prefix('"')?.strip_suffix('"')?;
        let mut value = String::with_capacity(body.len());
        let mut chars = body.chars();
        while let Some(c) = chars.next() {
            value.push(if c == '\\' { chars.next()? } else { c });
        }
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

/// The folders desktop entries live in (XDG base directories).
fn desktop_entry_dirs(env: impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    match env("XDG_DATA_HOME") {
        Some(d) if !d.is_empty() => dirs.push(PathBuf::from(d)),
        _ => dirs.extend(env("HOME").map(|h| Path::new(&h).join(".local/share"))),
    }
    let system = env("XDG_DATA_DIRS").filter(|d| !d.is_empty()).unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    dirs.extend(system.split(':').filter(|d| !d.is_empty()).map(PathBuf::from));
    dirs.into_iter().map(|d| d.join("applications")).collect()
}

/// The program a desktop entry's `Exec=` line starts (quoted or not, its field codes dropped).
fn desktop_exec(entry: &str) -> Option<PathBuf> {
    let exec = entry.lines().find_map(|l| l.strip_prefix("Exec="))?.trim();
    let program = match exec.strip_prefix('"') {
        Some(rest) => rest.split('"').next()?,
        None => exec.split_whitespace().next()?,
    };
    // a bare name (`photocraft %F`) was already looked for on the PATH
    Some(PathBuf::from(program)).filter(|p| p.is_absolute())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file name with cmd.exe syntax in it goes to Explorer as one plain argument, not
    /// through a shell.
    #[test]
    fn windows_default_open_goes_through_explorer_not_cmd() {
        let path = r"C:/Edits/a & b ^c %PATH% | d.psd";
        let c = windows_default_command(path);
        assert_eq!(c.get_program(), "explorer.exe");
        let args: Vec<_> = c.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(args, [r"C:\Edits\a & b ^c %PATH% | d.psd"]);
    }

    #[test]
    fn windows_lookup_prefers_the_recorded_install_folder() {
        let env = |k: &str| match k {
            "ProgramFiles" => Some(r"C:\Program Files".to_string()),
            "LOCALAPPDATA" => Some(r"C:\Users\Simon\AppData\Local".to_string()),
            _ => None,
        };
        let c = windows_candidates(Some(r"D:\Apps\PhotoCraft\".into()), Some(r#""C:\Program Files\PhotoCraft\photocraft.exe""#.into()), env);
        let c: Vec<String> = c.iter().map(|p| p.to_string_lossy().replace('/', "\\")).collect();
        assert!(c[0].starts_with(r"D:\Apps\PhotoCraft") && c[0].ends_with("photocraft.exe"), "{c:?}");
        assert_eq!(c[1], r"C:\Program Files\PhotoCraft\photocraft.exe", "App Paths, quotes dropped");
        assert!(c[2].starts_with(r"C:\Program Files") && c[2].ends_with("photocraft.exe"), "{c:?}");
        assert!(c[3].contains("Programs") && c[3].ends_with("photocraft.exe"), "{c:?}");
        assert_eq!(c.len(), 4);
    }

    /// A `reg export` file as Windows writes it: UTF-16LE with a byte order mark and CRLF.
    fn reg_file(text: &str) -> Vec<u8> {
        let mut bytes = vec![0xff, 0xfe];
        bytes.extend(text.replace('\n', "\r\n").encode_utf16().flat_map(u16::to_le_bytes));
        bytes
    }

    #[test]
    fn a_reg_export_is_read() {
        let install = reg_file(
            "Windows Registry Editor Version 5.00\n\n[HKEY_LOCAL_MACHINE\\Software\\PhotoCraft]\n\"InstallDir\"=\"C:\\\\Program Files\\\\PhotoCraft\\\\\"\n\n",
        );
        assert_eq!(reg_value(&install, Some("InstallDir")).as_deref(), Some(r"C:\Program Files\PhotoCraft\"));
        assert_eq!(reg_value(&install, Some("installdir")).as_deref(), Some(r"C:\Program Files\PhotoCraft\"), "names ignore case");
        assert_eq!(reg_value(&install, Some("Version")), None);
        assert_eq!(reg_value(&install, None), None, "no default value");
        let key = "[HKEY_LOCAL_MACHINE\\Software\\Microsoft\\Windows\\CurrentVersion\\App Paths\\photocraft.exe]";
        let default =
            reg_file(&format!("{key}\n@=\"C:\\\\Program Files\\\\PhotoCraft\\\\photocraft.exe\"\n\"Path\"=\"C:\\\\Program Files\\\\PhotoCraft\"\n"));
        assert_eq!(reg_value(&default, None).as_deref(), Some(r"C:\Program Files\PhotoCraft\photocraft.exe"));
        assert_eq!(reg_value(&default, Some("Path")).as_deref(), Some(r"C:\Program Files\PhotoCraft"));
        // a folder name outside ASCII: what `reg query`'s console-code-page output lost
        let chinese = reg_file(&format!("{key}\n@=\"D:\\\\软件\\\\PhotoCraft\\\\photocraft.exe\"\n"));
        assert_eq!(reg_value(&chinese, None).as_deref(), Some(r"D:\软件\PhotoCraft\photocraft.exe"));
    }

    /// Whatever the file holds, the answer is a value or none: never a panic.
    #[test]
    fn a_reg_export_without_a_usable_value_gives_none() {
        for text in [
            "",
            "[HKEY_LOCAL_MACHINE\\Software\\PhotoCraft]\n",
            "@=\"\"\n",
            "@=\"C:\\\\a.exe\n",
            "@=\"C:\\\\a\\\"\n",
            "@=hex(2):43,00\n",
            "@=dword:00000001\n",
            "\"InstallDir\n",
            "\"InstallDir\"=\n",
        ] {
            assert_eq!(reg_value(&reg_file(text), None), None, "{text:?}");
            assert_eq!(reg_value(&reg_file(text), Some("InstallDir")), None, "{text:?}");
        }
        // not UTF-16 at all, an odd number of bytes, a lone surrogate
        assert_eq!(reg_value(b"@=\"C:\\\\a.exe\"", None), None);
        assert_eq!(reg_value(&[0xff, 0xfe, 0x40], None), None);
        assert_eq!(reg_value(&[0xff, 0xfe, 0x00, 0xd8], None), None);
    }

    /// A helper that can't be started counts as not finished: never a hang or a panic.
    #[test]
    fn a_helper_that_cannot_run_is_given_up_on() {
        let mut missing = Command::new(std::env::temp_dir().join("lightcraft-no-such-helper"));
        assert!(!finishes_in_time(&mut missing, HELPER_DEADLINE));
        // the test binary itself, asked for a test that doesn't exist: runs, and exits quickly
        let mut quick = Command::new(std::env::current_exe().unwrap());
        quick.args(["--list", "--exact", "no::such::test"]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        assert!(finishes_in_time(&mut quick, std::time::Duration::from_secs(60)));
    }

    #[test]
    fn desktop_entries_give_their_program() {
        let appimage = "[Desktop Entry]\nName=PhotoCraft\nExec=\"/home/simon/Apps/photocraft-1.2-linux-x86_64.AppImage\" %F\n";
        assert_eq!(desktop_exec(appimage), Some(PathBuf::from("/home/simon/Apps/photocraft-1.2-linux-x86_64.AppImage")));
        assert_eq!(desktop_exec("Exec=/opt/photocraft/photocraft %F\n"), Some(PathBuf::from("/opt/photocraft/photocraft")));
        assert_eq!(desktop_exec("Exec=photocraft %F\nTryExec=photocraft\n"), None, "a bare name is a PATH lookup");
        assert_eq!(desktop_exec("[Desktop Entry]\n"), None);
        let dirs = desktop_entry_dirs(|k| (k == "HOME").then(|| "/home/simon".to_string()));
        assert_eq!(dirs[0], PathBuf::from("/home/simon/.local/share/applications"));
        assert!(dirs.contains(&PathBuf::from("/usr/share/applications")));
    }

    #[test]
    fn other_editors_are_used_as_named() {
        assert_eq!(launcher("").unwrap(), Launcher::Default);
        assert_eq!(launcher("  ").unwrap(), Launcher::Default);
        let named = launcher("/usr/bin/gimp").unwrap();
        if cfg!(target_os = "macos") {
            assert_eq!(named, Launcher::MacApp("/usr/bin/gimp".into()));
        } else {
            assert_eq!(named, Launcher::Program("/usr/bin/gimp".into()));
        }
    }
}
