use nisaba_core::error::SyncError;
use nisaba_core::types::Platform;
use reqwest::Client;
use serde_json::json;
use tracing::{debug, warn};

use crate::types::*;

const NA_BASE_URL: &str = "https://sellingpartnerapi-na.amazon.com";
const EU_BASE_URL: &str = "https://sellingpartnerapi-eu.amazon.com";
const FE_BASE_URL: &str = "https://sellingpartnerapi-fe.amazon.com";

const LISTINGS_API_VERSION: &str = "2021-08-01";

/// Percent-encode a string for use in URL path segments.
fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                String::from(b as char)
            }
            _ => format!("%{:02X}", b),
        })
        .collect()
}

/// Build the PATCH body for a quantity (fulfillment_availability) update.
///
/// `AmazonPatch::value` is the attribute value array itself, so each element
/// must be the attribute object directly: `"value": [{…}]`. Wrapping the
/// object in a further `json!([…])` serialises as `"value": [[{…}]]`, which
/// Amazon rejects (N-5.3).
pub fn build_quantity_patch_request(marketplace_id: &str, qty: i64) -> AmazonPatchRequest {
    AmazonPatchRequest {
        product_type: "PRODUCT".to_string(),
        patches: vec![AmazonPatch {
            op: "replace".to_string(),
            path: "/attributes/fulfillment_availability".to_string(),
            value: vec![json!({
                "fulfillment_channel_code": "DEFAULT",
                "quantity": qty,
                "marketplace_id": marketplace_id,
            })],
        }],
    }
}

/// Build the PATCH body for a price (purchasable_offer) update. Same
/// single-wrapping rule as [`build_quantity_patch_request`].
pub fn build_price_patch_request(
    marketplace_id: &str,
    amount: f64,
    currency: &str,
) -> AmazonPatchRequest {
    AmazonPatchRequest {
        product_type: "PRODUCT".to_string(),
        patches: vec![AmazonPatch {
            op: "replace".to_string(),
            path: "/attributes/purchasable_offer".to_string(),
            value: vec![json!({
                "marketplace_id": marketplace_id,
                "currency": currency,
                "our_price": [{
                    "schedule": [{
                        "value_with_tax": format!("{:.2}", amount),
                    }]
                }]
            })],
        }],
    }
}

/// Surface a rejected listing submission as `Err`.
///
/// Amazon answers writes with HTTP 200 even when it rejects the change: the
/// submission body carries `status: "INVALID"` and/or `issues` with
/// `severity: "ERROR"`. Treating the HTTP status alone as success reported
/// `Ok` for writes that never landed (N-5.3). Non-error (warning) issues are
/// logged and do not fail the write.
pub fn check_submission(
    submission: &AmazonListingSubmissionResponse,
    context: &str,
) -> Result<(), SyncError> {
    let error_issues: Vec<&AmazonListingIssue> = submission
        .issues
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .filter(|issue| {
            issue
                .severity
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case("ERROR"))
        })
        .collect();

    for issue in submission.issues.as_deref().unwrap_or(&[]) {
        let is_error = issue
            .severity
            .as_deref()
            .is_some_and(|s| s.eq_ignore_ascii_case("ERROR"));
        if !is_error {
            warn!(
                code = issue.code.as_deref().unwrap_or("?"),
                message = issue.message.as_deref().unwrap_or(""),
                "Amazon listing issue ({context})"
            );
        }
    }

    let is_invalid = submission
        .status
        .as_deref()
        .is_some_and(|s| s.eq_ignore_ascii_case("INVALID"));

    if is_invalid || !error_issues.is_empty() {
        let details: Vec<String> = error_issues
            .iter()
            .map(|issue| {
                format!(
                    "{}: {}",
                    issue.code.as_deref().unwrap_or("?"),
                    issue.message.as_deref().unwrap_or("")
                )
            })
            .collect();
        // An INVALID submission with no ERROR-severity issue still carries
        // useful detail in its remaining issues.
        let details = if details.is_empty() {
            submission
                .issues
                .as_deref()
                .unwrap_or(&[])
                .iter()
                .map(|issue| {
                    format!(
                        "{}: {}",
                        issue.code.as_deref().unwrap_or("?"),
                        issue.message.as_deref().unwrap_or("")
                    )
                })
                .collect::<Vec<_>>()
                .join("; ")
        } else {
            details.join("; ")
        };
        return Err(SyncError::ApiError {
            platform: Platform::Amazon,
            message: format!(
                "{context}: Amazon rejected the submission (status: {}){}",
                submission.status.as_deref().unwrap_or("?"),
                if details.is_empty() {
                    String::new()
                } else {
                    format!(": {details}")
                },
            ),
        });
    }

    Ok(())
}

