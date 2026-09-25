//! Golden-file tests for `parse_sales_page`.
//!
//! This parser is what `detect_sales()` returns, and the sync engine turns each
//! record into a negative quantity delta against the mapped product. A row read
//! wrongly here moves real stock: a declined order counted as a sale takes a
//! unit off the shelf that was never sold, and a missed sale oversells.

use nisaba_xmrbazaar::scraper::parse_sales_page;

const SALES_PAGE: &str = include_str!("fixtures/my-sales.html");

#[test]
fn only_successful_orders_are_reported_as_sales() {
    let sales = parse_sales_page(SALES_PAGE, "https://xmrbazaar.com");

    let order_ids: Vec<&str> = sales.iter().map(|s| s.order_id.as_str()).collect();
    assert_eq!(
        order_ids,
        vec!["436a", "91bc"],
        "only the completed and shipped orders are sales"
    );
}

#[test]
fn a_declined_order_is_not_a_sale() {
    let sales = parse_sales_page(SALES_PAGE, "https://xmrbazaar.com");
    assert!(
        !sales.iter().any(|s| s.order_id == "ccc1"),
        "a declined order must never decrement stock"
    );
}

#[test]
fn an_unpaid_order_is_not_a_sale() {
    let sales = parse_sales_page(SALES_PAGE, "https://xmrbazaar.com");
    assert!(!sales.iter().any(|s| s.order_id == "dd02"));
}

#[test]
fn a_row_without_an_order_link_is_skipped_not_fatal() {
    let sales = parse_sales_page(SALES_PAGE, "https://xmrbazaar.com");
    assert!(!sales.iter().any(|s| s.title == "Orphaned Row"));
}

#[test]
fn a_completed_order_with_no_listing_link_is_skipped() {
    // Without a listing id the engine has nothing to map the sale onto, so
    // reporting it would only produce an "unmapped listing" warning per cycle.
    let sales = parse_sales_page(SALES_PAGE, "https://xmrbazaar.com");
    assert!(!sales.iter().any(|s| s.order_id == "ee03"));
}

#[test]
fn ids_are_taken_from_the_trailing_path_segment() {
    let sales = parse_sales_page(SALES_PAGE, "https://xmrbazaar.com");
    let first = &sales[0];
    assert_eq!(first.order_id, "436a");
    assert_eq!(first.listing_id, "FxHA");
    assert_eq!(first.title, "Ripstop Field Pants");
    assert_eq!(first.status, "completed");
    assert_eq!(first.completed_at.as_deref(), Some("2026-09-21"));
}

#[test]
fn every_sale_is_one_unit() {
    // XMR Bazaar's sales table carries no quantity column, so the parser
    // reports one unit per order. If the site ever adds one, this is the test
    // that should start failing.
    let sales = parse_sales_page(SALES_PAGE, "https://xmrbazaar.com");
    assert!(!sales.is_empty());
    assert!(sales.iter().all(|s| s.quantity == 1));
}

#[test]
fn a_page_with_no_sales_table_yields_nothing() {
    let empty = r#"<html><body><p>You have no sales yet.</p></body></html>"#;
    assert!(parse_sales_page(empty, "https://xmrbazaar.com").is_empty());
}

#[test]
fn a_login_wall_yields_nothing_rather_than_phantom_sales() {
    // Session expiry is the realistic failure here: the scraper gets the login
    // page instead of the sales table. Reporting zero sales is correct; the
    // dangerous outcome would be reporting garbage rows.
    let login = r#"<html><body><form action="/login/">
        <input name="username"><input name="password"></form></body></html>"#;
    assert!(parse_sales_page(login, "https://xmrbazaar.com").is_empty());
}
