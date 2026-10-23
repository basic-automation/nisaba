//! N-12.3: regenerate the Rust third-party license notices at build time.
//!
//! Runs `cargo-about generate --format json` over the workspace dependency
//! closure, slims the raw JSON down to what the About -> Licenses view needs,
//! and writes `resources/licenses/rust-licenses.json` (only when the content
//! actually changed, so builds don't loop). The file is checked in, bundled
//! via `bundle.resources` in tauri.conf.json, and embedded into the binary
//! with `include_str!` for the `get_third_party_licenses` command.
//!
//! The step reruns only when `Cargo.lock` or `about.toml` changes —
//! `cargo-about` takes minutes on this dependency tree — and it never fails
//! the build: without `cargo-about` on PATH (e.g. CI runners) it warns and
//! keeps the checked-in file.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    tauri_build::build();
    generate_rust_licenses();
}

fn workspace_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("crates/tauri-app is two levels below the workspace root")
        .to_path_buf()
}

fn generate_rust_licenses() {
    let root = workspace_root();
    let about_toml = root.join("about.toml");
    let cargo_lock = root.join("Cargo.lock");
    let out_file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("resources")
        .join("licenses")
        .join("rust-licenses.json");

    // Only rerun the (slow) license scan when the inputs change.
    println!("cargo:rerun-if-changed={}", cargo_lock.display());
    println!("cargo:rerun-if-changed={}", about_toml.display());

    if inputs_unchanged(&out_file, &[&cargo_lock, &about_toml]) {
        return;
    }

    let version = match cargo_about_version() {
        Some(v) => v,
        None => {
            println!(
                "cargo:warning=generate-licenses: `cargo-about` not found on PATH; \
                 keeping checked-in resources/licenses/rust-licenses.json"
            );
            ensure_placeholder(&out_file);
            return;
        }
    };

    let tmp = out_file.with_extension("json.tmp");
    let status = Command::new("cargo-about")
        .args([
            "generate",
            "--format",
            "json",
            "--config",
            &about_toml.to_string_lossy(),
            "--manifest-path",
            &root.join("Cargo.toml").to_string_lossy(),
            "--offline",
            "--locked",
            "--output-file",
            &tmp.to_string_lossy(),
        ])
        .current_dir(&root)
        .status();
    match status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            println!(
                "cargo:warning=generate-licenses: `cargo-about generate` exited with {s}; \
                 keeping checked-in resources/licenses/rust-licenses.json"
            );
            let _ = std::fs::remove_file(&tmp);
            ensure_placeholder(&out_file);
            return;
        }
        Err(e) => {
            println!(
                "cargo:warning=generate-licenses: failed to run `cargo-about`: {e}; \
                 keeping checked-in resources/licenses/rust-licenses.json"
            );
            ensure_placeholder(&out_file);
            return;
        }
    }

    let raw = match std::fs::read_to_string(&tmp) {
        Ok(r) => r,
        Err(e) => {
            println!("cargo:warning=generate-licenses: cannot read cargo-about output: {e}");
            ensure_placeholder(&out_file);
            return;
        }
    };
    let _ = std::fs::remove_file(&tmp);

    match slim_licenses(&raw, &version) {
        Some(slimmed) => write_if_changed(&out_file, &slimmed),
        None => {
            println!(
                "cargo:warning=generate-licenses: failed to parse cargo-about JSON output; \
                 keeping checked-in resources/licenses/rust-licenses.json"
            );
            ensure_placeholder(&out_file);
        }
    }
}

/// Skip the scan when the output is newer than every input.
fn inputs_unchanged(out_file: &Path, inputs: &[&Path]) -> bool {
    let out_mtime = match std::fs::metadata(out_file).and_then(|m| m.modified()) {
        Ok(t) => t,
        Err(_) => return false,
    };
    inputs.iter().all(|p| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .map(|t| t <= out_mtime)
            .unwrap_or(false)
    })
}

fn cargo_about_version() -> Option<String> {
    let out = Command::new("cargo-about").arg("--version").output().ok()?;
    if !out.status.success() {
        return None;
    }
    // "cargo-about 0.9.2" -> "0.9.2"
    String::from_utf8(out.stdout)
        .ok()?
        .trim()
        .strip_prefix("cargo-about")
        .map(|s| s.trim().to_string())
}

/// Reduce cargo-about's raw JSON (which embeds full cargo metadata per crate)
/// to the `{generated_by, licenses[], dependencies[]}` shape the Licenses view uses.
fn slim_licenses(raw: &str, version: &str) -> Option<String> {
    let raw: serde_json::Value = serde_json::from_str(raw).ok()?;

    let mut licenses: Vec<serde_json::Value> = Vec::new();
    for lic in raw.get("licenses")?.as_array()? {
        let mut crates: Vec<serde_json::Value> = Vec::new();
        for used in lic.get("used_by")?.as_array()? {
            let krate = &used["crate"];
            crates.push(serde_json::json!({
                "name": krate["name"],
                "version": krate["version"],
            }));
        }
        crates.sort_by(|a, b| {
            a["name"]
                .as_str()
                .cmp(&b["name"].as_str())
                .then(a["version"].as_str().cmp(&b["version"].as_str()))
        });
        licenses.push(serde_json::json!({
            "id": lic["id"],
            "name": lic["name"],
            "text": lic["text"],
            "crates": crates,
        }));
    }
    licenses.sort_by(|a, b| {
        // Most-used licenses first — the MIT/Apache-2.0 texts land on top.
        b["crates"]
            .as_array()
            .map(|c| c.len())
            .cmp(&a["crates"].as_array().map(|c| c.len()))
            .then(a["id"].as_str().cmp(&b["id"].as_str()))
    });

    let mut dep_crates: Vec<serde_json::Value> = Vec::new();
    for entry in raw.get("crates")?.as_array()? {
        let package = &entry["package"];
        // Skip workspace members (`source: null`): these are Nisaba's own
        // crates, not third-party notices.
        if package["source"].is_null() {
            continue;
        }
        dep_crates.push(serde_json::json!({
            "name": package["name"],
            "version": package["version"],
            "license": entry["license"],
            "repository": package["repository"],
        }));
    }
    dep_crates.sort_by(|a, b| {
        a["name"]
            .as_str()
            .cmp(&b["name"].as_str())
            .then(a["version"].as_str().cmp(&b["version"].as_str()))
    });

    let slimmed = serde_json::json!({
        "generated_by": format!("cargo-about {version}"),
        "licenses": licenses,
        "dependencies": dep_crates,
    });
    serde_json::to_string_pretty(&slimmed).ok().map(|mut s| {
        s.push('\n');
        s
    })
}

fn write_if_changed(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let current = std::fs::read_to_string(path).ok();
    if current.as_deref() != Some(content) {
        if let Err(e) = std::fs::write(path, content) {
            println!(
                "cargo:warning=generate-licenses: cannot write {}: {e}",
                path.display()
            );
        } else {
            println!(
                "cargo:warning=generate-licenses: regenerated {}",
                path.display()
            );
        }
    }
}

/// The command module `include_str!`s the JSON at compile time, so the file
/// must exist even on a machine that has neither the checked-in file nor
/// `cargo-about`. An empty-but-valid placeholder keeps the build working;
/// the real notices come from the checked-in file in every normal checkout.
fn ensure_placeholder(out_file: &Path) {
    if out_file.exists() {
        return;
    }
    write_if_changed(
        out_file,
        "{\"generated_by\":\"unknown\",\"licenses\":[],\"crates\":[]}\n",
    );
}