pub struct AmazonClient {
    http: Client,
    api_base: String,
}

impl AmazonClient {
    pub fn new(region: &str) -> Self {
        let http = Client::builder()
            .user_agent("Nisaba/0.1.0")
            .build()
            .expect("Failed to build HTTP client");

        let api_base = match region.to_uppercase().as_str() {
            "EU" => EU_BASE_URL,
            "FE" => FE_BASE_URL,
            _ => NA_BASE_URL, // Default to NA
        }
        .to_string();

        Self { http, api_base }
    }

    /// Construct a client pointed at an explicit API base URL. Used by
    /// fake-server tests (and any future non-standard endpoint).
    pub fn with_api_base(api_base: String) -> Self {
        let http = Client::builder()
            .user_agent("Nisaba/0.1.0")
            .build()
            .expect("Failed to build HTTP client");
        Self { http, api_base }
    }

    pub fn http(&self) -> &Client {
        &self.http
    }

    /// Build the marketplace_ids query parameter string.
    fn marketplace_query(marketplace_ids: &[String]) -> String {
        marketplace_ids
            .iter()
            .map(|id| format!("marketplaceIds={}", url_encode(id)))
            .collect::<Vec<_>>()
            .join("&")
    }

    /// Fetch all listings for a seller across the given marketplaces.
    pub async fn fetch_listings(
        &self,
        access_token: &str,
        seller_id: &str,
        marketplace_ids: &[String],
    ) -> Result<Vec<AmazonListingItem>, SyncError> {
        let mut all_items = Vec::new();
        let mut seen_skus = std::collections::HashSet::new();

        for marketplace_id in marketplace_ids {
            let mut page_token: Option<String> = None;

            loop {
                let mut url = format!(
                    "{}/listings/{}/items/{}?marketplaceIds={}&includedData=summaries,fulfillmentAvailability,offers&pageSize=20",
                    self.api_base,
                    LISTINGS_API_VERSION,
                    url_encode(seller_id),
                    url_encode(marketplace_id),
                );

                if let Some(ref token) = page_token {
                    url.push_str(&format!("&pageToken={}", url_encode(token)));
                }

                let resp = self
                    .http
                    .get(&url)
                    .header("x-amz-access-token", access_token)
                    .send()
                    .await
                    .map_err(|e| SyncError::NetworkError {
                        platform: Platform::Amazon,
                        message: e.to_string(),
                    })?;

                let status = resp.status();
                if status.as_u16() == 401 || status.as_u16() == 403 {
                    return Err(SyncError::AuthError {
                        platform: Platform::Amazon,
                        message: format!("Access denied ({status})"),
                    });
                }
                if !status.is_success() {
                    let body = resp.text().await.unwrap_or_default();
                    return Err(SyncError::ApiError {
                        platform: Platform::Amazon,
                        message: format!("Listings fetch failed ({status}): {body}"),
                    });
                }

                let response: AmazonListingsResponse =
                    resp.json().await.map_err(|e| SyncError::ApiError {
                        platform: Platform::Amazon,
                        message: format!("Failed to parse listings response: {e}"),
                    })?;

                for item in response.items {
                    if seen_skus.insert(item.sku.clone()) {
                        all_items.push(item);
                    }
                }

                match response.next_token {
                    Some(token) if !token.is_empty() => page_token = Some(token),
                    _ => break,
                }
            }
        }

        debug!(count = all_items.len(), "Fetched Amazon listings");
        Ok(all_items)
    }

