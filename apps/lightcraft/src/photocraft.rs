//! Edit in PhotoCraft: find an installed PhotoCraft (the sibling image editor) and open a file in it.
//!
//! Where it is looked for:
//! - Windows: the `App Paths\photocraft.exe` registration its installer writes (machine, then
//!   user), then `photocraft.exe` in a `PATH` folder.
//! - macOS: `PhotoCraft.app` in `/Applications` or `~/Applications`.
//! - Linux: `photocraft` in a `PATH` folder.
//!
//! A portable copy somewhere else and a Flatpak are not found (a Flatpak could not read the file
//! handed to it anyway): Settings ▸ General ▸ External Editor is the way to use those.

use std::path::PathBuf;
use std::process::Command;

/// Set for the PhotoCraft we start: the files on its command line are edit copies it may save
/// back in place (File ▸ Save, no Save As). A PhotoCraft that doesn't know it ignores it.
#[cfg(any(not(target_os = "macos"), test))]
const EDIT_IN_PLACE: &str = "PHOTOCRAFT_EDIT_IN_PLACE";

/// A PhotoCraft that was found, and how it is started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    /// Its executable.
    #[cfg(any(not(target_os = "macos"), test))]
    Program(PathBuf),
    /// Its macOS application bundle.
    #[cfg(any(target_os = "macos", test))]
    MacApp(PathBuf),
}

/// The installed PhotoCraft, or why there is none to start.
pub fn find() -> Result<Found, String> {
    if std::env::var_os("FLATPAK_ID").is_some() {
        return Err("LightCraft runs in a Flatpak sandbox, which can't start other apps".into());
    }
    installed().ok_or_else(|| "PhotoCraft was not found".to_string())
}

/// Open `file` in `found`. PhotoCraft is started, never waited for.
pub fn open_in(found: &Found, file: &str) -> Result<(), String> {
    command(found, file).spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// The command that opens `file` in `found`.
fn command(found: &Found, file: &str) -> Command {
    match found {
        #[cfg(any(not(target_os = "macos"), test))]
        Found::Program(program) => {
            let mut c = Command::new(program);
            c.arg(file).env(EDIT_IN_PLACE, "1");
            c
        }
        // (`open` hands the file over as an Apple event: the environment doesn't reach the app)
        #[cfg(any(target_os = "macos", test))]
        Found::MacApp(bundle) => {
            let mut c = Command::new("open");
            c.arg("-a").arg(bundle).arg(file);
            c
        }
    }
}

#[cfg(windows)]
fn installed() -> Option<Found> {
    ["HKLM", "HKCU"]
        .into_iter()
        .find_map(|root| registered(root).filter(|p| p.is_file()))
        .or_else(|| in_path("photocraft.exe", std::env::var_os("PATH").as_deref()))
        .map(Found::Program)
}

#[cfg(target_os = "macos")]
fn installed() -> Option<Found> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    mac_bundle(home.as_deref()).map(Found::MacApp)
}

#[cfg(not(any(windows, target_os = "macos")))]
fn installed() -> Option<Found> {
    in_path("photocraft", std::env::var_os("PATH").as_deref()).map(Found::Program)
}

/// The program PhotoCraft's installer registered under `root` (`HKLM` or `HKCU`).
///
/// Read through `reg export`: its file is always UTF-16, while the text `reg query` prints is in
/// the console's code page, which garbles a path with, say, Chinese characters in it.
#[cfg(windows)]
fn registered(root: &str) -> Option<PathBuf> {
    use std::os::windows::process::CommandExt;
    /// No console window for the helper (`CREATE_NO_WINDOW`).
    const NO_WINDOW: u32 = 0x0800_0000;
    let reg = std::env::var_os("SystemRoot").map(|r| PathBuf::from(r).join("System32").join("reg.exe")).filter(|p| p.is_file());
    // (a name of its own each time: a file an earlier run could not delete must not be in the way)
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let file = std::env::temp_dir().join(format!("lightcraft-photocraft-{}-{stamp}-{root}.reg", std::process::id()));
    let key = format!(r"{root}\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\photocraft.exe");
    let mut export = Command::new(reg.as_deref().unwrap_or(std::path::Path::new("reg")));
    export
        .args(["export", &key])
        .arg(&file)
        .arg("/y")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(NO_WINDOW);
    let bytes = if finishes_in_time(&mut export, HELPER_DEADLINE) { std::fs::read(&file).ok() } else { None };
    let _ = std::fs::remove_file(&file);
    reg_default_value(&bytes?).map(PathBuf::from)
}

/// How long the registry helper may take. It runs on the UI thread and answers in a few tens of
/// milliseconds; a helper that hangs (a temporary folder on a dead network share) is given up on.
#[cfg(any(windows, test))]
const HELPER_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);

