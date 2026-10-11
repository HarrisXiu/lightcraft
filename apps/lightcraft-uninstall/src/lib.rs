//! Opt-in MSI cleanup. Delete only LightCraft's files, never an entire library root.
//! Preflight every target and hold the same OS locks as the app before removing anything.
//! A folder is removed only when deleting our files emptied it, and a folder a preference or
//! the environment points at is touched only when its contents show it is ours.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::collections::HashSet;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

const MAX_JSON: u64 = 1 << 20;
const MAX_ENTRIES: usize = 200_000;
const MAX_DEPTH: usize = 64;

pub struct Options {
    pub config: PathBuf,
    pub library: PathBuf,
    /// Explicit, separate consent to permanently delete copies in `Originals`.
    pub delete_originals: bool,
}

#[derive(Default)]
struct Plan {
    files: Vec<PathBuf>,
    dirs: Vec<PathBuf>,
    locks: Vec<File>,
    seen_files: HashSet<PathBuf>,
    seen_dirs: HashSet<PathBuf>,
    protected: Vec<PathBuf>,
    visited: usize,
}

fn fail(path: &Path, why: impl std::fmt::Display) -> String {
    format!("{}: {why}", path.display())
}

fn is_link(meta: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // Also refuse junctions and other reparse points, which file_type alone misses.
        if meta.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    meta.file_type().is_symlink()
}

/// Refuse relative paths, volume roots, `..` and links at any existing ancestor.
fn checked(path: &Path) -> Result<Option<Metadata>, String> {
    if !path.is_absolute() || path.parent().is_none() || path.file_name().is_none() {
        return Err(fail(path, "cleanup requires an absolute directory below the volume root"));
    }
    let mut part = PathBuf::new();
    let mut last = None;
    for component in path.components() {
        if matches!(component, Component::ParentDir | Component::CurDir) {
            return Err(fail(path, "cleanup refuses parent/relative path components"));
        }
        part.push(component.as_os_str());
        if matches!(component, Component::Prefix(_)) {
            // A Windows drive prefix alone (especially \\?\C:) is not a root directory.
            continue;
        }
        match fs::symlink_metadata(&part) {
            Ok(meta) if is_link(&meta) => return Err(fail(&part, "cleanup refuses symbolic links and junctions")),
            Ok(meta) => last = Some(meta),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(fail(&part, e)),
        }
    }
    Ok(last)
}

fn directory(path: &Path) -> Result<PathBuf, String> {
    match checked(path)? {
        Some(meta) if meta.is_dir() => fs::canonicalize(path).map_err(|e| fail(path, e)),
        Some(_) => Err(fail(path, "expected a directory")),
        None => Ok(path.components().collect()),
    }
}

fn json_file(path: &Path) -> Result<Option<serde_json::Value>, String> {
    let Some(meta) = checked(path)? else { return Ok(None) };
    if !meta.is_file() || meta.len() > MAX_JSON {
        return Err(fail(path, "settings are not a regular file of at most 1 MiB; clean up manually"));
    }
    let mut bytes = Vec::new();
    File::open(path).map_err(|e| fail(path, e))?.take(MAX_JSON + 1).read_to_end(&mut bytes).map_err(|e| fail(path, e))?;
    if bytes.len() as u64 > MAX_JSON {
        return Err(fail(path, "settings exceed 1 MiB"));
    }
    serde_json::from_slice(&bytes).map(Some).map_err(|e| fail(path, format!("settings could not be read ({e}); clean up manually")))
}

impl Plan {
    /// Queue `path` for deletion; a protected or missing file is skipped (`false`). Its folder is
    /// not touched: a library's own folder is never removed.
    fn queue(&mut self, path: &Path) -> Result<bool, String> {
        if self.protected.iter().any(|p| path.starts_with(p)) {
            return Ok(false);
        }
        let Some(meta) = checked(path)? else { return Ok(false) };
        if !meta.is_file() {
            return Err(fail(path, "expected a regular file"));
        }
        if self.seen_files.insert(path.to_path_buf()) {
            if self.seen_files.len() > MAX_ENTRIES {
                return Err(fail(path, "too many cleanup files; clean up manually"));
            }
            self.files.push(path.to_path_buf());
        }
        Ok(true)
    }

    /// [`Plan::queue`] in `root`, a folder that is ours to remove. The folders from the file's
    /// up to `root` become candidates for removal ([`Plan::apply`] removes one only if it is
    /// empty by then), so a folder that was already empty, or holds nothing of ours, is never a
    /// candidate.
    fn file(&mut self, path: &Path, root: &Path) -> Result<(), String> {
        if !self.queue(path)? {
            return Ok(());
        }
        let mut dir = path.parent();
        while let Some(d) = dir.filter(|d| d.starts_with(root)) {
            if self.seen_dirs.insert(d.to_path_buf()) {
                self.dirs.push(d.to_path_buf());
            }
            if d == root {
                break;
            }
            dir = d.parent();
        }
        Ok(())
    }

