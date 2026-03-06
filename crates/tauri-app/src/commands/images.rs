use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use image::imageops::FilterType;
use image::ImageFormat;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager, Runtime, State, UriSchemeContext};

use crate::state::AppState;

/// Max width for thumbnails used in grid/card views.
const THUMB_MAX_WIDTH: u32 = 400;
/// JPEG quality for thumbnails (1-100).
const THUMB_QUALITY: u8 = 80;

/// Resolve the image cache directory, creating it if needed.
fn cache_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data dir: {e}"))?
        .join("image_cache");
    if !dir.exists() {
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Failed to create image cache dir: {e}"))?;
    }
    Ok(dir)
}

/// Path to the manifest file that maps URL → cached filename.
fn manifest_path(dir: &Path) -> PathBuf {
    dir.join("manifest.json")
}

/// Read the manifest from disk. Returns empty map if missing/corrupt.
fn read_manifest(dir: &Path) -> HashMap<String, String> {
    let path = manifest_path(dir);
    match std::fs::read_to_string(&path) {
        Ok(json) => serde_json::from_str(&json).unwrap_or_default(),
        Err(_) => HashMap::new(),
    }
}

/// Merge new entries into the manifest and write to disk.
fn save_manifest(dir: &Path, new_entries: &[(String, String)]) {
    if new_entries.is_empty() {
        return;
    }
    let mut manifest = read_manifest(dir);
    for (url, fname) in new_entries {
        manifest.insert(url.clone(), fname.clone());
    }
    if let Ok(json) = serde_json::to_string(&manifest) {
        let _ = std::fs::write(manifest_path(dir), json);
    }
}

/// Deterministic filename from a URL: sha256 hex + extension.
fn filename_for_url(url: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(url.as_bytes());
    let hash = format!("{:x}", hasher.finalize());

    // Try to extract extension from URL
    let ext = url
        .split('?')
        .next()
        .and_then(|path| path.rsplit('.').next())
        .and_then(|e| {
            let e = e.to_lowercase();
            match e.as_str() {
                "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif" | "svg" => Some(e),
                _ => None,
            }
        })
        .unwrap_or_else(|| "jpg".to_string());

    format!("{hash}.{ext}")
}

/// Derive the thumbnail filename from a full image filename.
/// e.g. "abc123.jpg" → "abc123_thumb.jpg"
fn thumb_filename(full_name: &str) -> String {
    match full_name.rsplit_once('.') {
        Some((base, ext)) => format!("{base}_thumb.{ext}"),
        None => format!("{full_name}_thumb"),
    }
}

/// Derive the full-size filename from a thumbnail filename.
/// e.g. "abc123_thumb.jpg" → "abc123.jpg"
fn full_filename_from_thumb(thumb_name: &str) -> Option<String> {
    // Remove extension, check for _thumb suffix, reconstruct
    match thumb_name.rsplit_once('.') {
        Some((base, ext)) => {
            let stripped = base.strip_suffix("_thumb")?;
            Some(format!("{stripped}.{ext}"))
        }
        None => {
            let stripped = thumb_name.strip_suffix("_thumb")?;
            Some(stripped.to_string())
        }
    }
}

/// Generate a thumbnail from raw image bytes, saving as JPEG.
/// Returns true if the thumbnail was successfully generated.
fn generate_thumbnail(source_bytes: &[u8], thumb_path: &Path) -> bool {
    let img = match image::load_from_memory(source_bytes) {
        Ok(img) => img,
        Err(e) => {
            tracing::debug!(error = %e, "Could not decode image for thumbnail");
            return false;
        }
    };

    let (w, h) = (img.width(), img.height());
    // Only resize if wider than threshold
    if w <= THUMB_MAX_WIDTH {
        // Image is already small, just save a JPEG copy as the thumb
        let mut buf = Cursor::new(Vec::new());
        if img
            .write_to(&mut buf, ImageFormat::Jpeg)
            .is_ok()
        {
            return std::fs::write(thumb_path, buf.into_inner()).is_ok();
        }
        return false;
    }

    let new_h = (h as f64 * THUMB_MAX_WIDTH as f64 / w as f64).round() as u32;
    let resized = img.resize_exact(THUMB_MAX_WIDTH, new_h, FilterType::Triangle);

    let mut buf = Cursor::new(Vec::new());
    let encoder =
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, THUMB_QUALITY);
    if resized.write_with_encoder(encoder).is_ok() {
        return std::fs::write(thumb_path, buf.into_inner()).is_ok();
    }
    false
}

