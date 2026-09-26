//! Sandbox boundary tests: a vendor plugin is third-party code, and these pin down what
//! it must *not* be able to reach — files outside its own directory (by writing them at
//! install time or importing them at run time), and host capabilities beyond the
//! `op_nisaba_*` surface.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use nisaba_vendor_runtime::VendorRuntime;

/// A fresh directory outside the runtime's own `nisaba_plugins` area, standing in for
/// the rest of the user's filesystem.
fn outside_dir() -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "nisaba_sandbox_test_{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

/// Write a module that holds a secret somewhere a plugin should not be able to import.
fn plant_secret(dir: &std::path::Path, name: &str) -> (PathBuf, String) {
    let secret = format!("s3cr3t-{name}-{}", std::process::id());
    let path = dir.join(name);
    std::fs::write(&path, format!("export const secret = {secret:?};")).unwrap();
    (path, secret)
}

fn file_url(path: &std::path::Path) -> String {
    format!("file://{}", path.display())
}

/// Run a plugin whose `fetchListings` returns a single listing with the given JS
/// expression (a string) as its title.
async fn run_returning_title(title_expr: &str) -> anyhow::Result<String> {
    let code = format!(
        r#"
        export const metadata = {{ name: "probe", version: "1.0.0" }};
        export async function fetchListings() {{
            const title = await (async () => {{ {title_expr} }})();
            return [{{ vendor_item_id: "probe", title: String(title) }}];
        }}
        "#
    );
    let listings = VendorRuntime::execute_plugin(&code, HashMap::new(), None).await?;
    Ok(listings[0].title.clone())
}

fn plugin_files(extra: &[(&str, &str)]) -> HashMap<String, String> {
    let mut files = HashMap::from([(
        "index.ts".to_string(),
        r#"export const metadata = { name: "evil", version: "1.0.0" };
           export async function fetchListings() { return []; }"#
            .to_string(),
    )]);
    for (name, content) in extra {
        files.insert(name.to_string(), content.to_string());
    }
    files
}

#[tokio::test]
async fn plugin_file_names_cannot_write_outside_the_plugin_directory() {
    let outside = outside_dir();
    let absolute = outside.join("absolute.ts");
    // The runtime writes plugins to `<tmp>/nisaba_plugins/plugin_<nonce>/`, so one `..`
    // lands in the shared `nisaba_plugins` dir and two land in the temp dir itself.
    let shared = std::env::temp_dir()
        .join("nisaba_plugins")
        .join(format!("dotdot_{}.ts", std::process::id()));
    let traversal = format!("../dotdot_{}.ts", std::process::id());
    let nested = format!("lib/../../dotdot_{}.ts", std::process::id());

    for name in [
        absolute.to_str().unwrap(),
        traversal.as_str(),
        nested.as_str(),
        "",
    ] {
        let files = plugin_files(&[(name, "export const pwned = true;")]);

        let err = VendorRuntime::read_metadata_from_files(&files)
            .await
            .expect_err(&format!("metadata read accepted file name {name:?}"));
        assert!(
            err.to_string().contains("not a plain relative path"),
            "unexpected error for {name:?}: {err}"
        );
        VendorRuntime::execute_plugin_from_files(&files, HashMap::new(), None)
            .await
            .expect_err(&format!("execution accepted file name {name:?}"));
    }

    assert!(!absolute.exists(), "absolute file name was written");
    assert!(!shared.exists(), "`..` file name was written");
    let _ = std::fs::remove_dir_all(&outside);
}

#[tokio::test]
async fn static_import_outside_the_plugin_is_refused() {
    let outside = outside_dir();
    let (secret_path, secret) = plant_secret(&outside, "static.js");

    let code = format!(
        r#"import {{ secret }} from "{}";
           export const metadata = {{ name: secret, version: "1.0.0" }};"#,
        file_url(&secret_path)
    );
    let err = VendorRuntime::read_metadata(&code)
        .await
        .expect_err("a plugin imported a module outside its directory");
    let msg = format!("{err:#}");
    assert!(msg.contains("outside the plugin directory"), "{msg}");
    assert!(
        !msg.contains(&secret),
        "the refusal leaked the file's contents"
    );
    let _ = std::fs::remove_dir_all(&outside);
}

#[tokio::test]
async fn dynamic_import_outside_the_plugin_is_refused() {
    let outside = outside_dir();
    let (secret_path, secret) = plant_secret(&outside, "dynamic.js");

    let title = run_returning_title(&format!(
        r#"try {{ return (await import("{}")).secret; }}
           catch (e) {{ return "refused: " + e.message; }}"#,
        file_url(&secret_path)
    ))
    .await
    .unwrap();
    assert!(!title.contains(&secret), "dynamic import read {title}");
    assert!(title.contains("outside the plugin directory"), "{title}");
    let _ = std::fs::remove_dir_all(&outside);
}

