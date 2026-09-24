use std::sync::Arc;

use nisaba_core::error::SyncError;
use nisaba_core::types::{CreateListingRequest, Platform};
use reqwest::{cookie::Jar, Client};
use tracing::debug;

use crate::endpoints::Endpoints;
use crate::scraper as html;
use crate::types::{StockModeField, XmrListing, XmrListingDetail, XmrSaleRecord};

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; rv:128.0) Gecko/20100101 Firefox/128.0";

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
        let mut form_params: Vec<(&str, &str)> = Vec::new();
        form_params.push((&form_data.csrf_field_name, &form_data.csrf_token));

        let mut quantity_set = false;
        for (name, value) in &form_data.fields {
            if name == "quantity" || name == "stock" || name == "qty" {
                form_params.push((name, &qty_str));
                quantity_set = true;
            } else {
                form_params.push((name, value));
            }
        }

        if !quantity_set {
            // Try a common field name if none was found
            form_params.push(("quantity", &qty_str));
        }

        // POST the form back
        let resp = self
            .http
            .post(&edit_url)
            .header("Referer", &edit_url)
            .form(&form_params)
            .send()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::XmrBazaar,
                message: e.to_string(),
            })?;

        let status = resp.status();
        if status.is_redirection() || status.is_success() {
            debug!(listing_id, quantity, "XMR Bazaar quantity updated");
            Ok(())
        } else {
            let body = resp.text().await.unwrap_or_default();
            Err(SyncError::ApiError {
                platform: Platform::XmrBazaar,
                message: format!(
                    "Edit form submission failed ({status}): {}",
                    &body[..body.len().min(200)]
                ),
            })
        }
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
            tracing::warn!(
                listing_id,
                "No stock mode field found in edit form, skipping stock mode toggle"
            );
            return Ok(());
        };

        let target_value = if active {
            &field.unlimited_value
        } else {
            &field.onetime_value
        };

        // Build form submission with updated stock mode field
        let mut form_params: Vec<(String, String)> = Vec::new();
        form_params.push((
            form_data.csrf_field_name.clone(),
            form_data.csrf_token.clone(),
        ));

        let mut field_set = false;
        for (name, value) in &form_data.fields {
            if name == &field.field_name {
                form_params.push((name.clone(), target_value.clone()));
                field_set = true;
            } else if name.eq_ignore_ascii_case("status") {
                let status_val = if active { "active" } else { "out_of_stock" };
                form_params.push((name.clone(), status_val.to_string()));
            } else {
                form_params.push((name.clone(), value.clone()));
            }
        }

        if !field_set {
            form_params.push((field.field_name.clone(), target_value.clone()));
        }

        // POST the form back
        let resp = self
            .http
            .post(&edit_url)
            .header("Referer", &edit_url)
            .form(&form_params)
            .send()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::XmrBazaar,
                message: e.to_string(),
            })?;

        let status = resp.status();
        if status.is_redirection() || status.is_success() {
            debug!(
                listing_id,
                active,
                field_name = %field.field_name,
                target_value = %target_value,
                "XMR Bazaar stock mode updated"
            );
            Ok(())
        } else {
            let body = resp.text().await.unwrap_or_default();
            Err(SyncError::ApiError {
                platform: Platform::XmrBazaar,
                message: format!(
                    "Stock mode update failed ({status}): {}",
                    &body[..body.len().min(200)]
                ),
            })
        }
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

        // On redirect, check Location header for the new listing ID
        if status.is_redirection() {
            if let Some(location) = resp.headers().get("location") {
                if let Ok(loc_str) = location.to_str() {
                    if let Some(new_id) = extract_listing_id_from_path(loc_str) {
                        debug!(listing_id = %new_id, "Created XMR Bazaar listing (from redirect)");
                        return Ok(new_id);
                    }
                }
            }
            // Redirect but no listing ID in location — fetch latest listing
            debug!(status = %status, "Redirect after create — fetching latest listing");
            let listings = self.fetch_listings(endpoints).await?;
            if let Some(newest) = listings.first() {
                debug!(listing_id = %newest.id, "Created XMR Bazaar listing (from my-listings)");
                return Ok(newest.id.clone());
            }
        }

        let body = resp.text().await.map_err(|e| SyncError::NetworkError {
            platform: Platform::XmrBazaar,
            message: e.to_string(),
        })?;

        // Try to extract a listing ID from the response body
        if let Some(new_id) = extract_listing_id_from_body(&body) {
            debug!(listing_id = %new_id, status = %status, "Created XMR Bazaar listing (from body)");
            return Ok(new_id);
        }

        // Try to find validation errors in the HTML for a useful error message
        let validation_errors = html::extract_form_errors(&body);
        let error_detail = if !validation_errors.is_empty() {
            format!("Validation errors: {}", validation_errors.join("; "))
        } else {
            format!("Response: {}", &body[..body.len().min(500)])
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

        let mut field_set = false;
        for (name, value) in &form_data.fields {
            if name == field_name {
                form_params.push((name.clone(), field_value.to_string()));
                field_set = true;
            } else {
                form_params.push((name.clone(), value.clone()));
            }
        }

        if !field_set {
            // Field was not present in the form; add it explicitly
            form_params.push((field_name.to_string(), field_value.to_string()));
        }

        // POST the form back
        let resp = self
            .http
            .post(&edit_url)
            .header("Referer", &edit_url)
            .form(&form_params)
            .send()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::XmrBazaar,
                message: e.to_string(),
            })?;

        let status = resp.status();
        if status.is_redirection() || status.is_success() {
            debug!(listing_id, field_name, "XMR Bazaar field updated");
            Ok(())
        } else {
            let body = resp.text().await.unwrap_or_default();
            Err(SyncError::ApiError {
                platform: Platform::XmrBazaar,
                message: format!(
                    "Edit form submission failed for field '{}' ({status}): {}",
                    field_name,
                    &body[..body.len().min(200)]
                ),
            })
        }
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

/// Extract a listing ID from response body by searching for `/listing/{id}/` patterns.
fn extract_listing_id_from_body(body: &str) -> Option<String> {
    let marker = "/listing/";
    let mut search_from = 0;
    while let Some(start) = body[search_from..].find(marker) {
        let abs_start = search_from + start + marker.len();
        if let Some(end) = body[abs_start..].find('/') {
            let candidate = &body[abs_start..abs_start + end];
            if !candidate.is_empty()
                && candidate.len() <= 20
                && candidate.chars().all(|c| c.is_alphanumeric())
            {
                return Some(candidate.to_string());
            }
        }
        search_from = abs_start;
    }
    None
}