/// Get the MIME type from a file extension.
fn mime_for_ext(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "svg" => "image/svg+xml",
        _ => "image/jpeg",
    }
}

/// Serve a file from the cache dir, returning a proper HTTP response.
fn serve_file(file_path: &Path) -> tauri::http::Response<Vec<u8>> {
    match std::fs::read(file_path) {
        Ok(bytes) => {
            let ext = file_path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("jpg");
            tauri::http::Response::builder()
                .status(200)
                .header("content-type", mime_for_ext(ext))
                .header("cache-control", "public, max-age=31536000, immutable")
                .body(bytes)
                .unwrap_or_else(|_| empty_response(500))
        }
        Err(_) => empty_response(404),
    }
}

/// Load the entire cache manifest: returns all (url, filename) pairs.
/// Called once on app startup to pre-populate the frontend cache map instantly.
#[tauri::command]
pub async fn load_image_cache_manifest(
    app: AppHandle,
    _state: State<'_, AppState>,
) -> Result<Vec<(String, String)>, String> {
    let dir = cache_dir(&app)?;
    let manifest = read_manifest(&dir);
    Ok(manifest.into_iter().collect())
}

/// Cache a batch of image URLs to local disk.
/// Returns a list of (original_url, cached_filename) for successfully cached images.
/// Downloads run concurrently (up to 8 at a time) so the total wait is roughly
/// max(download_time) instead of sum(download_times).
/// Thumbnails are NOT generated here — they are generated async in the background.
/// The protocol handler falls back to full-size images when thumbs don't exist yet.
#[tauri::command]
pub async fn cache_images(
    app: AppHandle,
    _state: State<'_, AppState>,
    urls: Vec<String>,
) -> Result<Vec<(String, String)>, String> {
    let dir = cache_dir(&app)?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    let mut results = Vec::new();
    let mut to_download: Vec<(String, String, PathBuf)> = Vec::new();

    for url in &urls {
        let fname = filename_for_url(url);
        let path = dir.join(&fname);

        // Already cached — skip download
        if path.exists() {
            results.push((url.clone(), fname));
            continue;
        }

        to_download.push((url.clone(), fname, path));
    }

    // Download missing images concurrently (max 8 at a time)
    if !to_download.is_empty() {
        let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(8));
        let mut tasks = tokio::task::JoinSet::new();

        for (url, fname, path) in to_download {
            let client = client.clone();
            let sem = semaphore.clone();
            tasks.spawn(async move {
                let _permit = sem.acquire().await;
                match client.get(&url).send().await {
                    Ok(resp) if resp.status().is_success() => {
                        match resp.bytes().await {
                            Ok(bytes) if !bytes.is_empty() => {
                                if let Err(e) = std::fs::write(&path, &bytes) {
                                    tracing::warn!(url, error = %e, "Failed to write cached image");
                                    None
                                } else {
                                    Some((url, fname))
                                }
                            }
                            _ => {
                                tracing::warn!(url, "Empty response body for image");
                                None
                            }
                        }
                    }
                    Ok(resp) => {
                        tracing::warn!(url, status = %resp.status(), "Non-200 response for image");
                        None
                    }
                    Err(e) => {
                        tracing::warn!(url, error = %e, "Failed to download image");
                        None
                    }
                }
            });
        }

        while let Some(result) = tasks.join_next().await {
            if let Ok(Some((url, fname))) = result {
                results.push((url, fname));
            }
        }
    }

    // Persist new entries to manifest for instant startup loading
    save_manifest(&dir, &results);

    Ok(results)
}