    /// Enumerate bounded trees ahead of deletion. Never use remove_dir_all, including on failure.
    fn tree(&mut self, path: &Path, keep: fn(&Path) -> bool) -> Result<(), String> {
        self.tree_in(path, path, keep)
    }

    /// [`Plan::tree`] under `root`, the folder the caller owns: only folders from `root` down
    /// can be removed, and only when a deletion emptied them (see [`Plan::file`]).
    fn tree_in(&mut self, root: &Path, path: &Path, keep: fn(&Path) -> bool) -> Result<(), String> {
        self.walk(root, path, 0, keep)
    }

    fn walk(&mut self, root: &Path, path: &Path, depth: usize, keep: fn(&Path) -> bool) -> Result<(), String> {
        if self.protected.iter().any(|p| path.starts_with(p)) {
            return Ok(());
        }
        let Some(meta) = checked(path)? else { return Ok(()) };
        self.visited = self.visited.saturating_add(1);
        if depth > MAX_DEPTH || self.visited > MAX_ENTRIES {
            return Err(fail(path, "cleanup exceeds its safe size/depth limit; clean up manually"));
        }
        if meta.is_file() {
            if !keep(path) {
                self.file(path, root)?;
            }
        } else if meta.is_dir() {
            for entry in fs::read_dir(path).map_err(|e| fail(path, e))? {
                self.walk(root, &entry.map_err(|e| fail(path, e))?.path(), depth + 1, keep)?;
            }
        } else {
            return Err(fail(path, "cleanup refuses special files"));
        }
        Ok(())
    }

    /// A smart previews folder from a preference, which may be a hand edit or point anywhere. It
    /// is cleaned only when its contents are exactly our layout: `<32 hex>.lcsp` proxies and
    /// `look-version`, at least one proxy, no folders and no other files. Anything else is left
    /// alone, so a folder of photos or subfolders is never walked.
    fn smart_folder(&mut self, path: &Path) -> Result<(), String> {
        if self.protected.iter().any(|p| path.starts_with(p)) {
            return Ok(());
        }
        let Some(meta) = checked(path)? else { return Ok(()) };
        if !meta.is_dir() {
            return Ok(());
        }
        let mut ours = Vec::new();
        let mut proxies = 0usize;
        for entry in fs::read_dir(path).map_err(|e| fail(path, e))? {
            let entry = entry.map_err(|e| fail(path, e))?;
            let entry_path = entry.path();
            if !entry.file_type().map_err(|e| fail(&entry_path, e))?.is_file() {
                return Ok(());
            }
            if hex_name(&entry_path, "lcsp") {
                proxies += 1;
            } else if entry_path.file_name().is_none_or(|n| n != "look-version") {
                return Ok(());
            }
            ours.push(entry_path);
        }
        if proxies == 0 {
            return Ok(());
        }
        for file in &ours {
            self.file(file, path)?;
        }
        Ok(())
    }

    fn library(&mut self, path: &Path, delete_originals: bool) -> Result<(), String> {
        let Some(meta) = checked(path)? else { return Ok(()) };
        if !meta.is_dir() {
            return Err(fail(path, "library is not a directory"));
        }
        // Settings paths are untrusted. Require independent library markers before touching
        // catalog files or Originals; never recursively remove the containing folder.
        let mut found = 0;
        for name in ["catalog.snap", "catalog.log", "catalog.lock", "prefs.json"] {
            found += usize::from(checked(&path.join(name))?.is_some_and(|m| m.is_file()));
        }
        if found < 2 {
            return Err(fail(path, "not a recognized LightCraft library; no files were removed"));
        }
        let lock_path = path.join("catalog.lock");
        checked(&lock_path)?;
        let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&lock_path).map_err(|e| fail(&lock_path, e))?;
        file.try_lock().map_err(|e| fail(path, format!("close LightCraft and any CLI using this library before cleaning it ({e})")))?;
        self.locks.push(file);
        let prefs = json_file(&path.join("prefs.json"))?;
        let smart = prefs.as_ref().and_then(|p| p.get("smartPreviewsDir")).and_then(serde_json::Value::as_str).filter(|s| !s.is_empty());
        self.tree(&path.join("Smart Previews"), keep_non_preview)?;
        if let Some(smart) = smart {
            self.smart_folder(&directory(Path::new(smart))?)?;
        }
        self.tree(&path.join("thumbs"), keep_non_thumbnail)?;
        for entry in fs::read_dir(path).map_err(|e| fail(path, e))? {
            let entry = entry.map_err(|e| fail(path, e))?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if matches!(
                name.as_ref(),
                "catalog.snap" | "catalog.log" | "catalog.lock.owner" | "presets.json" | "view.json" | "prefs.json" | "face-scanned.bin"
            ) || (name.starts_with("face-embeddings-") && name.ends_with(".bin"))
                || name.starts_with("catalog.log.damaged-")
                || ["catalog.snap", "catalog.log", "presets.json", "view.json", "prefs.json"]
                    .iter()
                    .any(|n| name == format!("{n}.tmp") || name.starts_with(&format!("{n}.corrupt-")))
            {
                self.queue(&entry.path())?;
            }
        }
        if delete_originals {
            self.tree(&path.join("Originals"), |_| false)?;
        }
        Ok(())
    }

    fn apply(self) -> Result<usize, String> {
        let mut removed = 0;
        for path in &self.files {
            // Recheck ancestors immediately before deleting in case the tree changed.
            if checked(path)?.is_some() {
                fs::remove_file(path).map_err(|e| fail(path, e))?;
                removed += 1;
            }
        }
        // Deepest first: a folder is tried once the candidates inside it are gone. Only folders
        // that held a file deleted above are candidates (`file`), and `remove_dir` leaves any
        // that still holds something, so a folder that was already empty is never removed.
        let mut dirs = self.dirs;
        dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
        for path in &dirs {
            if checked(path)?.is_some() {
                match fs::remove_dir(path) {
                    Ok(()) => {}
                    // Preserved/unrecognized files keep their containing directory too.
                    Err(e) if matches!(e.kind(), io::ErrorKind::DirectoryNotEmpty | io::ErrorKind::NotFound) => {}
                    Err(e) => return Err(fail(path, e)),
                }
            }
        }
        // Keep catalog.lock: deleting it after unlocking races a new library opener. It
        // contains no catalog data and does not prevent first-run demo initialization.
        Ok(removed)
    }
}

