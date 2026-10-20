use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use anyhow::{anyhow, Result};
use deno_core::{
    v8, JsRuntime, ModuleLoadOptions, ModuleLoadReferrer, ModuleLoadResponse, ModuleSource,
    ModuleSourceCode, ModuleSpecifier, ModuleType, RuntimeOptions,
};
use deno_error::JsErrorBox;
use tracing::debug;

use crate::ops::{BatchCallback, FetchPolicy, MaxResponseBytes, OutputBudget, PluginResult};
use crate::types::{PluginMetadata, VendorListing};

/// Custom module loader that transpiles TypeScript on the fly.
///
/// Module loading is the one path by which plugin code reaches the filesystem, so the
/// loader is confined to `root` — the directory holding this plugin's own files. Any
/// `import`/`import()` that resolves outside it (an absolute `file://` URL, a `../`
/// escape, a symlink pointing out) is refused before the file is read.
struct TsModuleLoader {
    root: PathBuf,
}

impl deno_core::ModuleLoader for TsModuleLoader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        _kind: deno_core::ResolutionKind,
    ) -> Result<ModuleSpecifier, JsErrorBox> {
        deno_core::resolve_import(specifier, referrer)
            .map_err(|e| JsErrorBox::generic(e.to_string()))
    }

    fn load(
        &self,
        module_specifier: &ModuleSpecifier,
        _maybe_referrer: Option<&ModuleLoadReferrer>,
        _options: ModuleLoadOptions,
    ) -> ModuleLoadResponse {
        let specifier = module_specifier.clone();
        let root = self.root.clone();
        ModuleLoadResponse::Async(Box::pin(async move {
            let path = specifier.to_file_path().map_err(|_| {
                JsErrorBox::generic(format!(
                    "Cannot convert specifier to file path: {specifier}"
                ))
            })?;

            // Canonicalize before the containment check so `..` and symlinks cannot
            // walk out. The error deliberately does not say whether the file exists.
            let path = tokio::fs::canonicalize(&path)
                .await
                .ok()
                .filter(|p| p.starts_with(&root))
                .ok_or_else(|| {
                    JsErrorBox::generic(format!(
                        "Module {specifier} is outside the plugin directory"
                    ))
                })?;

            let code = tokio::fs::read_to_string(&path).await.map_err(|e| {
                JsErrorBox::generic(format!("Failed to read {}: {e}", path.display()))
            })?;

            let is_ts = path
                .extension()
                .map(|e| e == "ts" || e == "tsx")
                .unwrap_or(false);

            let js_code = if is_ts {
                let parsed = deno_ast::parse_module(deno_ast::ParseParams {
                    specifier: specifier.clone(),
                    text: code.into(),
                    media_type: deno_ast::MediaType::TypeScript,
                    capture_tokens: false,
                    scope_analysis: false,
                    maybe_syntax: None,
                })
                .map_err(|e| JsErrorBox::generic(format!("TypeScript parse error: {e}")))?;

                let transpiled = parsed
                    .transpile(
                        &deno_ast::TranspileOptions::default(),
                        &deno_ast::TranspileModuleOptions::default(),
                        &deno_ast::EmitOptions {
                            source_map: deno_ast::SourceMapOption::None,
                            ..Default::default()
                        },
                    )
                    .map_err(|e| JsErrorBox::generic(format!("TypeScript transpile error: {e}")))?;

                transpiled.into_source().text.to_string()
            } else {
                code
            };

            Ok(ModuleSource::new(
                ModuleType::JavaScript,
                ModuleSourceCode::String(js_code.into()),
                &specifier,
                None,
            ))
        }))
    }
}

/// Resource limits for a single plugin run. A plugin is third-party code: without these
/// a busy loop hangs its thread forever, and a runaway allocation hits V8's fatal
/// out-of-memory handler, which aborts the whole app.
#[derive(Debug, Clone)]
pub struct PluginLimits {
    /// Wall-clock budget for the whole run, busy or waiting.
    pub timeout: Duration,
    /// Ceiling on the plugin's V8 heap.
    pub max_heap_bytes: usize,
    /// Ceiling on the listing JSON handed to the host, summed over every `emitBatch` and
    /// the final result.
    pub max_output_bytes: usize,
    /// Ceiling on any single `Nisaba.fetch` response body.
    pub max_response_bytes: usize,
}

impl Default for PluginLimits {
    /// Limits for `fetchListings`: generous enough for a full supplier catalog pulled
    /// page by page with rate-limit sleeps.
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30 * 60),
            max_heap_bytes: 1024 * 1024 * 1024,
            max_output_bytes: 256 * 1024 * 1024,
            max_response_bytes: 64 * 1024 * 1024,
        }
    }
}