    /// Fetch a single listing item by SKU.
    pub async fn fetch_listing(
        &self,
        access_token: &str,
        seller_id: &str,
        sku: &str,
        marketplace_ids: &[String],
    ) -> Result<AmazonListingItem, SyncError> {
        let mkt_query = Self::marketplace_query(marketplace_ids);
        let url = format!(
            "{}/listings/{}/items/{}/{}?{}&includedData=summaries,fulfillmentAvailability,offers,issues",
            self.api_base,
            LISTINGS_API_VERSION,
            url_encode(seller_id),
            url_encode(sku),
            mkt_query,
        );

        let resp = self
            .http
            .get(&url)
            .header("x-amz-access-token", access_token)
            .send()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::Amazon,
                message: e.to_string(),
            })?;

        let status = resp.status();
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(SyncError::AuthError {
                platform: Platform::Amazon,
                message: format!("Access denied ({status})"),
            });
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SyncError::ApiError {
                platform: Platform::Amazon,
                message: format!("Fetch listing failed ({status}): {body}"),
            });
        }

        let item: AmazonListingItem = resp.json().await.map_err(|e| SyncError::ApiError {
            platform: Platform::Amazon,
            message: format!("Failed to parse listing response: {e}"),
        })?;

        debug!(sku = sku, "Fetched Amazon listing");
        Ok(item)
    }

    /// Update the quantity for a listing via PATCH on fulfillment_availability.
    pub async fn update_quantity(
        &self,
        access_token: &str,
        seller_id: &str,
        sku: &str,
        marketplace_ids: &[String],
        qty: i64,
    ) -> Result<(), SyncError> {
        for marketplace_id in marketplace_ids {
            let url = format!(
                "{}/listings/{}/items/{}/{}?marketplaceIds={}",
                self.api_base,
                LISTINGS_API_VERSION,
                url_encode(seller_id),
                url_encode(sku),
                url_encode(marketplace_id),
            );

            let body = build_quantity_patch_request(marketplace_id, qty);

            let resp = self
                .http
                .patch(&url)
                .header("x-amz-access-token", access_token)
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| SyncError::NetworkError {
                    platform: Platform::Amazon,
                    message: e.to_string(),
                })?;

            let status = resp.status();
            if status.as_u16() == 401 || status.as_u16() == 403 {
                return Err(SyncError::AuthError {
                    platform: Platform::Amazon,
                    message: format!("Access denied ({status})"),
                });
            }
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(SyncError::ApiError {
                    platform: Platform::Amazon,
                    message: format!(
                        "Update quantity failed for {sku} on {marketplace_id} ({status}): {body}"
                    ),
                });
            }

            let submission: AmazonListingSubmissionResponse =
                resp.json().await.map_err(|e| SyncError::ApiError {
                    platform: Platform::Amazon,
                    message: format!("Failed to parse patch response: {e}"),
                })?;

            check_submission(
                &submission,
                &format!("Update quantity failed for {sku} on {marketplace_id}"),
            )?;
        }

        debug!(sku = sku, qty = qty, "Updated Amazon quantity");
        Ok(())
    }

    /// Update the price for a listing via PATCH on purchasable_offer.
    pub async fn update_price(
        &self,
        access_token: &str,
        seller_id: &str,
        sku: &str,
        marketplace_ids: &[String],
        amount: f64,
        currency: &str,
    ) -> Result<(), SyncError> {
        for marketplace_id in marketplace_ids {
            let url = format!(
                "{}/listings/{}/items/{}/{}?marketplaceIds={}",
                self.api_base,
                LISTINGS_API_VERSION,
                url_encode(seller_id),
                url_encode(sku),
                url_encode(marketplace_id),
            );

            let body = build_price_patch_request(marketplace_id, amount, currency);

            let resp = self
                .http
                .patch(&url)
                .header("x-amz-access-token", access_token)
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| SyncError::NetworkError {
                    platform: Platform::Amazon,
                    message: e.to_string(),
                })?;

            let status = resp.status();
            if status.as_u16() == 401 || status.as_u16() == 403 {
                return Err(SyncError::AuthError {
                    platform: Platform::Amazon,
                    message: format!("Access denied ({status})"),
                });
            }
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(SyncError::ApiError {
                    platform: Platform::Amazon,
                    message: format!(
                        "Update price failed for {sku} on {marketplace_id} ({status}): {body}"
                    ),
                });
            }

            let submission: AmazonListingSubmissionResponse =
                resp.json().await.map_err(|e| SyncError::ApiError {
                    platform: Platform::Amazon,
                    message: format!("Failed to parse patch response: {e}"),
                })?;

            check_submission(
                &submission,
                &format!("Update price failed for {sku} on {marketplace_id}"),
            )?;
        }

        debug!(sku = sku, amount = amount, "Updated Amazon price");
        Ok(())
    }

    /// Create a new listing via PUT.
    pub async fn create_listing(
        &self,
        access_token: &str,
        seller_id: &str,
        sku: &str,
        marketplace_ids: &[String],
        product_type: &str,
        attributes: serde_json::Value,
    ) -> Result<AmazonListingSubmissionResponse, SyncError> {
        let mkt_query = Self::marketplace_query(marketplace_ids);
        let url = format!(
            "{}/listings/{}/items/{}/{}?{}",
            self.api_base,
            LISTINGS_API_VERSION,
            url_encode(seller_id),
            url_encode(sku),
            mkt_query,
        );

        let body = AmazonPutListingRequest {
            product_type: product_type.to_string(),
            requirements: "LISTING".to_string(),
            attributes,
        };

        let resp = self
            .http
            .put(&url)
            .header("x-amz-access-token", access_token)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::Amazon,
                message: e.to_string(),
            })?;

        let status = resp.status();
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(SyncError::AuthError {
                platform: Platform::Amazon,
                message: format!("Access denied ({status})"),
            });
        }
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(SyncError::ApiError {
                platform: Platform::Amazon,
                message: format!("Create listing failed ({status}): {body_text}"),
            });
        }

        let submission: AmazonListingSubmissionResponse =
            resp.json().await.map_err(|e| SyncError::ApiError {
                platform: Platform::Amazon,
                message: format!("Failed to parse create listing response: {e}"),
            })?;

        check_submission(&submission, &format!("Create listing failed for {sku}"))?;

        debug!(sku = sku, "Created Amazon listing");
        Ok(submission)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Read one HTTP/1.1 request (headers + content-length body).
    async fn read_request(socket: &mut tokio::net::TcpStream) -> String {
        let mut buf: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 4096];
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
        String::from_utf8_lossy(&buf).to_string()
    }

    fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack.windows(needle.len()).position(|w| w == needle)
    }

    /// Spawn a fake Amazon server that answers every request with the given
    /// status/body and captures raw requests for assertions.
    async fn spawn_fake_server(status: u16, body: String) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let captured_clone = captured.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let captured_inner = captured_clone.clone();
                let body_inner = body.clone();
                tokio::spawn(async move {
                    let req = read_request(&mut socket).await;
                    captured_inner.lock().unwrap().push(req);
                    let resp = format!(
                        "HTTP/1.1 {status} OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        body_inner.len(),
                        body_inner
                    );
                    let _ = socket.write_all(resp.as_bytes()).await;
                    let _ = socket.flush().await;
                });
            }
        });
        (format!("http://127.0.0.1:{port}"), captured)
    }

    fn request_body(raw: &str) -> &str {
        raw.split("\r\n\r\n").nth(1).unwrap_or("")
    }

    const ACCEPTED_BODY: &str =
        r#"{"sku":"SKU-1","status":"ACCEPTED","submissionId":"sub-1","issues":[]}"#;
    const INVALID_BODY: &str = r#"{"sku":"SKU-1","status":"INVALID","submissionId":"sub-2","issues":[{"code":"4000095","message":"Invalid quantity","severity":"ERROR"}]}"#;
    const ERROR_ISSUE_BODY: &str = r#"{"sku":"SKU-1","status":"ACCEPTED","submissionId":"sub-3","issues":[{"code":"90241","message":"Price is not valid","severity":"ERROR"}]}"#;

    fn submission(status: &str, severity: Option<&str>) -> AmazonListingSubmissionResponse {
        AmazonListingSubmissionResponse {
            sku: Some("SKU-1".to_string()),
            status: Some(status.to_string()),
            submission_id: Some("sub-x".to_string()),
            issues: severity.map(|sev| {
                vec![AmazonListingIssue {
                    code: Some("E1".to_string()),
                    message: Some("something is wrong".to_string()),
                    severity: Some(sev.to_string()),
                }]
            }),
        }
    }

    #[test]
    fn check_submission_rejects_invalid_and_error_issues_only() {
        assert!(check_submission(&submission("INVALID", None), "ctx").is_err());
        assert!(check_submission(&submission("ACCEPTED", Some("ERROR")), "ctx").is_err());
        assert!(check_submission(&submission("ACCEPTED", Some("WARNING")), "ctx").is_ok());
        assert!(check_submission(&submission("ACCEPTED", None), "ctx").is_ok());
    }

    #[test]
    fn quantity_patch_body_serialises_as_single_wrapped_value_array() {
        let body = build_quantity_patch_request("ATVPDKIKX0DER", 5);
        let value = serde_json::to_value(&body).unwrap();
        let patch_value = &value["patches"][0]["value"];
        // value must be [{…}], not [[{…}]].
        assert_eq!(
            patch_value,
            &json!([{
                "fulfillment_channel_code": "DEFAULT",
                "quantity": 5,
                "marketplace_id": "ATVPDKIKX0DER",
            }])
        );
        assert!(
            patch_value[0].is_object(),
            "value[0] must be the object: {patch_value}"
        );
        assert!(!serde_json::to_string(&body).unwrap().contains("[[{"));
    }

    #[test]
    fn price_patch_body_serialises_as_single_wrapped_value_array() {
        let body = build_price_patch_request("ATVPDKIKX0DER", 19.99, "USD");
        let value = serde_json::to_value(&body).unwrap();
        let patch_value = &value["patches"][0]["value"];
        assert!(patch_value.is_array());
        assert_eq!(patch_value.as_array().unwrap().len(), 1);
        assert!(
            patch_value[0].is_object(),
            "value[0] must be the object: {patch_value}"
        );
        assert_eq!(patch_value[0]["currency"], json!("USD"));
        assert_eq!(
            patch_value[0]["our_price"][0]["schedule"][0]["value_with_tax"],
            json!("19.99")
        );
        assert!(!serde_json::to_string(&body).unwrap().contains("[[{"));
    }

    fn assert_single_wrapped(raw_request: &str) {
        let body: serde_json::Value =
            serde_json::from_str(request_body(raw_request)).expect("request body is JSON");
        let patch_value = &body["patches"][0]["value"];
        assert!(patch_value.is_array(), "value must be an array: {body}");
        assert!(
            patch_value[0].is_object(),
            "value[0] must be an object, not a nested array: {body}"
        );
    }

    #[tokio::test]
    async fn update_quantity_invalid_submission_surfaces_as_err() {
        let (base, captured) = spawn_fake_server(200, INVALID_BODY.to_string()).await;
        let client = AmazonClient::with_api_base(base);
        let err = client
            .update_quantity(
                "token",
                "seller-1",
                "SKU-1",
                &["ATVPDKIKX0DER".to_string()],
                5,
            )
            .await
            .expect_err("an INVALID quantity submission must surface as Err");
        assert!(err.to_string().contains("SKU-1"), "got: {err}");
        let requests = captured.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_single_wrapped(&requests[0]);
    }

    #[tokio::test]
    async fn update_quantity_error_issue_surfaces_as_err() {
        let (base, _captured) = spawn_fake_server(200, ERROR_ISSUE_BODY.to_string()).await;
        let client = AmazonClient::with_api_base(base);
        client
            .update_quantity(
                "token",
                "seller-1",
                "SKU-1",
                &["ATVPDKIKX0DER".to_string()],
                5,
            )
            .await
            .expect_err("an ERROR issue on a quantity update must surface as Err");
    }

    #[tokio::test]
    async fn update_quantity_accepted_submission_returns_ok() {
        let (base, _captured) = spawn_fake_server(200, ACCEPTED_BODY.to_string()).await;
        let client = AmazonClient::with_api_base(base);
        client
            .update_quantity(
                "token",
                "seller-1",
                "SKU-1",
                &["ATVPDKIKX0DER".to_string()],
                5,
            )
            .await
            .expect("an ACCEPTED quantity submission must be Ok");
    }

    #[tokio::test]
    async fn update_price_invalid_submission_surfaces_as_err() {
        let (base, captured) = spawn_fake_server(200, INVALID_BODY.to_string()).await;
        let client = AmazonClient::with_api_base(base);
        let err = client
            .update_price(
                "token",
                "seller-1",
                "SKU-1",
                &["ATVPDKIKX0DER".to_string()],
                19.99,
                "USD",
            )
            .await
            .expect_err("an INVALID price submission must surface as Err");
        assert!(err.to_string().contains("SKU-1"), "got: {err}");
        let requests = captured.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_single_wrapped(&requests[0]);
    }

    #[tokio::test]
    async fn update_price_error_issue_surfaces_as_err() {
        let (base, _captured) = spawn_fake_server(200, ERROR_ISSUE_BODY.to_string()).await;
        let client = AmazonClient::with_api_base(base);
        client
            .update_price(
                "token",
                "seller-1",
                "SKU-1",
                &["ATVPDKIKX0DER".to_string()],
                19.99,
                "USD",
            )
            .await
            .expect_err("an ERROR issue on a price update must surface as Err");
    }

    #[tokio::test]
    async fn update_price_accepted_submission_returns_ok() {
        let (base, _captured) = spawn_fake_server(200, ACCEPTED_BODY.to_string()).await;
        let client = AmazonClient::with_api_base(base);
        client
            .update_price(
                "token",
                "seller-1",
                "SKU-1",
                &["ATVPDKIKX0DER".to_string()],
                19.99,
                "USD",
            )
            .await
            .expect("an ACCEPTED price submission must be Ok");
    }

    #[tokio::test]
    async fn create_listing_invalid_submission_surfaces_as_err() {
        let (base, _captured) = spawn_fake_server(200, INVALID_BODY.to_string()).await;
        let client = AmazonClient::with_api_base(base);
        let err = client
            .create_listing(
                "token",
                "seller-1",
                "SKU-1",
                &["ATVPDKIKX0DER".to_string()],
                "PRODUCT",
                json!({"item_name": [{"value": "Widget"}]}),
            )
            .await
            .expect_err("an INVALID create-listing submission must surface as Err");
        assert!(err.to_string().contains("SKU-1"), "got: {err}");
    }

    #[tokio::test]
    async fn create_listing_error_issue_surfaces_as_err() {
        let (base, _captured) = spawn_fake_server(200, ERROR_ISSUE_BODY.to_string()).await;
        let client = AmazonClient::with_api_base(base);
        client
            .create_listing(
                "token",
                "seller-1",
                "SKU-1",
                &["ATVPDKIKX0DER".to_string()],
                "PRODUCT",
                json!({"item_name": [{"value": "Widget"}]}),
            )
            .await
            .expect_err("an ERROR issue on create-listing must surface as Err");
    }

    #[tokio::test]
    async fn create_listing_accepted_submission_returns_ok() {
        let (base, _captured) = spawn_fake_server(200, ACCEPTED_BODY.to_string()).await;
        let client = AmazonClient::with_api_base(base);
        let submission = client
            .create_listing(
                "token",
                "seller-1",
                "SKU-1",
                &["ATVPDKIKX0DER".to_string()],
                "PRODUCT",
                json!({"item_name": [{"value": "Widget"}]}),
            )
            .await
            .expect("an ACCEPTED create-listing submission must be Ok");
        assert_eq!(submission.status.as_deref(), Some("ACCEPTED"));
    }
}