#[tokio::test]
async fn relative_import_cannot_reach_another_plugins_files() {
    // Every plugin is written under the shared `<tmp>/nisaba_plugins/`, so `../` from
    // one plugin's directory is where every other plugin's files live.
    let shared = std::env::temp_dir().join("nisaba_plugins");
    std::fs::create_dir_all(&shared).unwrap();
    let name = format!("sibling_{}.js", std::process::id());
    let (sibling, secret) = plant_secret(&shared, &name);

    let title = run_returning_title(&format!(
        r#"try {{ return (await import("../{name}")).secret; }}
           catch (e) {{ return "refused: " + e.message; }}"#
    ))
    .await
    .unwrap();
    let _ = std::fs::remove_file(&sibling);
    assert!(!title.contains(&secret), "relative import read {title}");
    assert!(title.contains("outside the plugin directory"), "{title}");
}

#[tokio::test]
async fn imports_inside_the_plugin_still_work() {
    let files = HashMap::from([
        (
            "index.ts".to_string(),
            r#"import { makeTitle } from "./lib/util.ts";
               export const metadata = { name: "multi", version: "1.0.0" };
               export async function fetchListings() {
                   const { suffix } = await import("./lib/dynamic.js");
                   return [{ vendor_item_id: "1", title: makeTitle("ok") + suffix }];
               }"#
            .to_string(),
        ),
        (
            "lib/util.ts".to_string(),
            "export function makeTitle(s: string): string { return `title-${s}`; }".to_string(),
        ),
        (
            "lib/dynamic.js".to_string(),
            r#"export const suffix = "-dyn";"#.to_string(),
        ),
    ]);
    let listings = VendorRuntime::execute_plugin_from_files(&files, HashMap::new(), None)
        .await
        .unwrap();
    assert_eq!(listings[0].title, "title-ok-dyn");
}

#[tokio::test]
async fn the_deno_global_is_not_reachable_from_plugin_code() {
    // `Deno.core.ops` exposes every deno_core built-in op. Among them `op_panic` panics
    // across the V8 boundary, which cannot unwind, so a single call from any plugin used
    // to abort the whole app. If this regresses the probe below aborts the test binary.
    let title = run_returning_title(
        r#"
        const reached = [];
        if (typeof Deno !== "undefined") reached.push("Deno");
        if (typeof globalThis.__bootstrap !== "undefined") reached.push("__bootstrap");
        // Same-realm tricks that evaluate in the global scope.
        if (new Function("return typeof Deno")() !== "undefined") reached.push("Function");
        if ((0, eval)("typeof Deno") !== "undefined") reached.push("eval");
        globalThis.Deno?.core?.ops?.op_panic?.("plugin reached op_panic");
        return reached.join(",");
        "#,
    )
    .await
    .unwrap();
    assert_eq!(title, "", "plugin code can reach: {title}");
}

#[tokio::test]
async fn internal_extension_modules_cannot_be_imported() {
    // deno_core registers its internals as `ext:` modules — `ext:core/ops` exports every
    // op — so importing one would hand back what hiding `Deno` took away.
    for specifier in [
        "ext:core/ops",
        "ext:core/mod.js",
        "ext:nisaba_vendor_ext/runtime_js.js",
    ] {
        let title = run_returning_title(&format!(
            r#"try {{ const m = await import("{specifier}"); return "imported: " + Object.keys(m).join("/"); }}
               catch (e) {{ return "refused"; }}"#
        ))
        .await
        .unwrap();
        assert_eq!(title, "refused", "{specifier} → {title}");
    }
}

#[tokio::test]
async fn no_host_capabilities_are_exposed_to_plugin_code() {
    let title = run_returning_title(
        r#"
        const globals = ["fetch", "XMLHttpRequest", "WebSocket", "require", "process",
                         "Bun", "setTimeout", "setInterval", "Worker"];
        return globals.filter((g) => typeof globalThis[g] !== "undefined").join(",");
        "#,
    )
    .await
    .unwrap();
    assert_eq!(title, "", "plugin code can reach: {title}");
}

#[tokio::test]
async fn the_plugin_api_surface_is_pinned() {
    // `Nisaba` is the whole API a plugin sees, and these ops are everything behind it.
    // Widening either must be a deliberate change to this test.
    let title = run_returning_title(
        r#"return Object.keys(Nisaba).sort().join(",") + "|" + Object.keys(Nisaba.log).sort().join(",");"#,
    )
    .await
    .unwrap();
    assert_eq!(
        title,
        "emitBatch,fetch,log,sleep|debug,error,info,trace,warn"
    );

    let ext = nisaba_vendor_runtime::nisaba_vendor_ext::init();
    let mut ops: Vec<&str> = ext.ops.iter().map(|op| op.name).collect();
    ops.sort();
    assert_eq!(
        ops,
        [
            "op_nisaba_emit_batch",
            "op_nisaba_fetch",
            "op_nisaba_log",
            "op_nisaba_set_result",
            "op_nisaba_sleep",
        ]
    );
}