impl PluginLimits {
    /// Limits for a metadata read, which should do nothing but evaluate the module.
    pub fn metadata() -> Self {
        Self {
            timeout: Duration::from_secs(10),
            max_heap_bytes: 128 * 1024 * 1024,
            max_output_bytes: 1024 * 1024,
            max_response_bytes: 1024 * 1024,
        }
    }
}

const STOPPED_NONE: u8 = 0;
const STOPPED_TIMEOUT: u8 = 1;
const STOPPED_HEAP: u8 = 2;

/// The vendor plugin runtime — wraps a deno_core JsRuntime.
///
/// Every entry point writes the plugin into a fresh directory of its own under the
/// system temp dir, runs it with the module loader confined to that directory, and
/// removes the directory afterwards whether or not the plugin succeeded.
pub struct VendorRuntime;

impl VendorRuntime {
    /// Read plugin metadata without executing fetchListings.
    pub async fn read_metadata(plugin_code: &str) -> Result<PluginMetadata> {
        Self::read_metadata_from_files(&single_file(plugin_code)).await
    }

    /// Read metadata from a plugin file on disk. The plugin may import files from the
    /// directory it sits in, and nothing outside it.
    pub async fn read_metadata_from_path(path: &str) -> Result<PluginMetadata> {
        let path = tokio::fs::canonicalize(path)
            .await
            .map_err(|e| anyhow!("Invalid plugin path {path}: {e}"))?;
        let root = path
            .parent()
            .ok_or_else(|| anyhow!("Plugin path has no parent directory"))?
            .to_path_buf();
        let wrapper_path = root.join(format!("_wrapper_{}.js", nonce()));
        let result = Self::read_metadata_in(&root, &path, &wrapper_path).await;
        let _ = tokio::fs::remove_file(&wrapper_path).await;
        result
    }

    /// Execute a plugin's fetchListings function with the given config.
    pub async fn execute_plugin(
        plugin_code: &str,
        config: HashMap<String, String>,
        batch_callback: Option<BatchCallback>,
    ) -> Result<Vec<VendorListing>> {
        Self::execute_plugin_from_files(&single_file(plugin_code), config, batch_callback).await
    }

    /// Write a multi-file plugin's files to a fresh temp directory.
    /// Skips `data:` URLs (binary icon assets stored inline).
    /// Returns `(temp_dir, index_path)`. Validates `index.ts` exists and that every file
    /// name is a plain relative path, so a plugin cannot write outside its directory.
    async fn write_plugin_files(files: &HashMap<String, String>) -> Result<(PathBuf, PathBuf)> {
        if !files.contains_key("index.ts") {
            return Err(anyhow!("Plugin must contain an index.ts file"));
        }
        // Validate everything before touching the disk, so a rejected plugin leaves
        // nothing behind.
        for filename in files.keys() {
            validate_plugin_file_name(filename)?;
        }

        let plugin_dir = std::env::temp_dir()
            .join("nisaba_plugins")
            .join(format!("plugin_{}", nonce()));
        tokio::fs::create_dir_all(&plugin_dir).await?;
        // Canonical, so the loader's containment check compares like with like
        // (the temp dir itself may sit behind a symlink).
        let plugin_dir = tokio::fs::canonicalize(&plugin_dir).await?;

        for (filename, content) in files {
            // Skip data URLs (binary assets like icons)
            if content.starts_with("data:") {
                continue;
            }
            let file_path = plugin_dir.join(filename);
            // Create parent directories for nested files
            if let Some(parent) = file_path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            if let Err(e) = tokio::fs::write(&file_path, content).await {
                let _ = tokio::fs::remove_dir_all(&plugin_dir).await;
                return Err(e.into());
            }
        }

        let index_path = plugin_dir.join("index.ts");
        Ok((plugin_dir, index_path))
    }

    /// Read metadata from a multi-file plugin (HashMap of filename → content).
    pub async fn read_metadata_from_files(
        files: &HashMap<String, String>,
    ) -> Result<PluginMetadata> {
        let (plugin_dir, index_path) = Self::write_plugin_files(files).await?;
        let wrapper_path = plugin_dir.join(format!("_wrapper_{}.js", nonce()));
        let result = Self::read_metadata_in(&plugin_dir, &index_path, &wrapper_path).await;
        let _ = tokio::fs::remove_dir_all(&plugin_dir).await;
        result
    }

    /// Execute a multi-file plugin's fetchListings with the given config, under the
    /// default [`PluginLimits`].
    pub async fn execute_plugin_from_files(
        files: &HashMap<String, String>,
        config: HashMap<String, String>,
        batch_callback: Option<BatchCallback>,
    ) -> Result<Vec<VendorListing>> {
        Self::execute_plugin_from_files_with_limits(
            files,
            config,
            batch_callback,
            PluginLimits::default(),
        )
        .await
    }

