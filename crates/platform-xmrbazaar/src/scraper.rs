use scraper::{ElementRef, Html, Selector};
use tracing::debug;

use crate::types::{EditFormData, StockModeField, XmrListing, XmrListingDetail, XmrSaleRecord};

/// Strip HTML tags from a string, returning plain text that preserves
/// block structure (paragraphs, line breaks, list items).
/// XMR Bazaar description fields are plain text — this converts HTML descriptions
/// (e.g. from eBay/Squarespace) into readable plain text for form submission.
pub fn strip_html_tags(html: &str) -> String {
    let fragment = Html::parse_fragment(html);
    let mut output = String::new();
    collect_plain_text(fragment.root_element(), &mut output);
    // Collapse runs of 3+ newlines into 2
    while output.contains("\n\n\n") {
        output = output.replace("\n\n\n", "\n\n");
    }
    output.trim().to_string()
}

/// Recursively walk the element tree, inserting newlines for block elements
/// and bullet markers for list items.
fn collect_plain_text(element: ElementRef, output: &mut String) {
    let block_tags = [
        "p",
        "div",
        "br",
        "hr",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "ul",
        "ol",
        "blockquote",
        "section",
        "article",
        "tr",
        "header",
        "footer",
    ];

    for child in element.children() {
        if let Some(text) = child.value().as_text() {
            output.push_str(text);
        } else if let Some(child_el) = ElementRef::wrap(child) {
            let tag = child_el.value().name();
            let is_block = block_tags.contains(&tag);
            let is_li = tag == "li";

            if (is_block || is_li) && !output.is_empty() && !output.ends_with('\n') {
                output.push('\n');
            }
            if is_li {
                output.push_str("· ");
            }

            collect_plain_text(child_el, output);

            if is_block && !output.ends_with('\n') {
                output.push('\n');
            }
        }
    }
}

/// Parse the "my listings" page to extract listing data.
///
/// The page uses a table with `tr.my-sales-tr` rows (same CSS classes as
/// the sales page). Each row has:
/// - `td.my-sales-td-guid a` — listing link (href `/listing/{id}/`)
/// - `td.my-sales-td-status .my-sales-status-tag` — status text (Active, etc.)
/// - `.my-sales-product-title` — product title
/// - `td.my-sales-td-price .my-sales-price` — price text
/// - `.my-sales-product-img` — background-image CSS with thumbnail URL
pub fn parse_listings(html: &str, base_url: &str) -> Vec<XmrListing> {
    let document = Html::parse_document(html);
    let mut listings = Vec::new();

    let row_sel = Selector::parse("tr.my-sales-tr").unwrap();

    for row in document.select(&row_sel) {
        // Extract listing ID from the guid cell link: /listing/44DV/ → 44DV
        let id = Selector::parse("td.my-sales-td-guid a")
            .ok()
            .and_then(|sel| row.select(&sel).next())
            .and_then(|a| {
                let href = a.value().attr("href")?;
                Some(
                    href.trim_matches('/')
                        .rsplit('/')
                        .next()
                        .unwrap_or("")
                        .to_string(),
                )
            })
            .unwrap_or_default();

        if id.is_empty() {
            continue;
        }

        // Extract title from .my-sales-product-title
        let title = Selector::parse(".my-sales-product-title")
            .ok()
            .and_then(|sel| row.select(&sel).next())
            .map(|el| el.text().collect::<String>().trim().to_string())
            .unwrap_or_else(|| format!("Listing {}", id));

        // Extract price from .my-sales-price (e.g. "$250.00")
        let price = Selector::parse("td.my-sales-td-price .my-sales-price")
            .ok()
            .and_then(|sel| row.select(&sel).next())
            .and_then(|el| {
                let text = el.text().collect::<String>();
                text.trim()
                    .replace(['$', ',', '€', '£'], "")
                    .trim()
                    .parse::<f64>()
                    .ok()
            });

        // Extract image URL from .my-sales-product-img background-image style
        let image_url = Selector::parse(".my-sales-product-img")
            .ok()
            .and_then(|sel| row.select(&sel).next())
            .and_then(|el| {
                let style = el.value().attr("style")?;
                // Parse: background-image:url(https://...thumb.jpeg)
                let start = style.find("url(")? + 4;
                let end = style[start..].find(')')? + start;
                Some(style[start..end].to_string())
            });

        // XMR Bazaar uses stock modes, not numeric quantity.
        // Check if the status indicates the listing is active.
        let status = Selector::parse("td.my-sales-td-status .my-sales-status-tag")
            .ok()
            .and_then(|sel| row.select(&sel).next())
            .map(|el| el.text().collect::<String>().trim().to_lowercase())
            .unwrap_or_default();

        // Active listings get quantity 1 (in stock), inactive get 0
        let quantity = if status.contains("active") { 1 } else { 0 };

        let photos = image_url.iter().cloned().collect();

        let edit_url = format!("{}/edit-listing/{}/", base_url.trim_end_matches('/'), id);

        listings.push(XmrListing {
            id,
            title,
            price,
            quantity,
            image_url,
            edit_url,
            description: None,
            photos,
        });
    }

    debug!(
        count = listings.len(),
        "Parsed XMR Bazaar listings from HTML"
    );
    listings
}

