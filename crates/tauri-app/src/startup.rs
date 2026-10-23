//! N-11.5: startup failures are visible instead of silent panics.
//!
//! The release build ships with `windows_subsystem = "windows"`, so on
//! Windows there is no console: an `expect()` on config load or database
//! open used to kill the process with no explanation. [`fatal_startup_error`]
//! replaces those panics — it names the failing stage, writes a timestamped
//! log file, shows a native OS error dialog, and exits non-zero.
//!
//! # Documented log location
//!
//! `<data dir>/logs/startup-errors/startup-error-<UTC timestamp>.log`, where
//! the data dir is `%APPDATA%\nisaba` on Windows and `~/.config/nisaba` on
//! Unix. If the data dir cannot be determined (or created), the current
//! working directory is used as a fallback.
//!
//! The log file contains the failing stage, the full error text, the app
//! version, and the platform, so a bug report can quote it verbatim.

use std::fmt::Display;
use std::path::{Path, PathBuf};

/// Report a fatal startup failure and never return.
///
/// Best effort at every step: the log file and the dialog may both fail
/// (e.g. a read-only home directory and no dialog backend); the report is
/// always written to stderr as well, and the process exits with code 1.
pub fn fatal_startup_error(stage: &str, err: impl Display) -> ! {
    let report = format_startup_error_report(stage, &err.to_string());

    let log_note = match write_startup_error_log(&report) {
        Some(path) => format!("A log was written to {}", path.display()),
        None => "No startup-error log file could be written.".to_string(),
    };

    let title = "Nisaba failed to start".to_string();
    let message = format!("{stage} failed.\n\n{err}\n\n{log_note}");
    show_native_error_dialog(&title, &truncate_message(&message, 1800));

    // Last resort when there is no console-free dialog backend (or the
    // dialog itself failed): at least leave the full report on stderr.
    eprintln!("{report}");
    std::process::exit(1);
}

/// Build the human-readable startup-failure report (also the log content).
fn format_startup_error_report(stage: &str, error: &str) -> String {
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC");
    format!(
        "Nisaba failed to start\n\
         =====================\n\
         When:     {now}\n\
         Version:  {version}\n\
         Platform: {platform}\n\
         Stage:    {stage}\n\
         \n\
         Error:\n{error}\n",
        version = env!("CARGO_PKG_VERSION"),
        platform = std::env::consts::OS,
    )
}

/// Directory for startup-error logs (documented above).
pub fn startup_error_log_dir() -> PathBuf {
    match nisaba_core::config::AppConfig::data_dir() {
        Ok(dir) => dir.join("logs").join("startup-errors"),
        Err(_) => std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir()),
    }
}

/// Write the report to a timestamped file under [`startup_error_log_dir`].
fn write_startup_error_log(report: &str) -> Option<PathBuf> {
    let dir = startup_error_log_dir();
    match write_report_to_dir(&dir, report) {
        Ok(path) => Some(path),
        Err(e) => {
            eprintln!(
                "Could not write startup-error log to {}: {e}",
                dir.display()
            );
            None
        }
    }
}

/// Write `report` to a timestamped `startup-error-*.log` file in `dir`.
fn write_report_to_dir(dir: &Path, report: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let path = dir.join(format!("startup-error-{stamp}.log"));
    std::fs::write(&path, report)?;
    Ok(path)
}

/// Cap a message at `max_chars` characters so native dialogs don't get a
/// multi-megabyte wall of text.
fn truncate_message(message: &str, max_chars: usize) -> String {
    if message.chars().count() <= max_chars {
        message.to_string()
    } else {
        let truncated: String = message.chars().take(max_chars).collect();
        format!("{truncated}\n…(truncated)")
    }
}

/// Show a blocking native OS error dialog. Best effort: never panics, never
/// blocks longer than the dialog is open.
fn show_native_error_dialog(title: &str, message: &str) {
    #[cfg(windows)]
    // SAFETY: MessageBoxW is called with valid NUL-terminated UTF-16
    // buffers and a NULL owner window; it blocks until dismissed.
    unsafe {
        windows_message_box(title, message)
    };

    #[cfg(target_os = "macos")]
    macos_alert(title, message);

    #[cfg(not(any(windows, target_os = "macos")))]
    linux_dialog(title, message);
}

/// Native error dialog on Windows via `MessageBoxW` (no new dependency).
#[cfg(windows)]
unsafe fn windows_message_box(title: &str, message: &str) {
    use std::os::windows::ffi::OsStrExt;

    type Hwnd = *mut std::ffi::c_void;
    unsafe extern "system" {
        fn MessageBoxW(h_wnd: Hwnd, text: *const u16, caption: *const u16, kind: u32) -> i32;
    }

    fn wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    let title_w = wide(title);
    let message_w = wide(message);
    // MB_OK | MB_ICONERROR | MB_SYSTEMMODAL
    const FLAGS: u32 = 0x0000_0000 | 0x0000_0010 | 0x0000_1000;
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            message_w.as_ptr(),
            title_w.as_ptr(),
            FLAGS,
        );
    }
}

/// Native error dialog on macOS via the bundled `osascript`.
#[cfg(target_os = "macos")]
fn macos_alert(title: &str, message: &str) {
    fn applescript_escape(s: &str) -> String {
        s.replace('\\', "\\\\").replace('"', "\\\"")
    }
    let script = format!(
        "display alert \"{}\" message \"{}\" as critical",
        applescript_escape(title),
        applescript_escape(message)
    );
    let _ = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .status();
}

/// Native error dialog on Linux via `zenity` then `kdialog`; both may be
/// absent, in which case the log file and stderr carry the report.
#[cfg(not(any(windows, target_os = "macos")))]
fn linux_dialog(title: &str, message: &str) {
    let zenity_ok = std::process::Command::new("zenity")
        .args([
            "--error",
            "--no-wrap",
            &format!("--title={title}"),
            &format!("--text={message}"),
        ])
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if zenity_ok {
        return;
    }
    let _ = std::process::Command::new("kdialog")
        .args(["--error", message, "--title", title])
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_names_stage_error_and_version() {
        let report = format_startup_error_report("database", "disk is on fire");
        assert!(report.contains("database"), "stage missing: {report}");
        assert!(
            report.contains("disk is on fire"),
            "error missing: {report}"
        );
        assert!(
            report.contains(env!("CARGO_PKG_VERSION")),
            "version missing: {report}"
        );
    }

    #[test]
    fn report_is_written_to_a_timestamped_log_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_report_to_dir(dir.path(), "hello").expect("write");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "hello");
        let name = path.file_name().unwrap().to_string_lossy();
        assert!(
            name.starts_with("startup-error-") && name.ends_with(".log"),
            "unexpected file name: {name}"
        );
    }

    #[test]
    fn message_truncation_keeps_short_text_and_caps_long_text() {
        assert_eq!(truncate_message("short", 10), "short");
        let long = "x".repeat(100);
        let truncated = truncate_message(&long, 10);
        assert!(truncated.starts_with(&"x".repeat(10)));
        assert!(truncated.contains("truncated"));
    }
}