    /// Execute a multi-file plugin's fetchListings with the given config and limits.
    pub async fn execute_plugin_from_files_with_limits(
        files: &HashMap<String, String>,
        config: HashMap<String, String>,
        batch_callback: Option<BatchCallback>,
        limits: PluginLimits,
    ) -> Result<Vec<VendorListing>> {
        let (plugin_dir, index_path) = Self::write_plugin_files(files).await?;
        let result =
            Self::execute_in(&plugin_dir, &index_path, config, batch_callback, &limits).await;
        let _ = tokio::fs::remove_dir_all(&plugin_dir).await;
        result
    }

    async fn read_metadata_in(
        root: &Path,
        index_path: &Path,
        wrapper_path: &Path,
    ) -> Result<PluginMetadata> {
        let plugin_url = ModuleSpecifier::from_file_path(index_path)
            .map_err(|_| anyhow!("Invalid plugin path: {}", index_path.display()))?;

        let wrapper_code = format!(
            r#"
            import {{ metadata }} from "{}";
            __nisabaSetResult(JSON.stringify(metadata));
            "#,
            plugin_url
        );

        let runtime = Self::run_wrapper(
            root,
            wrapper_path,
            &wrapper_code,
            None,
            &PluginLimits::metadata(),
            FetchPolicy::Deny,
        )
        .await?;

        let op_state = runtime.op_state();
        let state = op_state.borrow();
        let result = state
            .try_borrow::<PluginResult>()
            .ok_or_else(|| anyhow!("Plugin did not export metadata"))?;

        let metadata: PluginMetadata = serde_json::from_str(&result.0)
            .map_err(|e| anyhow!("Failed to parse plugin metadata: {e}"))?;

        Ok(metadata)
    }

    async fn execute_in(
        root: &Path,
        index_path: &Path,
        config: HashMap<String, String>,
        batch_callback: Option<BatchCallback>,
        limits: &PluginLimits,
    ) -> Result<Vec<VendorListing>> {
        // The fetch allowlist comes from the plugin's metadata, read in an isolate of its
        // own and applied from Rust before any of this run's plugin code executes — so
        // module top-level code cannot get in first and widen it.
        let meta_wrapper = root.join(format!("_wrapper_{}.js", nonce()));
        let metadata = Self::read_metadata_in(root, index_path, &meta_wrapper).await?;
        let fetch_policy = match metadata.allowed_hosts {
            None => FetchPolicy::Unrestricted,
            Some(hosts) => FetchPolicy::AllowList(Arc::new(hosts)),
        };

        let plugin_url = ModuleSpecifier::from_file_path(index_path)
            .map_err(|_| anyhow!("Invalid plugin path"))?;

        let config_json = serde_json::to_string(&config)?;

        let wrapper_code = format!(
            r#"
            import {{ fetchListings }} from "{}";
            const config = {};
            const result = await fetchListings(config);
            __nisabaSetResult(JSON.stringify(result));
            "#,
            plugin_url, config_json
        );

        let wrapper_path = root.join(format!("_run_{}.js", nonce()));
        let runtime = Self::run_wrapper(
            root,
            &wrapper_path,
            &wrapper_code,
            batch_callback,
            limits,
            fetch_policy,
        )
        .await?;

        let op_state = runtime.op_state();
        let state = op_state.borrow();
        let result = state
            .try_borrow::<PluginResult>()
            .ok_or_else(|| anyhow!("Plugin did not return results"))?;

        debug!("Plugin returned {} bytes of JSON", result.0.len());

        let listings: Vec<VendorListing> = serde_json::from_str(&result.0)
            .map_err(|e| anyhow!("Failed to parse plugin result: {e}"))?;

        Ok(listings)
    }