/// Run `command` and say whether it succeeded within `deadline`; one that is still running then
/// is killed.
#[cfg(any(windows, test))]
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

/// The key's default value (the `@="…"` line) in a `reg export` file: UTF-16LE, `\` and `"`
/// escaped with a backslash.
#[cfg(any(windows, test))]
fn reg_default_value(bytes: &[u8]) -> Option<String> {
    let units: Vec<u16> = bytes.chunks_exact(2).filter_map(|c| <[u8; 2]>::try_from(c).ok()).map(u16::from_le_bytes).collect();
    let text = String::from_utf16_lossy(&units);
    let line = text.lines().map(|l| l.trim_start_matches('\u{feff}').trim()).find(|l| l.starts_with("@=\""))?;
    let body = line.strip_prefix("@=\"")?.strip_suffix('"')?;
    let mut value = String::with_capacity(body.len());
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        value.push(if c == '\\' { chars.next()? } else { c });
    }
    // (a registration may quote the path)
    let value = value.trim().trim_matches('"').trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// `name` in the first folder of the `PATH`-style list `path` that has such a program. Folders
/// given relative to wherever LightCraft was started from are passed over: what is found is
/// handed the user's photo.
#[cfg(any(not(target_os = "macos"), test))]
fn in_path(name: &str, path: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    std::env::split_paths(path?).filter(|dir| dir.is_absolute()).map(|dir| dir.join(name)).find(|p| is_program(p))
}

