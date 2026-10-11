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

#[test]
fn custom_library_and_external_smart_previews_are_cleaned_without_other_files() {
    let f = Fixture::new();
    let custom = f.root.join("custom");
    f.put("custom/catalog.snap", b"catalog");
    f.put("custom/catalog.log", b"log");
    f.put("custom/Originals/preserved.raw", b"raw");
    f.put("proxies/abcdef0123456789abcdef0123456789.lcsp", b"proxy");
    f.put("proxies/photo.jpg", b"real photo");
    f.put("config/ui.json", serde_json::json!({"settings":{"libraryPath":custom}}).to_string().as_bytes());
    f.put("custom/prefs.json", serde_json::json!({"smartPreviewsDir":f.root.join("proxies")}).to_string().as_bytes());
    clean(&f.options).unwrap();
    assert!(!custom.join("catalog.snap").exists());
    assert!(custom.join("Originals/preserved.raw").exists());
    assert!(f.root.join("proxies/photo.jpg").exists());
    assert!(!f.root.join("proxies/abcdef0123456789abcdef0123456789.lcsp").exists());
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