    /// Write `wrapper_code` to `wrapper_path` inside `root`, then load and evaluate it to
    /// completion in a runtime confined to `root` and bounded by `limits`.
    async fn run_wrapper(
        root: &Path,
        wrapper_path: &Path,
        wrapper_code: &str,
        batch_callback: Option<BatchCallback>,
        limits: &PluginLimits,
        fetch_policy: FetchPolicy,
    ) -> Result<JsRuntime> {
        tokio::fs::write(wrapper_path, wrapper_code).await?;

        let wrapper_url = ModuleSpecifier::from_file_path(wrapper_path)
            .map_err(|_| anyhow!("Invalid wrapper path"))?;

        let mut runtime = Self::create_runtime(root.to_path_buf(), limits);
        let stopped = Arc::new(AtomicU8::new(STOPPED_NONE));

        // Near the heap ceiling, stop the plugin rather than letting V8 hit its fatal OOM
        // handler. The returned limit is the headroom the isolate needs to unwind.
        let isolate = runtime.v8_isolate().thread_safe_handle();
        let heap_stop = stopped.clone();
        runtime.add_near_heap_limit_callback(move |current, _initial| {
            heap_stop.store(STOPPED_HEAP, Ordering::SeqCst);
            isolate.terminate_execution();
            current * 2
        });

        // A plugin spinning in JS never yields to tokio, so the timer below cannot fire;
        // a watchdog thread terminates the isolate from outside instead. Dropping
        // `_watchdog` disconnects the channel and lets the thread exit early.
        let (_watchdog, disarmed) = mpsc::channel::<()>();
        let isolate = runtime.v8_isolate().thread_safe_handle();
        let timeout_stop = stopped.clone();
        let timeout = limits.timeout;
        std::thread::spawn(move || {
            if disarmed.recv_timeout(timeout) == Err(mpsc::RecvTimeoutError::Timeout) {
                timeout_stop.store(STOPPED_TIMEOUT, Ordering::SeqCst);
                isolate.terminate_execution();
            }
        });

        {
            let op_state = runtime.op_state();
            let mut op_state = op_state.borrow_mut();
            op_state.put(OutputBudget {
                remaining: limits.max_output_bytes,
            });
            op_state.put(MaxResponseBytes(limits.max_response_bytes));
            op_state.put(fetch_policy);
            if let Some(cb) = batch_callback {
                op_state.put(cb);
            }
        }

        // The tokio timeout covers a plugin that is *waiting* (on a long sleep or a slow
        // fetch), where no JS runs for the watchdog's termination to interrupt.
        let outcome =
            tokio::time::timeout(limits.timeout, Self::evaluate(&mut runtime, &wrapper_url)).await;

        let limit_error = |stopped: u8| match stopped {
            STOPPED_HEAP => Some(anyhow!(
                "Plugin exceeded its memory limit ({} MiB)",
                limits.max_heap_bytes / (1024 * 1024)
            )),
            STOPPED_TIMEOUT => Some(anyhow!(
                "Plugin exceeded its time limit ({}s)",
                limits.timeout.as_secs()
            )),
            _ => None,
        };
        match outcome {
            Ok(Ok(())) => Ok(runtime),
            Ok(Err(e)) => Err(limit_error(stopped.load(Ordering::SeqCst)).unwrap_or(e)),
            Err(_elapsed) => Err(limit_error(STOPPED_TIMEOUT).expect("timeout has a message")),
        }
    }

    async fn evaluate(runtime: &mut JsRuntime, wrapper_url: &ModuleSpecifier) -> Result<()> {
        let mod_id = runtime
            .load_main_es_module(wrapper_url)
            .await
            .map_err(|e| anyhow!("Failed to load plugin module: {e}"))?;
        let receiver = runtime.mod_evaluate(mod_id);
        runtime
            .run_event_loop(Default::default())
            .await
            .map_err(|e| anyhow!("Event loop error: {e}"))?;
        receiver
            .await
            .map_err(|e| anyhow!("Module evaluation error: {e}"))?;
        Ok(())
    }

    fn create_runtime(root: PathBuf, limits: &PluginLimits) -> JsRuntime {
        JsRuntime::new(RuntimeOptions {
            module_loader: Some(Rc::new(TsModuleLoader { root })),
            extensions: vec![crate::nisaba_vendor_ext::init()],
            create_params: Some(v8::CreateParams::default().heap_limits(0, limits.max_heap_bytes)),
            ..Default::default()
        })
    }
}

fn single_file(plugin_code: &str) -> HashMap<String, String> {
    HashMap::from([("index.ts".to_string(), plugin_code.to_string())])
}

/// A plugin file name must be a non-empty relative path made only of ordinary
/// components — no root, drive prefix, `.` or `..` — so joining it onto the plugin
/// directory can never land outside it. Plugin files come from untrusted zips and
/// directories, and they are written to disk before any code runs.
fn validate_plugin_file_name(name: &str) -> Result<()> {
    let path = Path::new(name);
    let plain = !name.is_empty() && path.components().all(|c| matches!(c, Component::Normal(_)));
    if plain {
        Ok(())
    } else {
        Err(anyhow!(
            "Plugin file name {name:?} is not a plain relative path"
        ))
    }
}

fn nonce() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    // The counter keeps two calls within one clock tick from colliding — plugins run
    // concurrently on separate threads, and each needs its own directory.
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "{:x}_{:x}_{:x}",
        nanos,
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}
