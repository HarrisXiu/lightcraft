use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture {
    root: PathBuf,
    options: Options,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("lc-uninstall-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let options = Options { config: root.join("config"), library: root.join("library"), delete_originals: false };
        let f = Self { root, options };
        f.put("config/ui.json", b"{\"settings\":{}}");
        f.put("config/logs/lightcraft.log", b"log");
        f.put("config/models/face/model.onnx", b"model");
        f.put("library/catalog.snap", b"catalog");
        f.put("library/catalog.log", b"log");
        f.put("library/prefs.json", b"{}");
        f.put("library/Originals/only-copy.raw", b"irreplaceable");
        f.put("library/Originals/nested/photo.jpg", b"original");
        f.put("external/photo.raw", b"external original");
        f.put("library/thumbs/ab/abcdef0123456789abcdef0123456789.jpg", b"thumbnail");
        f.put("library/thumbs/ab/my-photo.jpg", b"unrecognized photo");
        f.put("library/Smart Previews/abcdef0123456789abcdef0123456789.lcsp", b"proxy");
        f.put("library/Smart Previews/look-version", b"1");
        f
    }
    fn put(&self, name: &str, bytes: &[u8]) {
        let path = self.root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn clean_preserves_originals_external_files_and_unrecognized_cache_files() {
    let f = Fixture::new();
    assert!(clean(&f.options).unwrap() >= 7);
    assert!(!f.options.config.exists());
    assert!(!f.options.library.join("catalog.snap").exists());
    assert!(!f.options.library.join("thumbs/ab/abcdef0123456789abcdef0123456789.jpg").exists());
    assert!(!f.options.library.join("Smart Previews").exists());
    assert_eq!(fs::read(f.options.library.join("Originals/only-copy.raw")).unwrap(), b"irreplaceable");
    assert!(f.options.library.join("thumbs/ab/my-photo.jpg").exists());
    assert!(f.root.join("external/photo.raw").exists());
}

#[test]
fn deleting_originals_requires_separate_consent_and_keeps_external_photos() {
    let mut f = Fixture::new();
    f.options.delete_originals = true;
    clean(&f.options).unwrap();
    assert!(!f.options.library.join("Originals").exists());
    assert_eq!(fs::read(f.root.join("external/photo.raw")).unwrap(), b"external original");
}

/// The library last opened from Settings is cleaned too, and so is its smart previews folder
/// elsewhere when that holds nothing but proxies and their marker (a folder holding anything
/// else: `a_preview_folder_preference_pointing_at_a_user_folder_changes_nothing_there`).
#[test]
fn custom_library_and_a_preview_folder_of_only_proxies_are_cleaned() {
    let f = Fixture::new();
    let custom = f.root.join("custom");
    f.put("custom/catalog.snap", b"catalog");
    f.put("custom/catalog.log", b"log");
    f.put("custom/Originals/preserved.raw", b"raw");
    f.put("proxies/abcdef0123456789abcdef0123456789.lcsp", b"proxy");
    f.put("proxies/look-version", b"1");
    f.put("config/ui.json", serde_json::json!({"settings":{"libraryPath":custom}}).to_string().as_bytes());
    f.put("custom/prefs.json", serde_json::json!({"smartPreviewsDir":f.root.join("proxies")}).to_string().as_bytes());
    clean(&f.options).unwrap();
    assert!(!custom.join("catalog.snap").exists());
    assert!(custom.is_dir(), "a library folder itself is never removed");
    assert!(custom.join("Originals/preserved.raw").exists());
    assert!(!f.root.join("proxies").exists());
}

#[test]
fn active_library_refuses_cleanup_before_settings_or_photos_are_removed() {
    let mut f = Fixture::new();
    f.options.delete_originals = true;
    let lock = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(f.options.library.join("catalog.lock")).unwrap();
    lock.try_lock().unwrap();
    assert!(clean(&f.options).unwrap_err().contains("close LightCraft"));
    assert!(f.options.config.join("ui.json").exists());
    assert!(f.options.library.join("Originals/only-copy.raw").exists());
}

#[test]
fn damaged_or_oversized_settings_and_unrecognized_library_delete_nothing() {
    let f = Fixture::new();
    for bytes in [b"bad JSON".to_vec(), vec![b' '; MAX_JSON as usize + 1]] {
        f.put("config/ui.json", &bytes);
        assert!(clean(&f.options).is_err());
        assert!(f.options.library.join("catalog.snap").exists());
    }
    f.put("config/ui.json", serde_json::json!({"settings":{"libraryPath":f.root.join("external")}}).to_string().as_bytes());
    assert!(clean(&f.options).unwrap_err().contains("not a recognized"));
    assert!(f.options.config.join("ui.json").exists());
    assert!(f.options.library.join("catalog.snap").exists());
}

#[test]
fn relative_and_volume_root_paths_are_refused() {
    let mut f = Fixture::new();
    for path in [PathBuf::from("relative"), f.root.ancestors().last().unwrap().to_path_buf(), f.root.join("../other")] {
        f.options.library = path;
        assert!(clean(&f.options).is_err());
        assert!(f.options.config.join("ui.json").exists());
    }
}

#[test]
fn original_copies_are_protected_even_when_configured_as_a_preview_folder() {
    let f = Fixture::new();
    let originals = f.options.library.join("Originals");
    f.put("library/Originals/abcdef0123456789abcdef0123456789.lcsp", b"original file");
    // A manually edited preference, including a differently cased Windows path.
    let path = if cfg!(windows) { originals.to_string_lossy().to_uppercase() } else { originals.to_string_lossy().into_owned() };
    f.put("library/prefs.json", serde_json::json!({"smartPreviewsDir":path}).to_string().as_bytes());
    clean(&f.options).unwrap();
    assert!(originals.join("abcdef0123456789abcdef0123456789.lcsp").exists());
    assert!(originals.join("only-copy.raw").exists());
}

#[test]
fn an_offline_saved_library_leaves_settings_and_default_library_untouched() {
    let f = Fixture::new();
    f.put("config/ui.json", serde_json::json!({"settings":{"libraryPath":f.root.join("unplugged")}}).to_string().as_bytes());
    assert!(clean(&f.options).unwrap_err().contains("unavailable"));
    assert!(f.options.config.join("ui.json").exists());
    assert!(f.options.library.join("catalog.snap").exists());
}

/// Every folder and file below `dir`, with each file's contents: what a user would miss.
fn listing(dir: &Path) -> Vec<(PathBuf, Option<Vec<u8>>)> {
    let mut out = Vec::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for entry in fs::read_dir(&d).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                todo.push(path.clone());
                out.push((path, None));
            } else {
                let bytes = fs::read(&path).unwrap();
                out.push((path, Some(bytes)));
            }
        }
    }
    out.sort();
    out
}