/// A file that can be started: on Unix one of its execute bits is set.
#[cfg(any(not(target_os = "macos"), test))]
fn is_program(path: &std::path::Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else { return false };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

/// `PhotoCraft.app` in the system's or the user's Applications folder (a bundle: a folder with
/// its `Contents/Info.plist`, so an empty folder of that name doesn't hide the real one).
#[cfg(any(target_os = "macos", test))]
fn mac_bundle(home: Option<&std::path::Path>) -> Option<PathBuf> {
    let user = home.map(|h| h.join("Applications"));
    [Some(PathBuf::from("/Applications")), user]
        .into_iter()
        .flatten()
        .map(|dir| dir.join("PhotoCraft.app"))
        .find(|p| p.join("Contents").join("Info.plist").is_file())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::path::Path;

    use super::*;

    /// A `reg export` file as Windows writes it: UTF-16LE with a byte order mark and CRLF.
    fn reg_file(text: &str) -> Vec<u8> {
        let mut bytes = vec![0xff, 0xfe];
        bytes.extend(text.replace('\n', "\r\n").encode_utf16().flat_map(u16::to_le_bytes));
        bytes
    }

    #[test]
    fn the_registered_program_is_read_from_a_reg_export() {
        let key = "[HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths\\photocraft.exe]";
        let plain = reg_file(&format!(
            "Windows Registry Editor Version 5.00\n\n{key}\n@=\"C:\\\\Program Files\\\\PhotoCraft\\\\photocraft.exe\"\n\"Path\"=\"C:\\\\Program Files\\\\PhotoCraft\"\n\n"
        ));
        assert_eq!(reg_default_value(&plain).as_deref(), Some(r"C:\Program Files\PhotoCraft\photocraft.exe"));
        // a folder name outside ASCII (the reason this isn't parsed from `reg query`'s output)
        let chinese = reg_file(&format!("Windows Registry Editor Version 5.00\n\n{key}\n@=\"D:\\\\软件\\\\PhotoCraft\\\\photocraft.exe\"\n"));
        assert_eq!(reg_default_value(&chinese).as_deref(), Some(r"D:\软件\PhotoCraft\photocraft.exe"));
        // a registration that quotes its path
        let quoted = reg_file(&format!("{key}\n@=\"\\\"C:\\\\My Apps\\\\photocraft.exe\\\"\"\n"));
        assert_eq!(reg_default_value(&quoted).as_deref(), Some(r"C:\My Apps\photocraft.exe"));
    }

    /// Whatever the file holds, the answer is a value or none: never a panic.
    #[test]
    fn a_reg_export_without_a_usable_default_value_gives_none() {
        let key = "[HKEY_CURRENT_USER\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths\\photocraft.exe]";
        for text in [
            String::new(),
            format!("{key}\n"),
            format!("{key}\n@=\"\"\n"),
            format!("{key}\n@=\"C:\\\\a.exe\n"),
            format!("{key}\n@=\"C:\\\\a\\\"\n"),
            format!("{key}\n@=hex(2):43,00\n"),
        ] {
            assert_eq!(reg_default_value(&reg_file(&text)), None, "{text:?}");
        }
        // not UTF-16 at all, an odd number of bytes, a lone surrogate
        assert_eq!(reg_default_value(b"@=\"C:\\\\a.exe\""), None);
        assert_eq!(reg_default_value(&[0xff, 0xfe, 0x40]), None);
        assert_eq!(reg_default_value(&[0xff, 0xfe, 0x00, 0xd8]), None);
    }

    #[test]
    fn the_program_is_found_in_a_path_folder() {
        let dir = std::env::temp_dir().join(format!("lc-photocraft-path-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (empty, bin) = (dir.join("empty"), dir.join("with space").join("bin"));
        std::fs::create_dir_all(&empty).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        let (plain, later) = (dir.join("plain"), dir.join("later"));
        std::fs::create_dir_all(&plain).unwrap();
        std::fs::create_dir_all(&later).unwrap();
        for folder in [&plain, &bin, &later] {
            std::fs::write(folder.join("photocraft"), b"").unwrap();
        }
        // on Unix a program is a file that may be executed: `plain`'s isn't, and is passed over
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(plain.join("photocraft"), std::fs::Permissions::from_mode(0o644)).unwrap();
            for folder in [&bin, &later] {
                std::fs::set_permissions(folder.join("photocraft"), std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        // a folder of that name is not the program, and a relative folder is not looked in
        std::fs::create_dir_all(empty.join("photocraft")).unwrap();
        let first = if cfg!(unix) { bin.clone() } else { plain.clone() };
        let path = std::env::join_paths([
            PathBuf::from("relative"),
            PathBuf::from("."),
            empty.clone(),
            dir.join("missing"),
            plain.clone(),
            bin.clone(),
            later,
        ])
        .unwrap();
        assert_eq!(in_path("photocraft", Some(&path)), Some(first.join("photocraft")));
        assert_eq!(in_path("photocraft.exe", Some(&path)), None);
        assert_eq!(in_path("photocraft", Some(std::env::join_paths([PathBuf::from("."), PathBuf::from("bin")]).unwrap().as_os_str())), None);
        assert_eq!(in_path("photocraft", Some(OsStr::new(""))), None);
        assert_eq!(in_path("photocraft", None), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_mac_bundle_is_found_in_the_users_applications() {
        let home = std::env::temp_dir().join(format!("lc-photocraft-home-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("Applications")).unwrap();
        if !Path::new("/Applications/PhotoCraft.app").is_dir() {
            assert_eq!(mac_bundle(Some(&home)), None);
            assert_eq!(mac_bundle(None), None);
        }
        // an empty folder of that name is not the app
        let bundle = home.join("Applications").join("PhotoCraft.app");
        std::fs::create_dir_all(bundle.join("Contents")).unwrap();
        if !Path::new("/Applications/PhotoCraft.app").is_dir() {
            assert_eq!(mac_bundle(Some(&home)), None);
        }
        std::fs::write(bundle.join("Contents").join("Info.plist"), b"<plist/>").unwrap();
        if !Path::new("/Applications/PhotoCraft.app").is_dir() {
            assert_eq!(mac_bundle(Some(&home)), Some(bundle));
        }
        let _ = std::fs::remove_dir_all(&home);
    }

    /// A helper that can't be started, or fails, counts as not finished: never a hang or a panic.
    #[test]
    fn a_helper_that_cannot_run_is_given_up_on() {
        let mut missing = Command::new(std::env::temp_dir().join("lc-photocraft-no-such-helper"));
        assert!(!finishes_in_time(&mut missing, HELPER_DEADLINE));
        // the test binary itself, asked for a test that doesn't exist: runs, and exits quickly
        let mut quick = Command::new(std::env::current_exe().unwrap());
        quick.args(["--list", "--exact", "no::such::test"]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        assert!(finishes_in_time(&mut quick, std::time::Duration::from_secs(60)));
    }

    /// The program gets the file as one argument (spaces and all) and is told it may save it in
    /// place; a macOS bundle is opened through `open -a`.
    #[test]
    fn the_file_is_handed_over_as_one_argument() {
        let file = r"C:\My Photos\a & b-Edit.tif";
        let c = command(&Found::Program(PathBuf::from("photocraft")), file);
        assert_eq!(c.get_program(), OsStr::new("photocraft"));
        assert_eq!(c.get_args().collect::<Vec<_>>(), [OsStr::new(file)]);
        assert!(c.get_envs().any(|(k, v)| k == OsStr::new(EDIT_IN_PLACE) && v == Some(OsStr::new("1"))));

        let c = command(&Found::MacApp(PathBuf::from("/Applications/PhotoCraft.app")), "/Users/a/b Edit.tif");
        assert_eq!(c.get_program(), OsStr::new("open"));
        assert_eq!(
            c.get_args().collect::<Vec<_>>(),
            [OsStr::new("-a"), OsStr::new("/Applications/PhotoCraft.app"), OsStr::new("/Users/a/b Edit.tif")]
        );
        assert_eq!(c.get_envs().count(), 0);
    }
}
