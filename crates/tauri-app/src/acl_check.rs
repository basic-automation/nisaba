//! N-2.7 targeted check: the Tauri ACL allow list must stay in sync with the
//! commands registered in `tauri::generate_handler!`.
//!
//! Tauri ACL command names do not support wildcards, and once ANY app
//! permission exists, every app command not explicitly allowed for a window is
//! denied there. These tests fail the build if a command is added to the
//! handler without updating `permissions/main-window.toml`.
//!
//! The main window renders untrusted HTML with full IPC access and also hosts
//! Settings, so no command may return a secret (see `nisaba_core::masked`).

/// Commands that used to return a secret to the webview. They must never be
/// registered again.
#[cfg(test)]
const RETIRED_SECRET_COMMANDS: &[&str] = &["get_company_secret"];

/// Extract the command idents registered in `tauri::generate_handler![...]`
/// in src/main.rs.
#[cfg(test)]
fn handler_commands() -> Vec<String> {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let main_rs = std::fs::read_to_string(format!("{manifest_dir}/src/main.rs"))
        .expect("could not read src/main.rs");
    let start = main_rs
        .find("generate_handler![")
        .expect("generate_handler! not found in src/main.rs");
    let body = &main_rs[start..];
    // The handler list ends at the first `])` after the macro invocation.
    let end = body.find("])").expect("end of generate_handler! not found");
    let body = &body[..end];

    let mut cmds: Vec<String> = body
        .split("commands::")
        .skip(1)
        .filter_map(|rest| {
            // rest looks like `<module>::<command>,` possibly with whitespace
            let mut parts = rest.splitn(2, "::");
            parts.next()?; // module
            let cmd = parts.next()?;
            let cmd: String = cmd
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if cmd.is_empty() {
                None
            } else {
                Some(cmd)
            }
        })
        .collect();
    cmds.sort();
    cmds.dedup();
    assert!(
        !cmds.is_empty(),
        "no commands extracted from generate_handler!"
    );
    cmds
}

/// Naively extract the `commands.allow` string list from an app permission
/// TOML file (no TOML dependency needed for this shape).
#[cfg(test)]
fn permission_allow_list(file: &str) -> Vec<String> {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let toml = std::fs::read_to_string(format!("{manifest_dir}/permissions/{file}"))
        .unwrap_or_else(|_| panic!("could not read permissions/{file}"));
    let marker = "commands.allow = [";
    let start = toml
        .find(marker)
        .unwrap_or_else(|| panic!("commands.allow not found in permissions/{file}"));
    let body = &toml[start + marker.len()..];
    let end = body
        .find(']')
        .unwrap_or_else(|| panic!("unterminated commands.allow in permissions/{file}"));
    let body = &body[..end];

    let mut cmds: Vec<String> = body
        .split('"')
        .skip(1)
        .step_by(2)
        .map(|s| s.to_string())
        .collect();
    cmds.sort();
    cmds.dedup();
    assert!(!cmds.is_empty(), "empty allow list in permissions/{file}");
    cmds
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_window_allows_every_handler_command() {
        let handler = handler_commands();
        let allowed = permission_allow_list("main-window.toml");
        // Exact match: a typo'd or stale entry is a bug, not a warning.
        assert_eq!(
            allowed, handler,
            "main-window.toml allow list must exactly match generate_handler! commands"
        );
    }

    #[test]
    fn secret_returning_commands_stay_retired() {
        let handler = handler_commands();
        for retired in RETIRED_SECRET_COMMANDS {
            assert!(
                !handler.contains(&retired.to_string()),
                "{retired} returns a secret to the webview and must not be registered"
            );
        }
    }

    #[test]
    fn there_is_no_second_window_capability() {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let mut caps: Vec<String> = std::fs::read_dir(format!("{manifest_dir}/capabilities"))
            .expect("capabilities dir")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        caps.sort();
        assert_eq!(caps, ["default.json"], "one window, one capability");
    }
}
