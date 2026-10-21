//! Plugin API versioning: a plugin declares `api_version` in its metadata and the
//! host installs only the major it implements. These exercise the full path — the
//! metadata is read through the JS runtime exactly as at install time, then the
//! same check `install_vendor_plugin` runs decides the outcome.

use nisaba_vendor_runtime::{VendorRuntime, HOST_PLUGIN_API_VERSION};

fn template_source() -> String {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/template/index.ts");
    std::fs::read_to_string(path).unwrap()
}

#[tokio::test]
async fn the_template_declares_api_version_1() {
    let meta = VendorRuntime::read_metadata(&template_source())
        .await
        .unwrap();
    assert_eq!(meta.api_version, 1);
    meta.check_api_version()
        .expect("the template must be installable on this host");
}

#[tokio::test]
async fn api_version_1_is_accepted() {
    let meta = VendorRuntime::read_metadata(
        r#"export const metadata = { name: "v1 plugin", version: "1.0.0", api_version: 1 };"#,
    )
    .await
    .unwrap();
    assert_eq!(meta.api_version, HOST_PLUGIN_API_VERSION);
    meta.check_api_version().unwrap();
}

#[tokio::test]
async fn an_unknown_major_is_refused_with_a_clear_message() {
    let meta = VendorRuntime::read_metadata(
        r#"export const metadata = { name: "future plugin", version: "9.0.0", api_version: 2 };"#,
    )
    .await
    .unwrap();
    let err = meta
        .check_api_version()
        .expect_err("an unknown major must be refused");
    assert!(
        err.contains("plugin API version 2"),
        "the message names the plugin's version, got: {err}"
    );
    assert!(
        err.contains(&format!("version {HOST_PLUGIN_API_VERSION}")),
        "the message names the supported version, got: {err}"
    );
    assert!(
        err.contains("future plugin"),
        "the message names the plugin, got: {err}"
    );
}

#[tokio::test]
async fn a_missing_api_version_is_refused() {
    // Pre-freeze metadata has no api_version; it deserializes to 0, which is not
    // a real revision and is refused just like an unknown major.
    let meta = VendorRuntime::read_metadata(
        r#"export const metadata = { name: "old plugin", version: "0.9.0" };"#,
    )
    .await
    .unwrap();
    assert_eq!(meta.api_version, 0);
    let err = meta
        .check_api_version()
        .expect_err("a missing api_version must be refused");
    assert!(
        err.contains("plugin API version 0"),
        "the message says what was declared, got: {err}"
    );
}
