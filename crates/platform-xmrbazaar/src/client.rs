use std::sync::Arc;

use nisaba_core::error::SyncError;
use nisaba_core::types::{CreateListingRequest, Platform};
use reqwest::{cookie::Jar, Client};
use tracing::debug;

use crate::endpoints::Endpoints;
use crate::scraper as html;
use crate::types::{StockModeField, XmrListing, XmrListingDetail, XmrSaleRecord};

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; rv:128.0) Gecko/20100101 Firefox/128.0";

/// Truncate `body` to at most `max_bytes` for inclusion in an error message,
/// backing off to the nearest UTF-8 char boundary. Slicing at a raw byte
/// offset (`&body[..body.len().min(N)]`) panics when byte N splits a
/// multi-byte character in the remote HTML — turning an API error into a
/// crash — so every error snippet goes through here.
pub fn body_snippet(body: &str, max_bytes: usize) -> &str {
    if body.len() <= max_bytes {
        return body;
    }
    let mut end = max_bytes;
    while end > 0 && !body.is_char_boundary(end) {
        end -= 1;
    }
    &body[..end]
}

pub struct XmrBazaarClient {
    /// Main HTTP client with redirect following.
    http: Client,
    /// Shared cookie jar so a no-redirect client can share the session.
    jar: Arc<Jar>,
}

impl Default for XmrBazaarClient {
    fn default() -> Self {
        Self::new()
    }
}

impl XmrBazaarClient {
    pub fn new() -> Self {
        let jar = Arc::new(Jar::default());
        let http = Client::builder()
            .cookie_provider(jar.clone())
            .user_agent(USER_AGENT)
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .expect("Failed to build HTTP client");

        Self { http, jar }
    }

    pub fn http(&self) -> &Client {
        &self.http
    }

    /// Build a no-redirect client that shares the same cookie jar.
    /// Used for login where we need to inspect the 302 response directly.
    pub fn no_redirect_http(&self) -> Client {
        Client::builder()
            .cookie_provider(self.jar.clone())
            .user_agent(USER_AGENT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("Failed to build no-redirect HTTP client")
    }

    /// Fetch all listings from the "my listings" page.
    pub async fn fetch_listings(
        &self,
        endpoints: &Endpoints,
    ) -> Result<Vec<XmrListing>, SyncError> {
        let (_, url) = endpoints.my_listings();

        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::XmrBazaar,
                message: e.to_string(),
            })?;

        let status = resp.status();
        let final_url = resp.url().to_string();

        // Check if redirected to login
        if final_url.contains("login") {
            return Err(SyncError::AuthError {
                platform: Platform::XmrBazaar,
                message: "Session expired, redirected to login".into(),
            });
        }

        if !status.is_success() {
            return Err(SyncError::ApiError {
                platform: Platform::XmrBazaar,
                message: format!("Failed to fetch listings ({status})"),
            });
        }

        let body = resp.text().await.map_err(|e| SyncError::NetworkError {
            platform: Platform::XmrBazaar,
            message: e.to_string(),
        })?;

        let listings = html::parse_listings(&body, endpoints.base_url());
        debug!(count = listings.len(), "Fetched XMR Bazaar listings");