/// The `<form>` an element belongs to, if any.
fn enclosing_form(el: ElementRef) -> Option<ElementRef> {
    el.ancestors()
        .filter_map(ElementRef::wrap)
        .find(|a| a.value().name() == "form")
}

/// Whether a form control would actually be submitted by a browser.
///
/// The adapter re-posts the entire edit form to change one field, so anything
/// kept here is written back to a live listing. HTML only submits "successful
/// controls": a disabled control never is, an unchecked checkbox or radio never
/// is, and only the submit button the user activated is. File inputs are
/// successful in a browser, but they cannot be expressed in the
/// `application/x-www-form-urlencoded` body this adapter sends — posting the
/// name with an empty value asks the server to clear the photo — so they are
/// dropped too.
fn is_successful_control(el: ElementRef) -> bool {
    let element = el.value();

    if element.attr("disabled").is_some() {
        return false;
    }

    if element.name() != "input" {
        // <select> and <textarea> are always submitted when named and enabled.
        return true;
    }

    let input_type = element.attr("type").unwrap_or("text").to_ascii_lowercase();

    match input_type.as_str() {
        "checkbox" | "radio" => element.attr("checked").is_some(),
        "file" | "submit" | "reset" | "button" | "image" => false,
        _ => true,
    }
}

/// Parse a listing edit form to extract form token and all form fields.
/// Also extracts textarea content for fields like description.
/// Works with both Django-style (csrfmiddlewaretoken) and PHP-style (validation) tokens.
pub fn parse_edit_form(html: &str) -> Option<EditFormData> {
    let document = Html::parse_document(html);

    // Find form token — try multiple field names
    let token_names = ["validation", "csrfmiddlewaretoken", "_csrf", "csrf_token"];
    let mut token_input = None;
    let mut csrf_field_name = String::new();

    for name in &token_names {
        let selector_str = format!("input[name='{}']", name);
        let found = Selector::parse(&selector_str).ok().and_then(|sel| {
            document
                .select(&sel)
                .find(|el| el.value().attr("value").is_some_and(|v| !v.is_empty()))
        });
        if let Some(el) = found {
            token_input = Some(el);
            csrf_field_name = name.to_string();
            break;
        }
    }

    let token_input = token_input?;
    let csrf_token = token_input.value().attr("value")?.to_string();

    // Collect the fields of the form the token belongs to. A listing page also
    // carries site search, login and newsletter forms; sweeping the whole
    // document would post their fields back at the listing.
    let field_sel = Selector::parse("input, select, textarea").ok()?;
    let form_root = enclosing_form(token_input).unwrap_or_else(|| document.root_element());
    let mut fields = Vec::new();

    for el in form_root.select(&field_sel) {
        let name = match el.value().attr("name") {
            Some(n) => n.to_string(),
            None => continue,
        };

        // Skip the token field — we track it separately
        if name == csrf_field_name {
            continue;
        }

        // Everything we collect here is posted straight back at the listing, so
        // a control we keep that a browser would have dropped is a silent edit.
        // Mirror the HTML definition of a "successful control".
        if !is_successful_control(el) {
            continue;
        }

        // For textareas, get inner text; for selects, get the selected option's value;
        // for everything else, use the value attribute.
        let value = if el.value().name() == "textarea" {
            el.text().collect::<String>()
        } else if el.value().name() == "select" {
            // Find the <option selected> child, or fall back to the first <option> with a value
            let option_sel = Selector::parse("option[selected]")
                .or_else(|_| Selector::parse("option"))
                .ok();
            option_sel
                .and_then(|sel| {
                    el.select(&sel)
                        .next()
                        .and_then(|opt| opt.value().attr("value").map(|v| v.to_string()))
                })
                .or_else(|| {
                    // No selected option found; try first option with a non-empty value
                    Selector::parse("option").ok().and_then(|sel| {
                        el.select(&sel).find_map(|opt| {
                            opt.value()
                                .attr("value")
                                .filter(|v| !v.is_empty())
                                .map(|v| v.to_string())
                        })
                    })
                })
                .unwrap_or_default()
        } else {
            el.value().attr("value").unwrap_or("").to_string()
        };

        fields.push((name, value));
    }

    debug!(
        token_field = %csrf_field_name,
        token_len = csrf_token.len(),
        field_count = fields.len(),
        "Parsed edit form"
    );

    Some(EditFormData {
        csrf_token,
        csrf_field_name,
        fields,
    })
}

