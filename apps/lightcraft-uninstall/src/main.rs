#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use lightcraft_uninstall::{Options, clean};
use std::path::PathBuf;

fn run() -> Result<usize, String> {
    let mut args = std::env::args_os().skip(1);
    let mut config = None;
    let mut library = None;
    let mut originals = false;
    while let Some(arg) = args.next() {
        let value = args.next().ok_or("cleanup argument needs a value")?;
        match arg.to_str() {
            Some("--config") if config.is_none() => config = Some(PathBuf::from(value)),
            Some("--library") if library.is_none() => library = Some(PathBuf::from(value)),
            Some("--delete-originals") if value.is_empty() => originals = false,
            Some("--delete-originals") if value == "1" => originals = true,
            _ => return Err("expected --config <directory> --library <directory> --delete-originals \"\"|1".into()),
        }
    }
    clean(&Options { config: config.ok_or("missing --config")?, library: library.ok_or("missing --library")?, delete_originals: originals })
}

fn main() -> std::process::ExitCode {
    let result = run();
    let message = match &result {
        Ok(count) => format!("LightCraft cleanup succeeded: removed {count} files.\n"),
        Err(error) => format!("LightCraft cleanup failed: {error}\nClose LightCraft and retry, or clean up the reported path manually.\n"),
    };
    // GUI subsystem opens no console. Preserve an actionable error even when MSI can only
    // show its generic custom-action error, including silent runs.
    let _ = std::fs::write(std::env::temp_dir().join("lightcraft-uninstall.log"), &message);
    if result.is_err() {
        eprintln!("{message}");
        std::process::ExitCode::FAILURE
    } else {
        std::process::ExitCode::SUCCESS
    }
}