fn hex_name(path: &Path, extension: &str) -> bool {
    path.extension().is_some_and(|e| e == extension)
        && path.file_stem().and_then(|s| s.to_str()).is_some_and(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()))
}
fn keep_non_preview(path: &Path) -> bool {
    !hex_name(path, "lcsp") && path.file_name().is_none_or(|n| n != "look-version")
}
fn keep_non_thumbnail(path: &Path) -> bool {
    let shard = path.parent().and_then(Path::file_name).and_then(|s| s.to_str());
    !(hex_name(path, "jpg") && shard.is_some_and(|s| s.len() == 2 && s.bytes().all(|b| b.is_ascii_hexdigit())))
}

/// Clean this account's config, default/explicit library and remembered current library.
/// Preflight errors leave data untouched. Later I/O errors are reported to MSI.
pub fn clean(options: &Options) -> Result<usize, String> {
    // Canonical spellings make containment and duplicate-library checks insensitive to
    // Windows drive/path casing, alternate separators and the MSI's trailing \\. suffix.
    let options = Options { config: directory(&options.config)?, library: directory(&options.library)?, delete_originals: options.delete_originals };
    let settings = json_file(&options.config.join("ui.json"))?;
    let saved = settings
        .as_ref()
        .and_then(|s| s.get("settings"))
        .and_then(|s| s.get("libraryPath"))
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty());
    let mut libraries = vec![options.library.clone()];
    if let Some(saved) = saved {
        let saved = directory(Path::new(saved))?;
        if checked(&saved)?.is_none() {
            return Err(fail(&saved, "the saved library is unavailable; reconnect it before cleaning"));
        }
        if saved != options.library {
            libraries.push(saved);
        }
    }
    let mut plan = Plan::default();
    if !options.delete_originals {
        plan.protected = libraries.iter().map(|p| p.join("Originals")).collect();
    }
    for library in &libraries {
        plan.library(library, options.delete_originals)?;
    }
    // The settings folder can be set by an environment variable or an MSI property, so its name
    // proves nothing. Clean it only when it holds our settings file: without one, a `models` or
    // `logs` folder there may be anyone's.
    let ours = settings.as_ref().and_then(|s| s.get("settings")).is_some_and(serde_json::Value::is_object);
    if ours {
        for name in ["logs", "models", "denoise-models", "camera-profiles"] {
            let dir = options.config.join(name);
            for library in &libraries {
                if library.starts_with(&dir) || dir.starts_with(library) {
                    return Err(fail(&dir, "configuration and library folders overlap; clean up manually"));
                }
            }
            plan.tree_in(&options.config, &dir, |_| false)?;
        }
        for entry in fs::read_dir(&options.config).map_err(|e| fail(&options.config, e))? {
            let entry = entry.map_err(|e| fail(&options.config, e))?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if matches!(name.as_ref(), "ui.json" | "ui.json.tmp" | "gpu-init.marker") || name.starts_with("ui.json.corrupt-") {
                plan.file(&entry.path(), &options.config)?;
            }
        }
    }
    plan.apply()
}

#[cfg(test)]
mod tests;