/// Extract description text from form fields, looking for common field names.
pub fn extract_description_from_form(form_data: &EditFormData) -> Option<String> {
    let description_field_names = [
        "description",
        "body",
        "content",
        "desc",
        "listing_description",
    ];

    for (name, value) in &form_data.fields {
        let name_lower = name.to_lowercase();
        for &field_name in &description_field_names {
            if name_lower == field_name || name_lower.contains(field_name) {
                let trimmed = value.trim().to_string();
                if !trimmed.is_empty() {
                    return Some(trimmed);
                }
            }
        }
    }

    None
}

/// Parse a listing's edit page to extract detailed information including
/// description, photos, and price.
pub fn parse_listing_detail(html: &str, base_url: &str) -> Option<XmrListingDetail> {
    let document = Html::parse_document(html);

    // Extract description from textarea fields
    let description = extract_description_from_textarea(&document);

    // Extract all photos (images in the form / page)
    let photos = extract_photos_from_page(&document, base_url);

    // Extract price and currency from form input fields
    let price = extract_price_from_form(&document);
    let currency = extract_currency_from_form(&document);

    Some(XmrListingDetail {
        description,
        photos,
        price,
        currency,
    })
}

/// Extract description text from textarea elements in the document.
fn extract_description_from_textarea(document: &Html) -> Option<String> {
    let description_names = [
        "textarea[name='description']",
        "textarea[name='body']",
        "textarea[name='content']",
        "textarea[name='desc']",
        "textarea[name='listing_description']",
    ];

    for selector_str in &description_names {
        if let Ok(sel) = Selector::parse(selector_str) {
            if let Some(el) = document.select(&sel).next() {
                let text = el.text().collect::<String>();
                let trimmed = text.trim().to_string();
                if !trimmed.is_empty() {
                    return Some(trimmed);
                }
            }
        }
    }

    // Fallback: look for any textarea that might contain a description
    if let Ok(sel) = Selector::parse("form textarea") {
        for el in document.select(&sel) {
            let name = el.value().attr("name").unwrap_or("");
            let name_lower = name.to_lowercase();
            // Skip CSRF or other non-description textareas
            if name_lower.contains("csrf") || name_lower.contains("token") {
                continue;
            }
            let text = el.text().collect::<String>();
            let trimmed = text.trim().to_string();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }
    }

    None
}

/// Extract photo URLs from img tags in the page.
fn extract_photos_from_page(document: &Html, base_url: &str) -> Vec<String> {
    let mut photos = Vec::new();

    // Look for images inside the form or content area
    let selectors = [
        "form img[src]",
        ".listing-images img[src]",
        ".product-images img[src]",
        ".gallery img[src]",
        ".photos img[src]",
    ];

    for selector_str in &selectors {
        if let Ok(sel) = Selector::parse(selector_str) {
            for el in document.select(&sel) {
                if let Some(src) = el.value().attr("src") {
                    let url = normalize_url(src, base_url);
                    if !photos.contains(&url) {
                        photos.push(url);
                    }
                }
            }
        }
        // If we found photos with a specific selector, use those
        if !photos.is_empty() {
            break;
        }
    }

    // If no photos found with specific selectors, try all content images
    // but exclude common non-product images (icons, logos, avatars)
    if photos.is_empty() {
        if let Ok(sel) = Selector::parse("img[src]") {
            for el in document.select(&sel) {
                if let Some(src) = el.value().attr("src") {
                    let src_lower = src.to_lowercase();
                    // Skip common non-product images
                    if src_lower.contains("icon")
                        || src_lower.contains("logo")
                        || src_lower.contains("avatar")
                        || src_lower.contains("favicon")
                        || src_lower.contains("spinner")
                        || src_lower.contains("loading")
                    {
                        continue;
                    }
                    let url = normalize_url(src, base_url);
                    if !photos.contains(&url) {
                        photos.push(url);
                    }
                }
            }
        }
    }

    photos
}