        Ok(listings)
    }

    /// Update a listing's quantity by GETting the edit form, modifying the
    /// quantity field, and POSTing back with the CSRF token.
    pub async fn update_quantity(
        &self,
        endpoints: &Endpoints,
        listing_id: &str,
        quantity: i64,
    ) -> Result<(), SyncError> {
        let (_, edit_url) = endpoints.edit_listing(listing_id);

        // GET the edit form
        let resp = self
            .http
            .get(&edit_url)
            .send()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::XmrBazaar,
                message: e.to_string(),
            })?;

        if !resp.status().is_success() {
            return Err(SyncError::ApiError {
                platform: Platform::XmrBazaar,
                message: format!("Failed to load edit form for listing {listing_id}"),
            });
        }

        let final_url = resp.url().to_string();
        if is_login_url(&final_url) {
            return Err(SyncError::AuthError {
                platform: Platform::XmrBazaar,
                message: "Session expired, redirected to login".into(),
            });
        }

        let page = resp.text().await.map_err(|e| SyncError::NetworkError {
            platform: Platform::XmrBazaar,
            message: e.to_string(),
        })?;

        let form_data = html::parse_edit_form(&page).ok_or_else(|| {
            SyncError::ParseError(format!(
                "Could not parse edit form for listing {listing_id}"
            ))
        })?;

        // Build the form submission with updated quantity
        let qty_str = quantity.to_string();
        let mut form_params: Vec<(String, String)> = Vec::new();
        form_params.push((
            form_data.csrf_field_name.clone(),
            form_data.csrf_token.clone(),
        ));

        let mut quantity_field: Option<String> = None;
        for (name, value) in &form_data.fields {
            if name == "quantity" || name == "stock" || name == "qty" {
                form_params.push((name.clone(), qty_str.clone()));
                if quantity_field.is_none() {
                    quantity_field = Some(name.clone());
                }
            } else {
                form_params.push((name.clone(), value.clone()));
            }
        }

        let quantity_field = quantity_field.unwrap_or_else(|| {
            form_params.push(("quantity".to_string(), qty_str.clone()));
            "quantity".to_string()
        });

        // POST the form back and positively verify the saved value.
        self.submit_edit_and_verify(
            endpoints,
            listing_id,
            &edit_url,
            form_params,
            vec![(quantity_field, qty_str)],
            "quantity update",
        )
        .await?;
        debug!(listing_id, quantity, "XMR Bazaar quantity updated");
        Ok(())
    }

    /// POST an edit form with a no-redirect client and positively confirm
    /// the save: a redirect must not bounce to `/login/`, a 200 must not be
    /// a re-rendered form carrying errors (or the login page), and the
    /// expected field values must read back from a fresh GET of the edit
    /// form. Anything else is an `Err` — the redirect-following client's
    /// final 2xx alone proves nothing, since a login bounce and an error
    /// form both end there.
    async fn submit_edit_and_verify(
        &self,
        endpoints: &Endpoints,
        listing_id: &str,
        edit_url: &str,
        form_params: Vec<(String, String)>,
        expected: Vec<(String, String)>,
        context: &str,
    ) -> Result<(), SyncError> {
        let resp = self
            .no_redirect_http()
            .post(edit_url)
            .header("Referer", edit_url)
            .form(&form_params)
            .send()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::XmrBazaar,
                message: e.to_string(),
            })?;

        let status = resp.status();
        if status.is_redirection() {
            let location = resp
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            if is_login_url(&location) {
                return Err(SyncError::AuthError {
                    platform: Platform::XmrBazaar,
                    message: format!(
                        "Session expired: {context} for listing {listing_id} bounced to login"
                    ),
                });
            }
            if location.is_empty() {
                return Err(SyncError::ApiError {
                    platform: Platform::XmrBazaar,
                    message: format!(
                        "{context} for listing {listing_id} returned {status} without a Location header; save not confirmed"
                    ),
                });
            }
            // A plausible redirect target is not proof on its own: fall
            // through to the read-back verification below.
        } else if status.is_success() {
            let body = resp.text().await.map_err(|e| SyncError::NetworkError {
                platform: Platform::XmrBazaar,
                message: e.to_string(),
            })?;
            if looks_like_login_page(&body) {
                return Err(SyncError::AuthError {
                    platform: Platform::XmrBazaar,
                    message: format!(
                        "Session expired: {context} for listing {listing_id} returned the login page"
                    ),
                });
            }
            let errors = html::extract_form_errors(&body);
            if !errors.is_empty() {
                return Err(SyncError::ApiError {
                    platform: Platform::XmrBazaar,
                    message: format!(
                        "{context} for listing {listing_id} rejected: {}",
                        errors.join("; ")
                    ),
                });
            }
        } else {
            let body = resp.text().await.unwrap_or_default();
            return Err(SyncError::ApiError {
                platform: Platform::XmrBazaar,
                message: format!(
                    "{context} for listing {listing_id} failed ({status}): {}",
                    body_snippet(&body, 200)
                ),
            });
        }

        // Read-back: re-GET the edit form and compare the saved values.
        let (_, verify_url) = endpoints.edit_listing(listing_id);
        let resp =
            self.http
                .get(&verify_url)
                .send()
                .await
                .map_err(|e| SyncError::NetworkError {
                    platform: Platform::XmrBazaar,
                    message: e.to_string(),
                })?;
        let final_url = resp.url().to_string();
        if is_login_url(&final_url) {
            return Err(SyncError::AuthError {
                platform: Platform::XmrBazaar,
                message: "Session expired, redirected to login while verifying save".into(),
            });
        }
        if !resp.status().is_success() {
            return Err(SyncError::ApiError {
                platform: Platform::XmrBazaar,
                message: format!(
                    "Could not verify {context} for listing {listing_id}: edit form reload failed ({})",
                    resp.status()
                ),
            });
        }
        let page = resp.text().await.map_err(|e| SyncError::NetworkError {
            platform: Platform::XmrBazaar,
            message: e.to_string(),
        })?;
        let saved = html::parse_edit_form(&page).ok_or_else(|| SyncError::ApiError {
            platform: Platform::XmrBazaar,
            message: format!(
                "Could not verify {context} for listing {listing_id}: edit form did not parse after save"
            ),
        })?;
        for (name, want) in &expected {
            let got = saved
                .fields
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.as_str());
            if got != Some(want.as_str()) {
                return Err(SyncError::ApiError {
                    platform: Platform::XmrBazaar,
                    message: format!(
                        "{context} for listing {listing_id} not confirmed: field '{name}' reads back as {got:?}, expected '{want}'"
                    ),
                });
            }
        }
        Ok(())
    }

    /// Fetch detailed listing information (description, photos, price) from
    /// the listing's edit page.
    pub async fn fetch_listing_detail(
        &self,
        endpoints: &Endpoints,
        listing_id: &str,
    ) -> Result<XmrListingDetail, SyncError> {
        let (_, edit_url) = endpoints.edit_listing(listing_id);

        let resp = self
            .http
            .get(&edit_url)
            .send()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::XmrBazaar,
                message: e.to_string(),
            })?;

        let final_url = resp.url().to_string();
        if final_url.contains("login") {
            return Err(SyncError::AuthError {
                platform: Platform::XmrBazaar,
                message: "Session expired, redirected to login".into(),
            });
        }

        if !resp.status().is_success() {
            return Err(SyncError::ApiError {
                platform: Platform::XmrBazaar,
                message: format!("Failed to load edit page for listing {listing_id}"),
            });
        }

        let page = resp.text().await.map_err(|e| SyncError::NetworkError {
            platform: Platform::XmrBazaar,
            message: e.to_string(),
        })?;

        let detail = html::parse_listing_detail(&page, endpoints.base_url()).ok_or_else(|| {
            SyncError::ParseError(format!("Could not parse listing detail for {listing_id}"))
        })?;

        debug!(
            listing_id,
            has_description = detail.description.is_some(),
            photo_count = detail.photos.len(),
            has_price = detail.price.is_some(),
            "Fetched XMR Bazaar listing detail"
        );

        Ok(detail)
    }

    /// Fetch recent sales from the /my-sales/ page.
    pub async fn fetch_recent_sales(
        &self,
        endpoints: &Endpoints,
    ) -> Result<Vec<XmrSaleRecord>, SyncError> {
        let (_, url) = endpoints.my_sales();

        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::XmrBazaar,
                message: e.to_string(),
            })?;

        let final_url = resp.url().to_string();
        if final_url.contains("login") {
            return Err(SyncError::AuthError {
                platform: Platform::XmrBazaar,
                message: "Session expired, redirected to login".into(),
            });
        }

        let status = resp.status();
        if !status.is_success() {
            return Err(SyncError::ApiError {
                platform: Platform::XmrBazaar,
                message: format!("Failed to fetch sales page ({status})"),
            });
        }

        let body = resp.text().await.map_err(|e| SyncError::NetworkError {
            platform: Platform::XmrBazaar,
            message: e.to_string(),
        })?;

        let sales = html::parse_sales_page(&body, endpoints.base_url());
        debug!(count = sales.len(), "Fetched XMR Bazaar sales");

        Ok(sales)
    }

    /// Set the stock mode for a listing: active=true → unlimited, active=false → one-time/deactivated.
    /// GETs the edit form, discovers the stock mode field (auto-discovery or from passed-in info),
    /// sets the appropriate value, and POSTs back.
    pub async fn set_stock_mode(
        &self,
        endpoints: &Endpoints,
        listing_id: &str,
        active: bool,
        stock_field: Option<&StockModeField>,
    ) -> Result<(), SyncError> {
        let (_, edit_url) = endpoints.edit_listing(listing_id);

        // GET the edit form
        let resp = self
            .http
            .get(&edit_url)
            .send()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::XmrBazaar,
                message: e.to_string(),
            })?;

        let final_url = resp.url().to_string();
        if final_url.contains("login") {
            return Err(SyncError::AuthError {
                platform: Platform::XmrBazaar,
                message: "Session expired, redirected to login".into(),
            });
        }

        if !resp.status().is_success() {
            return Err(SyncError::ApiError {
                platform: Platform::XmrBazaar,
                message: format!("Failed to load edit form for listing {listing_id}"),
            });
        }

        let page = resp.text().await.map_err(|e| SyncError::NetworkError {
            platform: Platform::XmrBazaar,
            message: e.to_string(),
        })?;

        let form_data = html::parse_edit_form(&page).ok_or_else(|| {
            SyncError::ParseError(format!(
                "Could not parse edit form for listing {listing_id}"
            ))
        })?;

        // Determine which stock mode field to use
        let discovered = html::detect_stock_mode_field(&form_data);
        let field = stock_field.or(discovered.as_ref());

        let Some(field) = field else {
            // No stock field means there is nothing to toggle: reporting
            // success would tell the engine the listing changed state when
            // it did not.
            return Err(SyncError::ApiError {
                platform: Platform::XmrBazaar,
                message: format!(
                    "No stock mode field found in edit form for listing {listing_id}; stock mode was not changed"
                ),
            });
        };

        let target_value = if active {
            field.unlimited_value.clone()
        } else {
            field.onetime_value.clone()
        };

        // Build form submission with updated stock mode field
        let mut form_params: Vec<(String, String)> = Vec::new();
        form_params.push((
            form_data.csrf_field_name.clone(),
            form_data.csrf_token.clone(),
        ));

        let mut expected: Vec<(String, String)> =
            vec![(field.field_name.clone(), target_value.clone())];
        let mut field_set = false;
        for (name, value) in &form_data.fields {
            if name == &field.field_name {
                form_params.push((name.clone(), target_value.clone()));
                field_set = true;
            } else if name.eq_ignore_ascii_case("status") {
                let status_val = if active { "active" } else { "out_of_stock" };
                form_params.push((name.clone(), status_val.to_string()));
                expected.push((name.clone(), status_val.to_string()));
            } else {
                form_params.push((name.clone(), value.clone()));
            }
        }

        if !field_set {
            form_params.push((field.field_name.clone(), target_value.clone()));
        }

        self.submit_edit_and_verify(
            endpoints,
            listing_id,
            &edit_url,
            form_params,
            expected,
            "stock mode update",
        )
        .await?;
        debug!(
            listing_id,
            active,
            field_name = %field.field_name,
            target_value = %target_value,
            "XMR Bazaar stock mode updated"
        );
        Ok(())
    }

    /// Create a new listing by GETting the new-listing form page to obtain all
    /// form fields + CSRF token, overriding the listing-specific fields, and
    /// POSTing as multipart/form-data (supports photo uploads).
    /// Returns the new listing ID extracted from the response.
    pub async fn create_listing(
        &self,
        endpoints: &Endpoints,
        request: &CreateListingRequest,
        monero_address: &str,
    ) -> Result<String, SyncError> {
        let (_, url) = endpoints.new_listing();

        // Snapshot the existing listings before creating anything, so the
        // created listing can later be identified as the diff — never as
        // "the first listing on the page" or an arbitrary body link.
        let before_ids: Option<std::collections::HashSet<String>> = self
            .fetch_listings(endpoints)
            .await
            .ok()
            .map(|listings| listings.iter().map(|l| l.id.clone()).collect());

        // GET the new-listing form to extract CSRF token + all fields
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::XmrBazaar,
                message: e.to_string(),
            })?;

        let final_url = resp.url().to_string();
        if final_url.contains("login") {
            return Err(SyncError::AuthError {
                platform: Platform::XmrBazaar,
                message: "Session expired, redirected to login".into(),
            });
        }

        if !resp.status().is_success() {
            return Err(SyncError::ApiError {
                platform: Platform::XmrBazaar,
                message: format!("Failed to load new listing form ({})", resp.status()),
            });
        }

        let page = resp.text().await.map_err(|e| SyncError::NetworkError {
            platform: Platform::XmrBazaar,
            message: e.to_string(),
        })?;

        let form_data = html::parse_edit_form(&page)
            .ok_or_else(|| SyncError::ParseError("Could not parse new listing form".to_string()))?;

        // Prepare our override values
        let currency = request
            .price
            .as_ref()
            .map(|p| p.currency.to_uppercase())
            .unwrap_or_else(|| "USD".to_string());

        let price_str = request
            .price
            .as_ref()
            .map(|p| p.amount.to_string())
            .unwrap_or_default();

        // Strip HTML tags — XMR Bazaar descriptions are plain text
        let description = request
            .description_html
            .as_deref()
            .map(html::strip_html_tags)
            .unwrap_or_default();

        // Fields we want to override from the form defaults
        let mut overrides: std::collections::HashMap<String, String> = [
            ("action".to_string(), "publish_listing".to_string()),
            ("title".to_string(), request.title.clone()),
            ("description".to_string(), description),
            ("price".to_string(), price_str),
            ("currency".to_string(), currency),
            ("stock".to_string(), "unlimited".to_string()),
            ("location_type".to_string(), "online".to_string()),
            ("domestic_shipping".to_string(), "enabled".to_string()),
            ("payment_method_direct".to_string(), "enabled".to_string()),
            ("terms".to_string(), "confirm".to_string()),
            ("save_as_draft".to_string(), String::new()),
            ("monero_address".to_string(), monero_address.to_string()),
            ("privacy".to_string(), "public".to_string()),
        ]
        .into_iter()
        .collect();

        // Merge extras from the request — extras take precedence over defaults
        for (key, value) in &request.extras {
            overrides.insert(key.clone(), value.clone());
        }

        // File input fields and their selectors — skip from parsed form fields
        // (we'll add photos as proper multipart file parts)
        let skip_fields: std::collections::HashSet<&str> = [
            "photo",
            "additional_photo_1",
            "additional_photo_2",
            "additional_photo_3",
            "additional_photo_4",
            "additional_photo_5",
            "additional_photo_selector_1",
            "additional_photo_selector_2",
            "additional_photo_selector_3",
            "additional_photo_selector_4",
            "additional_photo_selector_5",
        ]
        .into_iter()
        .collect();

        // Download photos from URLs (up to 6: 1 main + 5 additional)
        let max_photos = 6;
        let mut downloaded_photos: Vec<(Vec<u8>, String)> = Vec::new(); // (bytes, filename)
        for (i, photo_url) in request.photo_urls.iter().take(max_photos).enumerate() {
            match self.http.get(photo_url).send().await {
                Ok(resp) if resp.status().is_success() => match resp.bytes().await {
                    Ok(bytes) => {
                        let filename = photo_url
                            .rsplit('/')
                            .next()
                            .unwrap_or("photo.jpg")
                            .to_string();
                        debug!(index = i, url = %photo_url, bytes = bytes.len(), "Downloaded photo");
                        downloaded_photos.push((bytes.to_vec(), filename));
                    }
                    Err(e) => {
                        debug!(url = %photo_url, error = %e, "Failed to read photo bytes, skipping");
                    }
                },
                Ok(resp) => {
                    debug!(url = %photo_url, status = %resp.status(), "Failed to download photo, skipping");
                }
                Err(e) => {
                    debug!(url = %photo_url, error = %e, "Failed to download photo, skipping");
                }
            }
        }

        // Build multipart form, deduplicating by field name (radio buttons
        // produce multiple entries with the same name — only the first/overridden
        // value should be sent).
        let mut form = reqwest::multipart::Form::new();
        let mut seen_fields: std::collections::HashSet<String> = std::collections::HashSet::new();

        // CSRF token first
        form = form.text(
            form_data.csrf_field_name.clone(),
            form_data.csrf_token.clone(),
        );
        seen_fields.insert(form_data.csrf_field_name.clone());

        for (name, value) in &form_data.fields {
            // Skip file inputs — they'll be added as multipart parts
            if skip_fields.contains(name.as_str()) {
                continue;
            }
            // Deduplicate: only include first occurrence of each field name
            if seen_fields.contains(name) {
                continue;
            }
            let use_value = if let Some(ov) = overrides.get(name) {
                ov.clone()
            } else {
                value.clone()
            };
            form = form.text(name.clone(), use_value);
            seen_fields.insert(name.clone());
        }

        // Add any override fields that weren't already in the form
        for (name, value) in &overrides {
            if !seen_fields.contains(name) {
                form = form.text(name.clone(), value.clone());
            }
        }

        // Add photos as multipart file parts
        for (i, (bytes, filename)) in downloaded_photos.into_iter().enumerate() {
            let mime = if filename.to_lowercase().ends_with(".png") {
                "image/png"
            } else if filename.to_lowercase().ends_with(".webp") {
                "image/webp"
            } else {
                "image/jpeg"
            };

            let part = reqwest::multipart::Part::bytes(bytes)
                .file_name(filename)
                .mime_str(mime)
                .map_err(|e| SyncError::ApiError {
                    platform: Platform::XmrBazaar,
                    message: format!("Invalid MIME type: {e}"),
                })?;

            if i == 0 {
                form = form.part("photo", part);
            } else {
                // additional_photo_selector_N=enabled tells the server an extra photo follows
                form = form.text(
                    format!("additional_photo_selector_{i}"),
                    "enabled".to_string(),
                );
                form = form.part(format!("additional_photo_{i}"), part);
            }
        }

        debug!(
            photo_count = request.photo_urls.len(),
            "Submitting new listing form (multipart)"
        );

        // POST with no-redirect client so we can inspect the 302 response
        let no_redirect = self.no_redirect_http();
        let resp = no_redirect
            .post(&url)
            .header("Referer", &url)
            .multipart(form)
            .send()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::XmrBazaar,
                message: e.to_string(),
            })?;

        let status = resp.status();

        // On redirect, the Location header is the one response shape that
        // deterministically names the created listing.
        if status.is_redirection() {
            let location = resp
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            if is_login_url(&location) {
                return Err(SyncError::AuthError {
                    platform: Platform::XmrBazaar,
                    message: "Session expired: listing creation bounced to login".into(),
                });
            }
            if let Some(new_id) = extract_listing_id_from_path(&location) {
                debug!(listing_id = %new_id, "Created XMR Bazaar listing (from redirect)");
                return Ok(new_id);
            }
            // Redirect without a listing id: identify the created listing
            // as the deterministic pre/post diff of my-listings, or error.
            debug!(status = %status, "Redirect after create without listing id — diffing my-listings");
            return self
                .resolve_created_listing(endpoints, before_ids.as_ref(), &request.title)
                .await;
        }

        let body = resp.text().await.map_err(|e| SyncError::NetworkError {
            platform: Platform::XmrBazaar,
            message: e.to_string(),
        })?;

        if status.is_success() {
            if looks_like_login_page(&body) {
                return Err(SyncError::AuthError {
                    platform: Platform::XmrBazaar,
                    message: "Session expired: listing creation returned the login page".into(),
                });
            }
            // A 200 is usually the form re-rendered with validation errors.
            let validation_errors = html::extract_form_errors(&body);
            if !validation_errors.is_empty() {
                return Err(SyncError::ApiError {
                    platform: Platform::XmrBazaar,
                    message: format!(
                        "Listing creation failed ({status}). Validation errors: {}",
                        validation_errors.join("; ")
                    ),
                });
            }
            // No redirect and no error: never accept an arbitrary
            // `/listing/<id>/` link from the body (the page links to many
            // unrelated listings). Identify via the my-listings diff.
            return self
                .resolve_created_listing(endpoints, before_ids.as_ref(), &request.title)
                .await;
        }

        // Try to find validation errors in the HTML for a useful error message
        let validation_errors = html::extract_form_errors(&body);
        let error_detail = if !validation_errors.is_empty() {
            format!("Validation errors: {}", validation_errors.join("; "))
        } else {
            format!("Response: {}", body_snippet(&body, 500))
        };

        Err(SyncError::ApiError {
            platform: Platform::XmrBazaar,
            message: format!("Listing creation failed ({status}). {error_detail}"),
        })
    }

    /// Update a specific field on a listing by GETting the edit form,
    /// modifying the named field, and POSTing back with the CSRF token.
    /// This is a generic version of `update_quantity`.
    pub async fn update_listing_field(
        &self,
        endpoints: &Endpoints,
        listing_id: &str,
        field_name: &str,
        field_value: &str,
    ) -> Result<(), SyncError> {
        self.update_listing_fields(endpoints, listing_id, &[(field_name, field_value)])
            .await
    }

    /// Update several fields of a listing in a single GET/POST cycle, so a
    /// multi-part value (e.g. a price *and* its currency) can never be
    /// half-written by two separate form submissions.
    pub async fn update_listing_fields(
        &self,
        endpoints: &Endpoints,
        listing_id: &str,
        updates: &[(&str, &str)],
    ) -> Result<(), SyncError> {
        let (_, edit_url) = endpoints.edit_listing(listing_id);

        // GET the edit form
        let resp = self
            .http
            .get(&edit_url)
            .send()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::XmrBazaar,
                message: e.to_string(),
            })?;

        let final_url = resp.url().to_string();
        if final_url.contains("login") {
            return Err(SyncError::AuthError {
                platform: Platform::XmrBazaar,
                message: "Session expired, redirected to login".into(),
            });
        }

        if !resp.status().is_success() {
            return Err(SyncError::ApiError {
                platform: Platform::XmrBazaar,
                message: format!("Failed to load edit form for listing {listing_id}"),
            });
        }

        let page = resp.text().await.map_err(|e| SyncError::NetworkError {
            platform: Platform::XmrBazaar,
            message: e.to_string(),
        })?;

        let form_data = html::parse_edit_form(&page).ok_or_else(|| {
            SyncError::ParseError(format!(
                "Could not parse edit form for listing {listing_id}"
            ))
        })?;

        // Build the form submission with the updated field
        let mut form_params: Vec<(String, String)> = Vec::new();
        form_params.push((
            form_data.csrf_field_name.clone(),
            form_data.csrf_token.clone(),
        ));

        let mut fields_set: Vec<&str> = Vec::new();
        for (name, value) in &form_data.fields {
            if let Some((_, update_value)) = updates.iter().find(|(n, _)| n == name) {
                form_params.push((name.clone(), update_value.to_string()));
                fields_set.push(name.as_str());
            } else {
                form_params.push((name.clone(), value.clone()));
            }
        }

        for (field_name, field_value) in updates {
            if !fields_set.contains(field_name) {
                // Field was not present in the form; add it explicitly
                form_params.push((field_name.to_string(), field_value.to_string()));
            }
        }

        let expected: Vec<(String, String)> = updates
            .iter()
            .map(|(n, v)| (n.to_string(), v.to_string()))
            .collect();
        let field_names: Vec<&str> = updates.iter().map(|(n, _)| *n).collect();
        self.submit_edit_and_verify(
            endpoints,
            listing_id,
            &edit_url,
            form_params,
            expected,
            &format!("field(s) '{}' update", field_names.join(", ")),
        )
        .await?;
        debug!(
            listing_id,
            fields = ?field_names,
            "XMR Bazaar fields updated"
        );
        Ok(())
    }

    /// Identify the listing a create created, deterministically: diff the
    /// post-create listings against the pre-create snapshot. Exactly one
    /// new listing identifies it; several new listings identify it only if
    /// exactly one of them carries the requested title; anything else is
    /// an error rather than a guess.
    async fn resolve_created_listing(
        &self,
        endpoints: &Endpoints,
        before_ids: Option<&std::collections::HashSet<String>>,
        title: &str,
    ) -> Result<String, SyncError> {
        let Some(before_ids) = before_ids else {
            return Err(SyncError::ApiError {
                platform: Platform::XmrBazaar,
                message: "Listing creation could not be confirmed: no pre-create listing snapshot, and the response carried no listing id".into(),
            });
        };
        let listings = self.fetch_listings(endpoints).await?;
        resolve_created_listing_id(before_ids, &listings, title).ok_or_else(|| {
            SyncError::ApiError {
                platform: Platform::XmrBazaar,
                message: format!(
                    "Listing creation could not be confirmed deterministically for title '{title}': the new listings since the pre-create snapshot do not identify exactly one listing"
                ),
            }
        })
    }
}

