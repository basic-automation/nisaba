//! Tests for `parse_listings`, the "my listings" page parser.
//!
//! The status tag is the only stock signal XMR Bazaar gives for a listing:
//! an active listing reads as quantity 1, everything else as 0. The parser
//! used to test `status.contains("active")`, which also matches
//! `"inactive"` — a deactivated listing read as in stock, so the sync engine
//! would reconcile other platforms against stock that does not exist.

use nisaba_xmrbazaar::scraper::parse_listings;

fn listings_page(status: &str) -> String {
    format!(
        r#"<!doctype html>
<html><body>
  <table class="my-sales-table"><tbody>
    <tr class="my-sales-tr">
      <td class="my-sales-td-guid"><a href="/listing/AbC1/">AbC1</a></td>
      <td class="my-sales-td-status"><span class="my-sales-status-tag">{status}</span></td>
      <td class="my-sales-td-product">
        <a href="/listing/AbC1/"><span class="my-sales-product-title">Widget</span></a>
      </td>
      <td class="my-sales-td-price"><span class="my-sales-price">$25.00</span></td>
    </tr>
  </tbody></table>
</body></html>"#
    )
}

#[test]
fn an_active_listing_is_in_stock() {
    let listings = parse_listings(&listings_page("Active"), "https://xmrbazaar.com");
    assert_eq!(listings.len(), 1);
    assert_eq!(listings[0].id, "AbC1");
    assert_eq!(listings[0].quantity, 1);
}

#[test]
fn an_inactive_listing_is_not_in_stock() {
    // Regression: "inactive".contains("active") is true, so this listing
    // used to parse with quantity 1.
    let listings = parse_listings(&listings_page("Inactive"), "https://xmrbazaar.com");
    assert_eq!(listings.len(), 1);
    assert_eq!(
        listings[0].quantity, 0,
        "an inactive listing must read as out of stock"
    );
}

#[test]
fn other_statuses_are_not_in_stock() {
    for status in ["Out of stock", "Draft", "Sold", ""] {
        let listings = parse_listings(&listings_page(status), "https://xmrbazaar.com");
        assert_eq!(listings.len(), 1);
        assert_eq!(listings[0].quantity, 0, "status {status:?} is not active");
    }
}