/// Extract price from form input fields.
fn extract_price_from_form(document: &Html) -> Option<f64> {
    let price_selectors = [
        "input[name='price']",
        "input[name='amount']",
        "input[name='listing_price']",
        "input[name='cost']",
    ];

    for selector_str in &price_selectors {
        if let Ok(sel) = Selector::parse(selector_str) {
            if let Some(el) = document.select(&sel).next() {
                if let Some(value) = el.value().attr("value") {
                    let cleaned = value.trim().replace(['$', ',', '€', '£'], "");
                    if let Ok(price) = cleaned.trim().parse::<f64>() {
                        return Some(price);
                    }
                }
            }
        }
    }

    None
}

/// Extract the selected currency from a `<select name="currency">` element.
fn extract_currency_from_form(document: &Html) -> Option<String> {
    let sel = Selector::parse("select[name='currency']").ok()?;
    let select_el = document.select(&sel).next()?;

    // Look for the <option> with "selected" attribute
    let option_sel = Selector::parse("option[selected]").ok()?;
    if let Some(opt) = select_el.select(&option_sel).next() {
        if let Some(val) = opt.value().attr("value") {
            let v = val.trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }

    None
}

/// Normalize a potentially relative URL to an absolute URL.
fn normalize_url(src: &str, base_url: &str) -> String {
    if src.starts_with("http://") || src.starts_with("https://") {
        src.to_string()
    } else if src.starts_with("//") {
        format!("https:{}", src)
    } else {
        format!(
            "{}/{}",
            base_url.trim_end_matches('/'),
            src.trim_start_matches('/')
        )
    }
}

/// Parsed login form structure with discovered field names.
#[derive(Debug)]
pub struct LoginForm {
    pub token_field_name: String,
    pub token_value: String,
    pub username_field_name: String,
    pub password_field_name: String,
    /// All hidden fields (action, validation token, etc.) to include in POST.
    pub hidden_fields: Vec<(String, String)>,
}

/// Parse the login page to discover actual form field names.
/// Returns the token, username field name, password field name, and all hidden fields.
pub fn parse_login_form(html: &str) -> Option<LoginForm> {
    let document = Html::parse_document(html);

    // Find the login form
    let form_sel = Selector::parse("form").ok()?;
    let input_sel = Selector::parse("input").ok()?;

    // Find the form that contains a password field (that's the login form)
    let form = document.select(&form_sel).find(|f| {
        let inner = Html::parse_fragment(&f.html());
        Selector::parse("input[type='password']")
            .ok()
            .map(|sel| inner.select(&sel).next().is_some())
            .unwrap_or(false)
    })?;

    let mut hidden_fields = Vec::new();
    let mut username_field_name = String::new();
    let mut password_field_name = String::new();
    let mut token_field_name = String::new();
    let mut token_value = String::new();

    for el in form.select(&input_sel) {
        let name = el.value().attr("name").unwrap_or("").to_string();
        let input_type = el.value().attr("type").unwrap_or("text").to_lowercase();
        let value = el.value().attr("value").unwrap_or("").to_string();

        if name.is_empty() {
            continue;
        }

        match input_type.as_str() {
            "password" => {
                password_field_name = name;
            }
            "hidden" => {
                // Detect the CSRF/validation token (long hidden value)
                if token_field_name.is_empty() && value.len() > 40 {
                    token_field_name = name.clone();
                    token_value = value.clone();
                }
                hidden_fields.push((name, value));
            }
            "submit" => {
                // Skip submit buttons
            }
            _ => {
                // Text/email fields — likely the username field
                if username_field_name.is_empty() {
                    username_field_name = name;
                }
            }
        }
    }

    if password_field_name.is_empty() {
        return None;
    }

    // Default username field if we didn't find one
    if username_field_name.is_empty() {
        username_field_name = "username".to_string();
    }

    debug!(
        token_field = %token_field_name,
        username_field = %username_field_name,
        password_field = %password_field_name,
        hidden_count = hidden_fields.len(),
        hidden_names = ?hidden_fields.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
        "Discovered login form fields"
    );

    Some(LoginForm {
        token_field_name,
        token_value,
        username_field_name,
        password_field_name,
        hidden_fields,
    })
}

/// Extract the validation token from the login page.
/// XMR Bazaar uses a hidden "validation" field (PHP-based site).
pub fn extract_validation_token(html: &str) -> Option<String> {
    let document = Html::parse_document(html);

    // Primary: hidden input named "validation"
    if let Ok(sel) = Selector::parse("input[name='validation']") {
        if let Some(el) = document.select(&sel).next() {
            if let Some(val) = el.value().attr("value") {
                if !val.is_empty() {
                    return Some(val.to_string());
                }
            }
        }
    }

    // Fallback: any hidden input with a long base64-ish value (anti-CSRF token)
    if let Ok(sel) = Selector::parse("input[type='hidden']") {
        for el in document.select(&sel) {
            let name = el.value().attr("name").unwrap_or("");
            // Skip known non-token fields
            if name == "action" || name == "username" || name == "password" {
                continue;
            }
            if let Some(val) = el.value().attr("value") {
                // Validation tokens are typically long base64 strings
                if val.len() > 40 {
                    debug!(
                        field_name = name,
                        value_len = val.len(),
                        "Found candidate validation token"
                    );
                    return Some(val.to_string());
                }
            }
        }
    }

    None
}

/// Extract the form-level validation/CSRF token from an edit form.
/// Checks for "validation", "csrfmiddlewaretoken", "_csrf", and "csrf_token" fields.
pub fn extract_form_token(html: &str) -> Option<(String, String)> {
    let document = Html::parse_document(html);
    let token_names = ["validation", "csrfmiddlewaretoken", "_csrf", "csrf_token"];

    for name in &token_names {
        let selector_str = format!("input[name='{}']", name);
        let found = Selector::parse(&selector_str).ok().and_then(|sel| {
            document
                .select(&sel)
                .next()
                .and_then(|el| el.value().attr("value"))
                .filter(|v| !v.is_empty())
                .map(|v| (name.to_string(), v.to_string()))
        });
        if found.is_some() {
            return found;
        }
    }

    None
}

/// Parse the "my sales" page to extract sale records.
/// The page uses a table with `tr.my-sales-tr` rows. Each row has:
/// - `td.my-sales-td-guid a` — order link (href `/order/{id}/`)
/// - `td.my-sales-td-status .my-sales-status-tag` — status text
/// - `td.my-sales-td-product a[href*='listing']` — listing link (href `/listing/{id}/`)
/// - `.my-sales-product-title` — product title
/// - `.my-sales-date` — date text
///
/// Only returns sales with a successful status — orders marked "Declined" or
/// similar non-success statuses are filtered out.
pub fn parse_sales_page(html: &str, _base_url: &str) -> Vec<XmrSaleRecord> {
    let document = Html::parse_document(html);
    let mut sales = Vec::new();

    let row_sel = Selector::parse("tr.my-sales-tr").unwrap();

    for row in document.select(&row_sel) {
        // Extract order ID from the guid cell link: /order/436a/ → 436a
        let order_id = Selector::parse("td.my-sales-td-guid a")
            .ok()
            .and_then(|sel| row.select(&sel).next())
            .and_then(|a| {
                a.value().attr("href").map(|href| {
                    href.trim_matches('/')
                        .rsplit('/')
                        .next()
                        .unwrap_or("")
                        .to_string()
                })
            })
            .unwrap_or_default();

        if order_id.is_empty() {
            continue;
        }

        // Extract status from .my-sales-status-tag
        let status = Selector::parse("td.my-sales-td-status .my-sales-status-tag")
            .ok()
            .and_then(|sel| row.select(&sel).next())
            .map(|el| el.text().collect::<String>().trim().to_lowercase())
            .unwrap_or_default();

        // Filter out non-successful statuses early
        let success_statuses = [
            "completed",
            "paid",
            "confirmed",
            "delivered",
            "finalized",
            "shipped",
        ];
        if !success_statuses.iter().any(|s| status.contains(s)) {
            debug!(
                order_id = %order_id,
                status = %status,
                "Skipping non-successful sale"
            );
            continue;
        }

        // Extract listing ID from product cell link: /listing/FxHA/ → FxHA
        let listing_id = Selector::parse("td.my-sales-td-product a[href]")
            .ok()
            .and_then(|sel| {
                row.select(&sel).find_map(|a| {
                    let href = a.value().attr("href")?;
                    if href.contains("listing") {
                        Some(
                            href.trim_matches('/')
                                .rsplit('/')
                                .next()
                                .unwrap_or("")
                                .to_string(),
                        )
                    } else {
                        None
                    }
                })
            })
            .unwrap_or_default();

        if listing_id.is_empty() {
            continue;
        }

        // Extract title from .my-sales-product-title
        let title = Selector::parse(".my-sales-product-title")
            .ok()
            .and_then(|sel| row.select(&sel).next())
            .map(|el| el.text().collect::<String>().trim().to_string())
            .unwrap_or_else(|| format!("Order {}", order_id));

        // Extract date from .my-sales-date
        let completed_at = Selector::parse(".my-sales-date")
            .ok()
            .and_then(|sel| row.select(&sel).next())
            .map(|el| el.text().collect::<String>().trim().to_string());

        // XMR Bazaar doesn't show quantity in the table — default to 1
        let quantity = 1;

        sales.push(XmrSaleRecord {
            order_id,
            listing_id,
            title,
            quantity,
            status,
            completed_at,
        });
    }

    debug!(count = sales.len(), "Parsed XMR Bazaar sales page");
    sales
}

/// Examine edit form fields for stock-mode toggles.
/// Searches for field names containing stock/sale_type/listing_type/availability/type
/// keywords, and determines which values map to unlimited vs one-time.
pub fn detect_stock_mode_field(form_data: &EditFormData) -> Option<StockModeField> {
    let stock_keywords = ["stock", "sale_type", "listing_type", "availability", "type"];

    for (name, value) in &form_data.fields {
        let name_lower = name.to_lowercase();

        // Skip unrelated fields
        if name_lower == "csrf"
            || name_lower.contains("token")
            || name_lower.contains("description")
        {
            continue;
        }

        let matches_keyword = stock_keywords.iter().any(|kw| name_lower.contains(kw));
        if !matches_keyword {
            continue;
        }

        // Determine unlimited vs one-time values based on current value and field name
        let value_lower = value.to_lowercase();

        let (unlimited_val, onetime_val) = if value_lower.contains("unlimited")
            || value_lower.contains("always")
            || value_lower == "1"
        {
            (value.clone(), guess_opposite_value(value))
        } else if value_lower.contains("one-time")
            || value_lower.contains("onetime")
            || value_lower.contains("single")
            || value_lower == "0"
        {
            (guess_opposite_value(value), value.clone())
        } else {
            // Default assumption: "unlimited" and "one-time" as string values
            ("unlimited".to_string(), "one-time".to_string())
        };

        debug!(
            field_name = %name,
            current_value = %value,
            unlimited = %unlimited_val,
            onetime = %onetime_val,
            "Detected stock mode field"
        );

        return Some(StockModeField {
            field_name: name.clone(),
            unlimited_value: unlimited_val,
            onetime_value: onetime_val,
        });
    }

    None
}

/// Extract validation/error messages from a form page response.
/// Looks for common error patterns: `.error`, `.alert`, `.validation-error`,
/// `.form-error`, elements with `error` or `alert` in their class.
pub fn extract_form_errors(html_str: &str) -> Vec<String> {
    let document = Html::parse_document(html_str);
    let mut errors = Vec::new();

    let selectors = [
        ".error",
        ".alert",
        ".validation-error",
        ".form-error",
        ".error-message",
        ".field-error",
        "[class*='error']",
        "[class*='alert']",
    ];

    for selector_str in &selectors {
        if let Ok(sel) = Selector::parse(selector_str) {
            for el in document.select(&sel) {
                let text = el.text().collect::<String>();
                let trimmed = text.trim().to_string();
                if !trimmed.is_empty() && trimmed.len() < 500 && !errors.contains(&trimmed) {
                    errors.push(trimmed);
                }
            }
        }
        if !errors.is_empty() {
            break;
        }
    }

    errors
}

/// Best-guess for the opposite stock mode value.
fn guess_opposite_value(current: &str) -> String {
    match current.to_lowercase().as_str() {
        "unlimited" | "always" => "one-time".to_string(),
        "one-time" | "onetime" | "single" => "unlimited".to_string(),
        "1" => "0".to_string(),
        "0" => "1".to_string(),
        _ => "unlimited".to_string(),
    }
}