/// Extract a listing ID from a URL path like `/listing/AbC1/` or full URL.
fn extract_listing_id_from_path(path: &str) -> Option<String> {
    let marker = "/listing/";
    let start = path.find(marker)? + marker.len();
    let rest = &path[start..];
    let end = rest.find('/').unwrap_or(rest.len());
    let candidate = &rest[..end];
    if !candidate.is_empty()
        && candidate.len() <= 20
        && candidate.chars().all(|c| c.is_alphanumeric())
    {
        Some(candidate.to_string())
    } else {
        None
    }
}

/// Whether a URL or Location value points at the login page.
fn is_login_url(url: &str) -> bool {
    url.to_lowercase().contains("/login")
}

/// Whether a response body is the login page (a password form posting to
/// the login endpoint) rather than a result page.
fn looks_like_login_page(body: &str) -> bool {
    let lower = body.to_lowercase();
    lower.contains("type=\"password\"") && lower.contains("/login")
}

/// Deterministically identify a just-created listing from a pre/post diff:
/// exactly one new listing wins; several new listings win only when
/// exactly one carries `title`; otherwise `None` (the caller errors).
fn resolve_created_listing_id(
    before_ids: &std::collections::HashSet<String>,
    listings: &[XmrListing],
    title: &str,
) -> Option<String> {
    let new: Vec<&XmrListing> = listings
        .iter()
        .filter(|l| !before_ids.contains(&l.id))
        .collect();
    match new.len() {
        1 => Some(new[0].id.clone()),
        0 => None,
        _ => {
            let titled: Vec<&&XmrListing> = new.iter().filter(|l| l.title == title).collect();
            if titled.len() == 1 {
                Some(titled[0].id.clone())
            } else {
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[derive(Clone, Copy, PartialEq)]
    enum PostEditMode {
        SaveAndRedirect,
        SaveAndOk,
        LoginBounce,
        ErrorForm,
        RedirectWithoutSaving,
    }

    #[derive(Clone, Copy, PartialEq)]
    enum CreateMode {
        RedirectToNewListing,
        RedirectNoId,
        LoginBounce,
        ErrorForm,
        OkWithDecoyLinks,
    }

    #[derive(Clone)]
    struct FakeState {
        edit_fields: Vec<(String, String)>,
        post_edit_mode: PostEditMode,
        listings: Vec<(String, String)>,
        create_mode: CreateMode,
        created: Vec<(String, String)>,
    }

    fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack.windows(needle.len()).position(|w| w == needle)
    }

    async fn read_request(socket: &mut tokio::net::TcpStream) -> (String, String) {
        let mut buf: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            if let Some(header_end) = find_subslice(&buf, b"\r\n\r\n") {
                let header_text = String::from_utf8_lossy(&buf[..header_end]).to_lowercase();
                let content_length: usize = header_text
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .and_then(|v| v.trim().parse().ok())
                    .unwrap_or(0);
                if buf.len() >= header_end + 4 + content_length {
                    break;
                }
            }
            match socket.read(&mut chunk).await {
                Ok(0) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(_) => break,
            }
        }
        let text = String::from_utf8_lossy(&buf).to_string();
        let first_line = text.lines().next().unwrap_or("").to_string();
        let path = first_line
            .split_whitespace()
            .nth(1)
            .unwrap_or("")
            .to_string();
        (path, text)
    }

    fn url_decode(s: &str) -> String {
        let bytes = s.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'+' => out.push(b' '),
                b'%' if i + 2 < bytes.len() => {
                    let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                    if let Ok(v) = u8::from_str_radix(hex, 16) {
                        out.push(v);
                        i += 2;
                    } else {
                        out.push(bytes[i]);
                    }
                }
                b => out.push(b),
            }
            i += 1;
        }
        String::from_utf8_lossy(&out).to_string()
    }

    fn parse_urlencoded(body: &str) -> Vec<(String, String)> {
        body.split('&')
            .filter(|p| !p.is_empty())
            .map(|p| {
                let (k, v) = p.split_once('=').unwrap_or((p, ""));
                (url_decode(k), url_decode(v))
            })
            .collect()
    }

    fn render_edit_form(fields: &[(String, String)]) -> String {
        let mut inputs = String::new();
        for (name, value) in fields {
            if name == "stock" {
                let mut options = vec!["unlimited", "one-time"];
                if !options.contains(&value.as_str()) {
                    options.push(value.as_str());
                }
                for opt in options {
                    let checked = if opt == value { " checked" } else { "" };
                    inputs.push_str(&format!(
                        "<input type=\"radio\" name=\"stock\" value=\"{opt}\"{checked}>"
                    ));
                }
            } else if name == "status" {
                let mut options = vec!["active", "out_of_stock", "draft"];
                if !options.contains(&value.as_str()) {
                    options.push(value.as_str());
                }
                let opts: String = options
                    .iter()
                    .map(|o| {
                        let sel = if *o == value { " selected" } else { "" };
                        format!("<option value=\"{o}\"{sel}>{o}</option>")
                    })
                    .collect();
                inputs.push_str(&format!("<select name=\"status\">{opts}</select>"));
            } else if name == "description" {
                inputs.push_str(&format!(
                    "<textarea name=\"description\">{value}</textarea>"
                ));
            } else {
                inputs.push_str(&format!(
                    "<input type=\"text\" name=\"{name}\" value=\"{value}\">"
                ));
            }
        }
        format!(
            "<html><body><form method=\"post\"><input type=\"hidden\" name=\"validation\" value=\"tok_test_token_123\">{inputs}</form></body></html>"
        )
    }

    fn render_listings(listings: &[(String, String)]) -> String {
        let rows: String = listings
            .iter()
            .map(|(id, title)| {
                format!(
                    "<tr class=\"my-sales-tr\"><td class=\"my-sales-td-guid\"><a href=\"/listing/{id}/\">{id}</a></td><td class=\"my-sales-td-status\"><span class=\"my-sales-status-tag\">Active</span></td><td class=\"my-sales-td-product\"><span class=\"my-sales-product-title\">{title}</span></td><td class=\"my-sales-td-price\"><span class=\"my-sales-price\">$10.00</span></td></tr>"
                )
            })
            .collect();
        format!("<html><body><table>{rows}</table></body></html>")
    }

    fn apply_posted(state: &mut FakeState, posted: Vec<(String, String)>) {
        for (k, v) in posted {
            if k == "validation" {
                continue;
            }
            if let Some(slot) = state.edit_fields.iter_mut().find(|(n, _)| *n == k) {
                slot.1 = v;
            } else {
                state.edit_fields.push((k, v));
            }
        }
    }

    async fn spawn_fake(state: FakeState) -> Endpoints {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let state = Arc::new(Mutex::new(state));
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let state = state.clone();
                tokio::spawn(async move {
                    let (path, raw) = read_request(&mut socket).await;
                    let method = raw.lines().next().unwrap_or("").to_string();
                    let body = raw
                        .split_once("\r\n\r\n")
                        .map(|(_, b)| b.to_string())
                        .unwrap_or_default();
                    let (status, headers, resp_body): (u16, Vec<(String, String)>, String) = {
                        let mut st = state.lock().unwrap();
                        if method.starts_with("GET") && path.starts_with("/edit-listing/") {
                            (200, vec![], render_edit_form(&st.edit_fields))
                        } else if method.starts_with("POST") && path.starts_with("/edit-listing/") {
                            match st.post_edit_mode {
                                PostEditMode::LoginBounce => (
                                    302,
                                    vec![("location".into(), "/login/".into())],
                                    String::new(),
                                ),
                                PostEditMode::ErrorForm => (
                                    200,
                                    vec![],
                                    format!(
                                        "<div class=\"error\">Value rejected by server</div>{}",
                                        render_edit_form(&st.edit_fields)
                                    ),
                                ),
                                PostEditMode::SaveAndRedirect
                                | PostEditMode::RedirectWithoutSaving => {
                                    if st.post_edit_mode == PostEditMode::SaveAndRedirect {
                                        apply_posted(&mut st, parse_urlencoded(&body));
                                    }
                                    (
                                        302,
                                        vec![("location".into(), "/my-listings/".into())],
                                        String::new(),
                                    )
                                }
                                PostEditMode::SaveAndOk => {
                                    apply_posted(&mut st, parse_urlencoded(&body));
                                    (200, vec![], "<html><body>Saved</body></html>".into())
                                }
                            }
                        } else if method.starts_with("GET") && path.starts_with("/my-listings") {
                            (200, vec![], render_listings(&st.listings))
                        } else if method.starts_with("GET") && path.starts_with("/new-listing") {
                            (
                                200,
                                vec![],
                                render_edit_form(&[
                                    ("title".to_string(), String::new()),
                                    ("price".to_string(), String::new()),
                                ]),
                            )
                        } else if method.starts_with("POST") && path.starts_with("/new-listing") {
                            match st.create_mode {
                                CreateMode::LoginBounce => (
                                    302,
                                    vec![("location".into(), "/login/".into())],
                                    String::new(),
                                ),
                                CreateMode::ErrorForm => (
                                    200,
                                    vec![],
                                    "<div class=\"error\">Title is required</div>".into(),
                                ),
                                CreateMode::RedirectToNewListing => {
                                    let created = st.created.clone();
                                    st.listings.extend(created.iter().cloned());
                                    let loc = created
                                        .first()
                                        .map(|(id, _)| format!("/listing/{id}/"))
                                        .unwrap_or_else(|| "/my-listings/".into());
                                    (302, vec![("location".into(), loc)], String::new())
                                }
                                CreateMode::RedirectNoId => {
                                    let created = st.created.clone();
                                    st.listings.extend(created.iter().cloned());
                                    (
                                        302,
                                        vec![("location".into(), "/my-listings/".into())],
                                        String::new(),
                                    )
                                }
                                CreateMode::OkWithDecoyLinks => {
                                    let created = st.created.clone();
                                    st.listings.extend(created.iter().cloned());
                                    // Body links to an unrelated old listing
                                    // first; only the diff identifies the new one.
                                    (
                                        200,
                                        vec![],
                                        "<html><body><a href=\"/listing/OLD1/\">old</a></body></html>".into(),
                                    )
                                }
                            }
                        } else {
                            (404, vec![], "not found".into())
                        }
                    };
                    let mut resp = format!(
                        "HTTP/1.1 {status} R\r\ncontent-type: text/html\r\ncontent-length: {}\r\nconnection: close\r\n",
                        resp_body.len()
                    );
                    for (k, v) in headers {
                        resp.push_str(&format!("{k}: {v}\r\n"));
                    }
                    resp.push_str("\r\n");
                    resp.push_str(&resp_body);
                    let _ = socket.write_all(resp.as_bytes()).await;
                    let _ = socket.flush().await;
                });
            }
        });
        Endpoints::new(format!("http://127.0.0.1:{port}"), HashMap::new())
    }

    fn base_state() -> FakeState {
        FakeState {
            edit_fields: vec![
                ("title".to_string(), "Widget".to_string()),
                ("price".to_string(), "48.00".to_string()),
                ("quantity".to_string(), "3".to_string()),
                ("stock".to_string(), "unlimited".to_string()),
                ("status".to_string(), "active".to_string()),
            ],
            post_edit_mode: PostEditMode::SaveAndRedirect,
            listings: vec![("OLD1".to_string(), "Existing Widget".to_string())],
            create_mode: CreateMode::RedirectToNewListing,
            created: vec![("NEW9".to_string(), "Brand New Widget".to_string())],
        }
    }

    fn create_request() -> CreateListingRequest {
        CreateListingRequest {
            title: "Brand New Widget".to_string(),
            description_html: Some("A widget".to_string()),
            sku: None,
            quantity: 1,
            price: Some(nisaba_core::types::ListingPrice {
                amount: rust_decimal::Decimal::new(25, 0),
                currency: "USD".to_string(),
            }),
            photo_urls: vec![],
            extras: HashMap::new(),
        }
    }

    // --- pure helpers ---

    #[test]
    fn resolve_created_id_requires_a_deterministic_diff() {
        let before: std::collections::HashSet<String> = ["OLD1".to_string()].into_iter().collect();
        let mk = |id: &str, title: &str| XmrListing {
            id: id.to_string(),
            title: title.to_string(),
            price: None,
            quantity: 1,
            image_url: None,
            edit_url: String::new(),
            description: None,
            photos: vec![],
        };
        // Page order must not matter: the new listing is second.
        let listings = vec![
            mk("OLD1", "Existing Widget"),
            mk("NEW9", "Brand New Widget"),
        ];
        assert_eq!(
            resolve_created_listing_id(&before, &listings, "Brand New Widget"),
            Some("NEW9".to_string())
        );
        // No new listing -> None, even though listings.first() exists.
        assert_eq!(
            resolve_created_listing_id(&before, &listings[..1], "Brand New Widget"),
            None
        );
        // Two new listings with the requested title on one -> that one.
        let listings = vec![
            mk("OLD1", "Existing Widget"),
            mk("NEW8", "Other"),
            mk("NEW9", "Brand New Widget"),
        ];
        assert_eq!(
            resolve_created_listing_id(&before, &listings, "Brand New Widget"),
            Some("NEW9".to_string())
        );
        // Two new listings, neither (or both) matching -> ambiguous.
        let listings = vec![
            mk("OLD1", "Existing Widget"),
            mk("NEW8", "Other"),
            mk("NEW7", "Another"),
        ];
        assert_eq!(
            resolve_created_listing_id(&before, &listings, "Brand New Widget"),
            None
        );
    }

    #[test]
    fn login_detection_covers_redirects_and_pages() {
        assert!(is_login_url("/login/"));
        assert!(is_login_url("https://example.test/login/?next=/edit/"));
        assert!(!is_login_url("/listing/NEW9/"));
        assert!(looks_like_login_page(
            "<form action=\"/login/\"><input type=\"password\" name=\"password\"></form>"
        ));
        assert!(!looks_like_login_page("<html><body>Saved</body></html>"));
    }

    // --- edit writes: fake server ---

    #[tokio::test]
    async fn update_listing_field_succeeds_only_when_read_back_matches() {
        let endpoints = spawn_fake(base_state()).await;
        let client = XmrBazaarClient::new();
        client
            .update_listing_field(&endpoints, "LST1", "price", "99.50")
            .await
            .expect("a saved-and-read-back price update must be Ok");
    }

    #[tokio::test]
    async fn update_listing_field_plain_200_with_save_is_ok() {
        let mut state = base_state();
        state.post_edit_mode = PostEditMode::SaveAndOk;
        let endpoints = spawn_fake(state).await;
        let client = XmrBazaarClient::new();
        client
            .update_listing_field(&endpoints, "LST1", "price", "99.50")
            .await
            .expect("a 200 with no errors and a matching read-back must be Ok");
    }

    #[tokio::test]
    async fn update_listing_field_login_bounce_is_err() {
        let mut state = base_state();
        state.post_edit_mode = PostEditMode::LoginBounce;
        let endpoints = spawn_fake(state).await;
        let client = XmrBazaarClient::new();
        let err = client
            .update_listing_field(&endpoints, "LST1", "price", "99.50")
            .await
            .expect_err("a POST bounced to /login/ must not count as success");
        assert!(
            matches!(err, SyncError::AuthError { .. }),
            "login bounce should be an auth error, got: {err}"
        );
    }

    #[tokio::test]
    async fn update_listing_field_error_form_is_err() {
        let mut state = base_state();
        state.post_edit_mode = PostEditMode::ErrorForm;
        let endpoints = spawn_fake(state).await;
        let client = XmrBazaarClient::new();
        client
            .update_listing_field(&endpoints, "LST1", "price", "99.50")
            .await
            .expect_err("a re-rendered form with errors must not count as success");
    }

    #[tokio::test]
    async fn update_listing_field_redirect_without_save_is_err() {
        // The server 302s to a plausible page but never saves: the
        // read-back still shows the old value, so this must be Err.
        let mut state = base_state();
        state.post_edit_mode = PostEditMode::RedirectWithoutSaving;
        let endpoints = spawn_fake(state).await;
        let client = XmrBazaarClient::new();
        client
            .update_listing_field(&endpoints, "LST1", "price", "99.50")
            .await
            .expect_err("a redirect alone must not count as success without read-back");
    }

    #[tokio::test]
    async fn update_quantity_verified_by_read_back() {
        let endpoints = spawn_fake(base_state()).await;
        let client = XmrBazaarClient::new();
        client
            .update_quantity(&endpoints, "LST1", 7)
            .await
            .expect("a saved quantity must verify by read-back");
    }

    #[tokio::test]
    async fn update_quantity_login_bounce_is_err() {
        let mut state = base_state();
        state.post_edit_mode = PostEditMode::LoginBounce;
        let endpoints = spawn_fake(state).await;
        let client = XmrBazaarClient::new();
        client
            .update_quantity(&endpoints, "LST1", 7)
            .await
            .expect_err("a login-bounced quantity update must be Err");
    }

    #[tokio::test]
    async fn set_stock_mode_verified_by_read_back() {
        let endpoints = spawn_fake(base_state()).await;
        let client = XmrBazaarClient::new();
        client
            .set_stock_mode(&endpoints, "LST1", false, None)
            .await
            .expect("a saved stock mode must verify by read-back");
    }

    #[tokio::test]
    async fn set_stock_mode_login_bounce_is_err() {
        let mut state = base_state();
        state.post_edit_mode = PostEditMode::LoginBounce;
        let endpoints = spawn_fake(state).await;
        let client = XmrBazaarClient::new();
        client
            .set_stock_mode(&endpoints, "LST1", false, None)
            .await
            .expect_err("a login-bounced stock mode update must be Err");
    }

    #[tokio::test]
    async fn set_stock_mode_without_stock_field_is_err() {
        let mut state = base_state();
        state
            .edit_fields
            .retain(|(n, _)| n != "stock" && n != "status");
        let endpoints = spawn_fake(state).await;
        let client = XmrBazaarClient::new();
        client
            .set_stock_mode(&endpoints, "LST1", false, None)
            .await
            .expect_err("no stock field must be Err, not Ok");
    }

    // --- create_listing: fake server ---

    #[tokio::test]
    async fn create_listing_uses_redirect_listing_id() {
        let endpoints = spawn_fake(base_state()).await;
        let client = XmrBazaarClient::new();
        let id = client
            .create_listing(&endpoints, &create_request(), "4FakeAddress")
            .await
            .expect("a redirect naming the new listing must be Ok");
        assert_eq!(id, "NEW9");
    }

    #[tokio::test]
    async fn create_listing_without_redirect_id_uses_diff_not_first() {
        // OLD1 stays first on my-listings; the created listing is second.
        // The old code returned listings.first() == OLD1 here.
        let mut state = base_state();
        state.create_mode = CreateMode::RedirectNoId;
        let endpoints = spawn_fake(state).await;
        let client = XmrBazaarClient::new();
        let id = client
            .create_listing(&endpoints, &create_request(), "4FakeAddress")
            .await
            .expect("a unique new listing in the diff must be Ok");
        assert_eq!(id, "NEW9");
    }

    #[tokio::test]
    async fn create_listing_login_bounce_is_err() {
        let mut state = base_state();
        state.create_mode = CreateMode::LoginBounce;
        let endpoints = spawn_fake(state).await;
        let client = XmrBazaarClient::new();
        let err = client
            .create_listing(&endpoints, &create_request(), "4FakeAddress")
            .await
            .expect_err("a login-bounced create must be Err");
        assert!(matches!(err, SyncError::AuthError { .. }), "got: {err}");
    }

    #[tokio::test]
    async fn create_listing_error_form_is_err() {
        let mut state = base_state();
        state.create_mode = CreateMode::ErrorForm;
        let endpoints = spawn_fake(state).await;
        let client = XmrBazaarClient::new();
        client
            .create_listing(&endpoints, &create_request(), "4FakeAddress")
            .await
            .expect_err("a 200 error form must be Err, not a body-link guess");
    }

    #[tokio::test]
    async fn create_listing_body_decoy_link_is_not_the_answer() {
        // 200 body links to OLD1 first; the created listing is only
        // identifiable via the my-listings diff.
        let mut state = base_state();
        state.create_mode = CreateMode::OkWithDecoyLinks;
        let endpoints = spawn_fake(state).await;
        let client = XmrBazaarClient::new();
        let id = client
            .create_listing(&endpoints, &create_request(), "4FakeAddress")
            .await
            .expect("the diff must identify the created listing");
        assert_eq!(id, "NEW9", "must not record the decoy body link");
    }

    #[tokio::test]
    async fn create_listing_with_no_new_listing_is_err() {
        let mut state = base_state();
        state.create_mode = CreateMode::RedirectNoId;
        state.created = vec![];
        let endpoints = spawn_fake(state).await;
        let client = XmrBazaarClient::new();
        client
            .create_listing(&endpoints, &create_request(), "4FakeAddress")
            .await
            .expect_err("a redirect with no new listing must be Err, not first()");
    }

    #[tokio::test]
    async fn create_listing_with_ambiguous_new_listings_is_err() {
        let mut state = base_state();
        state.create_mode = CreateMode::RedirectNoId;
        state.created = vec![
            ("NEW8".to_string(), "Other Widget".to_string()),
            ("NEW7".to_string(), "Another Widget".to_string()),
        ];
        let endpoints = spawn_fake(state).await;
        let client = XmrBazaarClient::new();
        client
            .create_listing(&endpoints, &create_request(), "4FakeAddress")
            .await
            .expect_err("two unrelated new listings must be ambiguous -> Err");
    }

    #[test]
    fn snippet_shorter_than_the_limit_is_returned_whole() {
        assert_eq!(body_snippet("short body", 200), "short body");
    }

    #[test]
    fn snippet_truncates_ascii_at_the_limit() {
        let body = "a".repeat(300);
        assert_eq!(body_snippet(&body, 200).len(), 200);
    }

    #[test]
    fn snippet_never_splits_a_multibyte_character() {
        // 199 ASCII bytes followed by a 2-byte 'é' straddling byte 200: the
        // old `&body[..200]` slice panicked here.
        let mut body = "a".repeat(199);
        body.push('é');
        body.push_str(&"b".repeat(100));
        let snippet = body_snippet(&body, 200);
        assert_eq!(snippet.len(), 199, "backs off to the char boundary");
        assert!(snippet.ends_with('a'));
    }

    #[test]
    fn snippet_of_a_multibyte_error_body_is_valid_utf8() {
        // A remote error page full of multi-byte characters: every limit
        // must yield a valid &str prefix, never a panic.
        let body = "€".repeat(200); // 600 bytes, 3 bytes per char
        for limit in [1, 2, 3, 200, 500] {
            let snippet = body_snippet(&body, limit);
            assert!(snippet.len() <= limit);
            assert_eq!(snippet.len() % 3, 0, "whole characters only");
        }
        assert_eq!(body_snippet(&body, 500).chars().count(), 166);
    }
}
