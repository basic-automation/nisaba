//! N-12.3: third-party license notices for the About -> Licenses view.
//!
//! The notices are generated at build time — `cargo-about` for the Rust
//! dependency closure (see `crates/tauri-app/build.rs` and the workspace-root
//! `about.toml`), `license-checker` for the npm closure (see
//! `scripts/generate-licenses.mjs`, wired into the frontend `pregenerate`
//! hook) — and bundled in the app resources (`bundle.resources` in
//! tauri.conf.json). They are also embedded in the binary via `include_str!`
//! so the view works identically in dev and in every packaged build; a test
//! below asserts the bundled resource files are byte-identical to what is
//! embedded.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// One third-party crate or npm package.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ThirdPartyDependency {
    pub name: String,
    pub version: String,
    /// SPDX license expression, e.g. `"MIT OR Apache-2.0"`.
    pub license: String,
    pub repository: Option<String>,
}

/// A `name`/`version` reference to one dependency.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DependencyRef {
    pub name: String,
    pub version: String,
}

/// One distinct license text shared by a set of dependencies.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ThirdPartyLicenseText {
    /// SPDX license id, e.g. `"MIT"`.
    pub id: String,
    /// Human-readable name, e.g. `"MIT License"`.
    pub name: String,
    /// The full license text that MIT/Apache-2.0 require us to ship.
    pub text: String,
    /// Each dependency using this text.
    pub crates: Vec<DependencyRef>,
}

/// The Rust (cargo-about) half of the notices.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct RustLicenses {
    pub generated_by: String,
    pub licenses: Vec<ThirdPartyLicenseText>,
    pub dependencies: Vec<ThirdPartyDependency>,
}

/// One npm package entry.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct NpmPackage {
    pub name: String,
    pub version: String,
    pub licenses: Vec<String>,
    pub repository: Option<String>,
    pub publisher: Option<String>,
}

/// One distinct npm license file text, deduplicated by content.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct NpmLicenseText {
    /// The license expression shared by the packages below, e.g. `"MIT"`.
    pub licenses: String,
    pub text: String,
    /// `"name@version"` of each package whose license file had this text.
    pub packages: Vec<String>,
}

/// The npm (license-checker) half of the notices.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct NpmLicenses {
    pub generated_by: String,
    pub packages: Vec<NpmPackage>,
    pub license_texts: Vec<NpmLicenseText>,
}

/// Both halves, as served to the About -> Licenses view.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ThirdPartyLicenses {
    pub rust: RustLicenses,
    pub npm: NpmLicenses,
}

/// Raw generated files, embedded at compile time. `build.rs` regenerates
/// them when `Cargo.lock`/`about.toml` change; the frontend `pregenerate`
/// hook regenerates the npm half on every `nuxt generate`.
const RUST_LICENSES_JSON: &str = include_str!("../../resources/licenses/rust-licenses.json");
const NPM_LICENSES_JSON: &str = include_str!("../../resources/licenses/npm-licenses.json");

/// Serve the bundled third-party license notices to the Licenses view.
#[tauri::command]
pub fn get_third_party_licenses() -> Result<ThirdPartyLicenses, String> {
    let rust: RustLicenses = serde_json::from_str(RUST_LICENSES_JSON)
        .map_err(|e| format!("invalid rust-licenses.json: {e}"))?;
    let npm: NpmLicenses = serde_json::from_str(NPM_LICENSES_JSON)
        .map_err(|e| format!("invalid npm-licenses.json: {e}"))?;
    Ok(ThirdPartyLicenses { rust, npm })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Paths of the notices inside the app resources (tauri.conf.json
    /// `bundle.resources`), relative to the tauri-app crate directory.
    const RUST_LICENSES_RESOURCE: &str = "resources/licenses/rust-licenses.json";
    const NPM_LICENSES_RESOURCE: &str = "resources/licenses/npm-licenses.json";

    fn resource_path(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(name)
    }

    #[test]
    fn embedded_notices_parse() {
        let licenses = get_third_party_licenses().expect("embedded notices must parse");
        assert!(
            !licenses.rust.dependencies.is_empty(),
            "expected Rust third-party dependencies"
        );
        assert!(
            !licenses.rust.licenses.is_empty(),
            "expected Rust license texts"
        );
        assert!(!licenses.npm.packages.is_empty(), "expected npm packages");
    }

    #[test]
    fn rust_dependencies_are_well_formed() {
        let licenses = get_third_party_licenses().expect("embedded notices must parse");
        for dep in &licenses.rust.dependencies {
            assert!(!dep.name.is_empty(), "dependency name must not be empty");
            assert!(
                !dep.version.is_empty(),
                "dependency version must not be empty"
            );
            assert!(
                !dep.license.is_empty(),
                "dependency {} must declare a license",
                dep.name
            );
        }
        for lic in &licenses.rust.licenses {
            assert!(!lic.id.is_empty(), "license id must not be empty");
            assert!(
                !lic.text.trim().is_empty(),
                "license {} must ship its text",
                lic.id
            );
            assert!(
                !lic.crates.is_empty(),
                "license {} must list the crates using it",
                lic.id
            );
        }
        // Sanity: this workspace pulls in hundreds of crates under dozens of
        // licenses; a much smaller file means generation silently broke.
        assert!(
            licenses.rust.dependencies.len() > 100,
            "unexpectedly few Rust dependencies: {}",
            licenses.rust.dependencies.len()
        );
    }

    #[test]
    fn npm_packages_are_well_formed() {
        let licenses = get_third_party_licenses().expect("embedded notices must parse");
        for pkg in &licenses.npm.packages {
            assert!(!pkg.name.is_empty(), "package name must not be empty");
            assert!(!pkg.version.is_empty(), "package version must not be empty");
        }
        assert!(
            licenses.npm.packages.len() > 50,
            "unexpectedly few npm packages: {}",
            licenses.npm.packages.len()
        );
    }

    /// The files bundled in the app resources must be byte-identical to what
    /// the binary embeds; otherwise the Licenses view and the shipped
    /// notices could drift apart.
    #[test]
    fn bundled_resources_match_embedded() {
        let rust_on_disk = std::fs::read_to_string(resource_path(RUST_LICENSES_RESOURCE))
            .expect("rust-licenses.json must exist");
        let npm_on_disk = std::fs::read_to_string(resource_path(NPM_LICENSES_RESOURCE))
            .expect("npm-licenses.json must exist");
        assert_eq!(
            rust_on_disk, RUST_LICENSES_JSON,
            "resources/licenses/rust-licenses.json drifted from the embedded copy"
        );
        assert_eq!(
            npm_on_disk, NPM_LICENSES_JSON,
            "resources/licenses/npm-licenses.json drifted from the embedded copy"
        );
    }
}
