use nisaba_core::error::SyncError;
use nisaba_core::types::Platform;
use tracing::{debug, info, warn};

use crate::client::XmrBazaarClient;
use crate::endpoints::Endpoints;
use crate::scraper as html;

/// Session-based auth for XMR Bazaar: PHP session with form login.
pub struct XmrBazaarAuth {
    username: String,
    password: String,
}

impl XmrBazaarAuth {
    pub fn new(username: String, password: String) -> Self {
        Self { username, password }
    }

    /// Login by POSTing credentials to the login endpoint.
    ///
    /// XMR Bazaar is a PHP site using PHPSESSID cookies and a hidden
    /// "validation" form field for CSRF protection. The login flow is:
    /// 1. GET /login/ — seeds the PHPSESSID cookie and returns a form
    ///    with a hidden "validation" field
    /// 2. POST /login/ with action=login, validation=<token>,
    ///    username=<user>, password=<pass>
    /// 3. On success: 302 redirect to / with a new PHPSESSID
    pub async fn login(
        &self,
        client: &XmrBazaarClient,
        endpoints: &Endpoints,
    ) -> Result<(), SyncError> {
        let http = client.http();
        let (_, login_url) = endpoints.login();

        // Step 1: GET the login page to seed PHPSESSID cookie and extract validation token
        let page_resp = http
            .get(&login_url)
            .send()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::XmrBazaar,
                message: e.to_string(),
            })?;

        let page_status = page_resp.status();
        debug!(
            status = %page_status,
            final_url = %page_resp.url(),
            "GET login page response"
        );

        let page_html = page_resp
            .text()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::XmrBazaar,
                message: e.to_string(),
            })?;

        // Parse the login form to discover field names and extract all hidden fields
        let login_form = html::parse_login_form(&page_html);

        debug!(
            has_form = login_form.is_some(),
            "Parsed login page form"
        );

        // Build form params from the actual form fields
        let form_params: Vec<(String, String)> = if let Some(form) = &login_form {
            debug!(
                token_field = %form.token_field_name,
                token_len = form.token_value.len(),
                username_field = %form.username_field_name,
                password_field = %form.password_field_name,
                hidden_field_count = form.hidden_fields.len(),
                "Login form field discovery"
            );

            let mut params: Vec<(String, String)> = Vec::new();

            // Include all hidden fields (validation token, action, etc.)
            for (name, value) in &form.hidden_fields {
                params.push((name.clone(), value.clone()));
            }

            // Set username and password using discovered field names
            params.push((form.username_field_name.clone(), self.username.clone()));
            params.push((form.password_field_name.clone(), self.password.clone()));

            params
        } else {
            // Fallback: use best-guess field names
            let validation_token =
                html::extract_validation_token(&page_html).unwrap_or_default();

            warn!(
                has_token = !validation_token.is_empty(),
                "Could not parse login form, falling back to guessed field names"
            );

            let mut params = vec![
                ("action".to_string(), "login".to_string()),
                ("username".to_string(), self.username.clone()),
                ("password".to_string(), self.password.clone()),
            ];

            if !validation_token.is_empty() {
                params.push(("validation".to_string(), validation_token));
            }

            params
        };

        debug!(
            field_count = form_params.len(),
            fields = ?form_params.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
            "Submitting login form"
        );

        // Step 2: POST with a no-redirect client that shares the same cookie jar.
        // This avoids reqwest's redirect following, which can race with Set-Cookie
        // processing — the 302 response sets a new PHPSESSID, but the auto-redirect
        // GET may fire before the cookie store is updated, causing a redirect loop.
        let no_redirect = client.no_redirect_http();

        let login_resp = no_redirect
            .post(&login_url)
            .header("Referer", &login_url)
            .header("Origin", endpoints.base_url())
            .form(&form_params)
            .send()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::XmrBazaar,
                message: e.to_string(),
            })?;

        let status = login_resp.status();

        // Extract Location header — try to_str() first, fall back to lossy UTF-8
        let location = login_resp
            .headers()
            .get("location")
            .map(|v| {
                v.to_str()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|_| String::from_utf8_lossy(v.as_bytes()).to_string())
            })
            .unwrap_or_default();

        // Log all response headers for diagnostics
        let header_names: Vec<&str> = login_resp
            .headers()
            .keys()
            .map(|k| k.as_str())
            .collect();

        debug!(
            status = %status,
            location = %location,
            headers = ?header_names,
            "Login POST response (no redirect)"
        );

        if status.as_u16() == 302 {
            // On any 302, verify the session by actually fetching a protected page.
            // The Location header may be empty or absent on some servers, but
            // the Set-Cookie header may have set a valid PHPSESSID anyway.
            let (_, listings_url) = endpoints.my_listings();
            let check = http
                .get(&listings_url)
                .send()
                .await
                .map_err(|e| SyncError::NetworkError {
                    platform: Platform::XmrBazaar,
                    message: e.to_string(),
                })?;

            let check_url = check.url().to_string();
            debug!(
                check_url = %check_url,
                check_status = %check.status(),
                "Session verification after login"
            );

            if !check_url.contains("/login") {
                info!("XMR Bazaar login successful and session verified");
                Ok(())
            } else if location.contains("/login") {
                Err(SyncError::AuthError {
                    platform: Platform::XmrBazaar,
                    message: "Login failed: server redirected back to login page (bad credentials or expired validation token)".into(),
                })
            } else {
                warn!(
                    location = %location,
                    "Login 302 received but session verification failed"
                );
                Err(SyncError::AuthError {
                    platform: Platform::XmrBazaar,
                    message: format!(
                        "Login returned 302 (Location: '{}') but session cookie was not stored. \
                         Check credentials and cookie handling.",
                        location
                    ),
                })
            }
        } else if status.is_success() {
            // Some servers return 200 on failed login with error message in body
            Err(SyncError::AuthError {
                platform: Platform::XmrBazaar,
                message: format!("Login failed: server returned {status} instead of redirect"),
            })
        } else {
            Err(SyncError::AuthError {
                platform: Platform::XmrBazaar,
                message: format!("Login failed with status {status}"),
            })
        }
    }

    /// Check if the session is still valid by fetching the listings page.
    pub async fn check_session(&self, client: &XmrBazaarClient, endpoints: &Endpoints) -> bool {
        let (_, url) = endpoints.my_listings();
        match client.http().get(&url).send().await {
            Ok(resp) => {
                let final_url = resp.url().to_string();
                !final_url.contains("/login")
            }
            Err(_) => false,
        }
    }
}