const PROXY: &str = "abcdef0123456789abcdef0123456789.lcsp";

/// The smart previews folder is a preference: a user setting, a hand edit or a damaged file can
/// point it at a home or Pictures folder. Nothing there may change: not the empty folders, not
/// a file that happens to be called `look-version` or to look like a proxy (review of PR #931).
#[test]
fn a_preview_folder_preference_pointing_at_a_user_folder_changes_nothing_there() {
    type Fill = fn(&Fixture);
    let cases: [(&str, Fill); 3] = [
        ("a home folder", |f| {
            f.put("elsewhere/Pictures/2024/trip.jpg", b"photo");
            f.put("elsewhere/Pictures/look-version", b"a file of the user's");
            f.put(&format!("elsewhere/Documents/{PROXY}"), b"a file of the user's");
            f.put("elsewhere/look-version", b"a file of the user's");
            f.put(&format!("elsewhere/{PROXY}"), b"a file of the user's");
            for empty in ["elsewhere/New folder", "elsewhere/Pictures/Empty album", "elsewhere/Projects/a/b"] {
                fs::create_dir_all(f.root.join(empty)).unwrap();
            }
        }),
        ("proxies beside a photo", |f| {
            f.put(&format!("elsewhere/{PROXY}"), b"proxy");
            f.put("elsewhere/photo.jpg", b"photo");
        }),
        ("no proxy at all", |f| {
            f.put("elsewhere/look-version", b"a file of the user's");
        }),
    ];
    for (what, fill) in cases {
        let f = Fixture::new();
        let elsewhere = f.root.join("elsewhere");
        fill(&f);
        f.put("library/prefs.json", serde_json::json!({"smartPreviewsDir":elsewhere}).to_string().as_bytes());
        let before = listing(&elsewhere);
        clean(&f.options).unwrap();
        assert_eq!(listing(&elsewhere), before, "{what}");
        assert!(!f.options.library.join("catalog.snap").exists(), "{what}: the library is still cleaned");
    }
}

/// A folder is removed only when deleting our files emptied it: one that was already empty, and
/// so every folder above it, stays.
#[test]
fn folders_that_were_already_empty_are_kept() {
    let mut f = Fixture::new();
    f.options.delete_originals = true;
    let kept = ["library/thumbs/cd", "library/Smart Previews/Mine", "library/Originals/Empty album", "config/models/custom", "config/Notes"];
    for dir in kept {
        fs::create_dir_all(f.root.join(dir)).unwrap();
    }
    clean(&f.options).unwrap();
    for dir in kept {
        assert!(f.root.join(dir).is_dir(), "{dir}");
    }
    // what our deletions emptied is gone, files and folders
    for gone in ["config/ui.json", "config/logs", "config/models/face", "library/Originals/nested", "library/Originals/only-copy.raw"] {
        assert!(!f.root.join(gone).exists(), "{gone}");
    }
    assert!(!f.root.join("library/Smart Previews").join(PROXY).exists());
}

/// The settings folder comes from an environment variable or an MSI property. Without our
/// settings file in it, a `models` or `logs` folder there is not known to be ours.
#[test]
fn a_settings_folder_without_our_settings_file_is_left_alone() {
    for settings in [None, Some("{}"), Some("[]")] {
        let f = Fixture::new();
        match settings {
            Some(text) => f.put("config/ui.json", text.as_bytes()),
            None => fs::remove_file(f.options.config.join("ui.json")).unwrap(),
        }
        let before = listing(&f.options.config);
        clean(&f.options).unwrap();
        assert_eq!(listing(&f.options.config), before, "{settings:?}");
        assert!(!f.options.library.join("catalog.snap").exists(), "{settings:?}: the library is still cleaned");
    }
}

#[cfg(windows)]
#[test]
fn junction_inside_originals_is_refused_without_deleting_target_or_settings() {
    let mut f = Fixture::new();
    f.options.delete_originals = true;
    let link = f.options.library.join("Originals/junction");
    let target = f.root.join("external");
    let status = std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(link.to_string_lossy().replace('/', "\\"))
        .arg(target.to_string_lossy().replace('/', "\\"))
        .status()
        .unwrap();
    assert!(status.success());
    let result = clean(&f.options);
    // Remove only the junction before teardown, never recurse through it.
    fs::remove_dir(&link).unwrap();
    assert!(result.unwrap_err().contains("junctions"));
    assert!(target.join("photo.raw").exists());
    assert!(f.options.config.join("ui.json").exists());
    assert!(f.options.library.join("Originals/only-copy.raw").exists());
}
