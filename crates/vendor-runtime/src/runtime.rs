use std::collections::HashMap;
use std::rc::Rc;

use anyhow::{anyhow, Result};
use deno_core::{
    JsRuntime, ModuleLoadOptions, ModuleLoadReferrer, ModuleLoadResponse, ModuleSource,
    ModuleSourceCode, ModuleSpecifier, ModuleType, RuntimeOptions,
};
use deno_error::JsErrorBox;
use tracing::debug;

use crate::ops::{BatchCallback, PluginResult};
use crate::types::{PluginMetadata, VendorListing};

/// Custom module loader that transpiles TypeScript on the fly.
struct TsModuleLoader;

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
        ModuleLoadResponse::Async(Box::pin(async move {
            let path = specifier.to_file_path().map_err(|_| {
                JsErrorBox::generic(format!(
                    "Cannot convert specifier to file path: {specifier}"
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

/// The vendor plugin runtime — wraps a deno_core JsRuntime.
pub struct VendorRuntime;

impl VendorRuntime {
    /// Read plugin metadata without executing fetchListings.
    pub async fn read_metadata(plugin_code: &str) -> Result<PluginMetadata> {
        let tmp_dir = std::env::temp_dir().join("nisaba_plugins");
        tokio::fs::create_dir_all(&tmp_dir).await?;
        let tmp_path = tmp_dir.join(format!("_meta_{}.ts", nonce()));
        tokio::fs::write(&tmp_path, plugin_code).await?;

        let result = Self::read_metadata_from_path(tmp_path.to_str().unwrap()).await;
        let _ = tokio::fs::remove_file(&tmp_path).await;
        result
    }

    /// Read metadata from a plugin file on disk.
    pub async fn read_metadata_from_path(path: &str) -> Result<PluginMetadata> {
        let plugin_url = ModuleSpecifier::from_file_path(path)
            .map_err(|_| anyhow!("Invalid plugin path: {path}"))?;

        let wrapper_code = format!(
            r#"
            import {{ metadata }} from "{}";
            Deno.core.ops.op_nisaba_set_result(JSON.stringify(metadata));
            "#,
            plugin_url
        );

        let tmp_dir = std::env::temp_dir().join("nisaba_plugins");
        tokio::fs::create_dir_all(&tmp_dir).await?;
        let wrapper_path = tmp_dir.join(format!("_wrapper_{}.js", nonce()));
        tokio::fs::write(&wrapper_path, &wrapper_code).await?;

        let wrapper_url = ModuleSpecifier::from_file_path(&wrapper_path)
            .map_err(|_| anyhow!("Invalid wrapper path"))?;

        let mut runtime = Self::create_runtime();

        let mod_id = runtime
            .load_main_es_module(&wrapper_url)
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

        let _ = tokio::fs::remove_file(&wrapper_path).await;

        let op_state = runtime.op_state();
        let state = op_state.borrow();
        let result = state
            .try_borrow::<PluginResult>()
            .ok_or_else(|| anyhow!("Plugin did not export metadata"))?;

        let metadata: PluginMetadata = serde_json::from_str(&result.0)
            .map_err(|e| anyhow!("Failed to parse plugin metadata: {e}"))?;

        Ok(metadata)
    }

    /// Execute a plugin's fetchListings function with the given config.
    pub async fn execute_plugin(
        plugin_code: &str,
        config: HashMap<String, String>,
        batch_callback: Option<BatchCallback>,
    ) -> Result<Vec<VendorListing>> {
        let tmp_dir = std::env::temp_dir().join("nisaba_plugins");
        tokio::fs::create_dir_all(&tmp_dir).await?;
        let plugin_path = tmp_dir.join(format!("_exec_{}.ts", nonce()));
        tokio::fs::write(&plugin_path, plugin_code).await?;

        let plugin_url = ModuleSpecifier::from_file_path(&plugin_path)
            .map_err(|_| anyhow!("Invalid plugin path"))?;

        let config_json = serde_json::to_string(&config)?;

        let wrapper_code = format!(
            r#"
            import {{ fetchListings }} from "{}";
            const config = {};
            const result = await fetchListings(config);
            Deno.core.ops.op_nisaba_set_result(JSON.stringify(result));
            "#,
            plugin_url, config_json
        );

        let wrapper_path = tmp_dir.join(format!("_run_{}.js", nonce()));
        tokio::fs::write(&wrapper_path, &wrapper_code).await?;

        let wrapper_url = ModuleSpecifier::from_file_path(&wrapper_path)
            .map_err(|_| anyhow!("Invalid wrapper path"))?;

        let mut runtime = Self::create_runtime();

        if let Some(cb) = batch_callback {
            runtime.op_state().borrow_mut().put(cb);
        }

        let mod_id = runtime
            .load_main_es_module(&wrapper_url)
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

        let _ = tokio::fs::remove_file(&plugin_path).await;
        let _ = tokio::fs::remove_file(&wrapper_path).await;

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

    /// Write a multi-file plugin's files to a temp directory.
    /// Skips `data:` URLs (binary icon assets stored inline).
    /// Returns `(temp_dir, index_path)`. Validates `index.ts` exists.
    async fn write_plugin_files(
        files: &HashMap<String, String>,
    ) -> Result<(std::path::PathBuf, std::path::PathBuf)> {
        if !files.contains_key("index.ts") {
            return Err(anyhow!("Plugin must contain an index.ts file"));
        }

        let plugin_dir = std::env::temp_dir()
            .join("nisaba_plugins")
            .join(format!("plugin_{}", nonce()));
        tokio::fs::create_dir_all(&plugin_dir).await?;

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
            tokio::fs::write(&file_path, content).await?;
        }

        let index_path = plugin_dir.join("index.ts");
        Ok((plugin_dir, index_path))
    }

    /// Read metadata from a multi-file plugin (HashMap of filename → content).
    pub async fn read_metadata_from_files(
        files: &HashMap<String, String>,
    ) -> Result<PluginMetadata> {
        let (plugin_dir, index_path) = Self::write_plugin_files(files).await?;
        let result = Self::read_metadata_from_path(index_path.to_str().unwrap()).await;
        let _ = tokio::fs::remove_dir_all(&plugin_dir).await;
        result
    }

    /// Execute a multi-file plugin's fetchListings with the given config.
    pub async fn execute_plugin_from_files(
        files: &HashMap<String, String>,
        config: HashMap<String, String>,
        batch_callback: Option<BatchCallback>,
    ) -> Result<Vec<VendorListing>> {
        let (plugin_dir, index_path) = Self::write_plugin_files(files).await?;

        let plugin_url = ModuleSpecifier::from_file_path(&index_path)
            .map_err(|_| anyhow!("Invalid plugin path"))?;

        let config_json = serde_json::to_string(&config)?;

        let wrapper_code = format!(
            r#"
            import {{ fetchListings }} from "{}";
            const config = {};
            const result = await fetchListings(config);
            Deno.core.ops.op_nisaba_set_result(JSON.stringify(result));
            "#,
            plugin_url, config_json
        );

        let wrapper_path = plugin_dir.join(format!("_run_{}.js", nonce()));
        tokio::fs::write(&wrapper_path, &wrapper_code).await?;

        let wrapper_url = ModuleSpecifier::from_file_path(&wrapper_path)
            .map_err(|_| anyhow!("Invalid wrapper path"))?;

        let mut runtime = Self::create_runtime();

        if let Some(cb) = batch_callback {
            runtime.op_state().borrow_mut().put(cb);
        }

        let mod_id = runtime
            .load_main_es_module(&wrapper_url)
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

        let _ = tokio::fs::remove_dir_all(&plugin_dir).await;

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

    fn create_runtime() -> JsRuntime {
        JsRuntime::new(RuntimeOptions {
            module_loader: Some(Rc::new(TsModuleLoader)),
            extensions: vec![crate::nisaba_vendor_ext::init()],
            ..Default::default()
        })
    }
}

fn nonce() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{:x}", nanos)
}