/// Generate thumbnails for all cached images that don't have one yet.
/// Called on startup in the background to backfill existing cache entries.
/// Runs on a blocking thread so CPU-bound image processing doesn't starve
/// the async runtime (which handles IPC commands, downloads, etc.).
#[tauri::command]
pub async fn generate_missing_thumbnails(
    app: AppHandle,
    _state: State<'_, AppState>,
) -> Result<u32, String> {
    let dir = cache_dir(&app)?;
    let manifest = read_manifest(&dir);

    // Collect work items, then process on a blocking thread
    let work: Vec<(PathBuf, PathBuf)> = manifest
        .values()
        .filter_map(|fname| {
            let tname = thumb_filename(fname);
            let tpath = dir.join(&tname);
            if tpath.exists() {
                return None;
            }
            let full_path = dir.join(fname);
            if !full_path.exists() {
                return None;
            }
            Some((full_path, tpath))
        })
        .collect();

    if work.is_empty() {
        return Ok(0);
    }

    let count = tokio::task::spawn_blocking(move || {
        let mut count = 0u32;
        for (full_path, tpath) in &work {
            if let Ok(bytes) = std::fs::read(full_path) {
                if generate_thumbnail(&bytes, tpath) {
                    count += 1;
                }
            }
        }
        count
    })
    .await
    .map_err(|e| format!("Thumbnail generation task failed: {e}"))?;

    if count > 0 {
        tracing::info!(count, "Generated missing thumbnails");
    }
    Ok(count)
}

/// Check which URLs are already cached. Returns (url, filename) for cached ones only.
#[tauri::command]
pub async fn get_cached_images(
    app: AppHandle,
    _state: State<'_, AppState>,
    urls: Vec<String>,
) -> Result<Vec<(String, String)>, String> {
    let dir = cache_dir(&app)?;
    let mut results = Vec::new();
    for url in &urls {
        let fname = filename_for_url(url);
        if dir.join(&fname).exists() {
            results.push((url.clone(), fname));
        }
    }
    Ok(results)
}

/// Validate that a filename is safe (no path traversal).
/// Our filenames are SHA256 hex + optional `_thumb` + extension — no slashes or dots-dots.
fn is_safe_filename(name: &str) -> bool {
    !name.contains("..") && !name.contains('/') && !name.contains('\\')
}

/// Handle `cachedimg://localhost/{filename}` protocol requests.
/// Registered via `.register_uri_scheme_protocol("cachedimg", ...)` in main.rs.
///
/// When a `_thumb` file is requested but doesn't exist yet, this generates
/// the thumbnail on-demand from the full-size image (fast: ~20-60KB JPEG
/// vs 1-1.7MB originals), saves it for next time, and serves it immediately.
pub fn handle_protocol_request<R: Runtime>(
    ctx: UriSchemeContext<'_, R>,
    request: tauri::http::Request<Vec<u8>>,
) -> tauri::http::Response<Vec<u8>> {
    let path = percent_decode(request.uri().path().trim_start_matches('/'));

    // Security: reject any path traversal attempts
    if !is_safe_filename(&path) {
        return empty_response(400);
    }

    let dir = match ctx.app_handle().path().app_data_dir() {
        Ok(d) => d.join("image_cache"),
        Err(_) => return empty_response(404),
    };

    let file_path = dir.join(&path);

    // Fast path: file already exists on disk (covers both full images and pre-generated thumbs)
    if file_path.exists() {
        return serve_file(&file_path);
    }

    // If a _thumb was requested but doesn't exist yet, serve the full-size
    // image immediately and spawn a background thread to generate the thumbnail.
    // Next request will hit the fast path above and serve the small thumbnail.
    if let Some(full_name) = full_filename_from_thumb(&path) {
        let full_path = dir.join(&full_name);
        if full_path.exists() {
            // Spawn background thread to generate thumbnail for next request.
            // This is fire-and-forget — if two requests race, both threads may
            // generate the same thumbnail (harmless; same content, last writer wins).
            let thumb_dest = file_path;
            let source = full_path.clone();
            std::thread::spawn(move || {
                // Re-check in case another thread already generated it
                if thumb_dest.exists() {
                    return;
                }
                if let Ok(bytes) = std::fs::read(&source) {
                    let _ = generate_thumbnail(&bytes, &thumb_dest);
                }
            });
            // Serve the full-size image right now (non-blocking)
            return serve_file(&full_path);
        }
    }

    empty_response(404)
}

fn empty_response(status: u16) -> tauri::http::Response<Vec<u8>> {
    tauri::http::Response::builder()
        .status(status)
        .body(Vec::new())
        .unwrap()
}

fn percent_decode(s: &str) -> String {
    urlencoding::decode(s)
        .unwrap_or(std::borrow::Cow::Borrowed(s))
        .into_owned()
}
