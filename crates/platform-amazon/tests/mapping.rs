//! Fixture-based tests for the Amazon mapping layer.
//!
//! The SP-API Listings Items resource returns arrays where Nisaba wants a
//! single value — a listing can be stocked on several fulfilment channels and
//! offered in several marketplaces at once — so most of what `mapping.rs` does
//! is choose. These tests pin the corrected choices: quantity is summed across
//! channels, price is taken from the configured marketplace (keeping its
//! currency), pagination follows the documented nested `pagination` shape, and
//! ERROR-severity issues are carried through `extras`.
//!
//! The fixture is the response shape with invented data: no seller, token,
//! real ASIN or order.

use nisaba_amazon::mapping::{
    to_full_listing, to_full_listing_for_marketplaces, to_inventory_item, to_listing,
    to_listing_for_marketplaces,
};
use nisaba_amazon::types::{AmazonListingItem, AmazonListingsResponse};

const US_MARKETPLACE: &str = "ATVPDKIKX0DER";
const CA_MARKETPLACE: &str = "A2EUQ1WTGCTBG2";

fn response() -> AmazonListingsResponse {
    serde_json::from_str(include_str!("fixtures/listings_items.json"))
        .expect("listings fixture parses")
}

fn items() -> Vec<AmazonListingItem> {
    response().items
}

fn item(sku: &str) -> AmazonListingItem {
    items()
        .into_iter()
        .find(|i| i.sku == sku)
        .unwrap_or_else(|| panic!("no fixture item with sku {sku}"))
}

#[test]
fn the_listings_response_deserializes() {
    let items = items();
    assert_eq!(items.len(), 4);
    assert_eq!(items[0].sku, "PANTS-RIPSTOP-M");
}

#[test]
fn pagination_next_token_is_read_from_the_nested_pagination_object() {
    // The documented `searchListingsItems` shape nests the token under
    // `pagination`; there is no top-level `nextToken`. The fixture encodes
    // exactly that shape, and the token must be recovered from it — reading a
    // top-level key would yield `None` and silently stop after page one.
    let resp = response();
    let pagination = resp
        .pagination
        .as_ref()
        .expect("fixture carries a pagination block");
    assert_eq!(
        pagination.next_token.as_deref(),
        Some("fixture-page-2-token")
    );
    assert_eq!(
        resp.effective_next_token().as_deref(),
        Some("fixture-page-2-token")
    );
    assert!(
        resp.next_token.is_none(),
        "no top-level nextToken in the fixture"
    );

    // A last page carries a pagination block with no next token.
    let last: AmazonListingsResponse =
        serde_json::from_str(r#"{"items":[],"pagination":{"previousToken":"p1"}}"#)
            .expect("last-page shape parses");
    assert_eq!(last.effective_next_token(), None);
}

#[test]
fn a_single_channel_listing_maps_its_quantity_straight_through() {
    let mapped = to_inventory_item(&item("PANTS-RIPSTOP-M"));
    assert_eq!(mapped.platform_item_id, "PANTS-RIPSTOP-M");
    assert_eq!(mapped.quantity, 7);
}

#[test]
fn quantities_sum_across_fulfilment_channels() {
    // Replaces `only_the_first_fulfilment_channel_is_counted`. The wool cap is
    // stocked on two channels — 2 merchant-fulfilled and 11 FBA — and the
    // total stock is 13, regardless of which channel Amazon lists first.
    let cap = item("CAP-WOOL");
    assert_eq!(to_inventory_item(&cap).quantity, 13);
    assert_eq!(to_listing(&cap).quantity, 13);
    assert_eq!(to_full_listing(&cap).quantity, 13);

    let mut reversed = cap.clone();
    reversed.fulfillment_availability.reverse();
    assert_eq!(
        to_inventory_item(&reversed).quantity,
        13,
        "summation is order-independent"
    );
}

#[test]
fn a_listing_with_no_fulfilment_availability_reports_zero() {
    assert_eq!(to_inventory_item(&item("BOOTS-SUPPRESSED")).quantity, 0);
    assert_eq!(to_inventory_item(&item("DRAFT-ONLY")).quantity, 0);
}

#[test]
fn the_sku_is_the_platform_item_id() {
    for sku in [
        "PANTS-RIPSTOP-M",
        "CAP-WOOL",
        "BOOTS-SUPPRESSED",
        "DRAFT-ONLY",
    ] {
        assert_eq!(to_inventory_item(&item(sku)).platform_item_id, sku);
        assert_eq!(to_listing(&item(sku)).platform_item_id, sku);
        assert_eq!(to_full_listing(&item(sku)).platform_item_id, sku);
    }
}

#[test]
fn a_listing_carries_the_summary_title_price_and_main_image() {
    let pants = to_listing(&item("PANTS-RIPSTOP-M"));
    assert_eq!(pants.title, "Ripstop Field Pants — Medium");
    assert_eq!(pants.sku.as_deref(), Some("PANTS-RIPSTOP-M"));
    assert_eq!(pants.price, Some(rust_decimal::Decimal::new(4800, 2)));
    assert_eq!(
        pants.image_url.as_deref(),
        Some("https://images.example.test/pants-main.jpg")
    );
}

#[test]
fn price_comes_from_the_configured_marketplace_and_keeps_its_currency() {
    // Replaces `only_the_first_marketplace_offer_sets_the_price`. The cap is
    // offered at USD 24.00 in the US and CAD 32.50 in Canada; the configured
    // marketplace decides which offer (and currency) is reported.
    let cap = item("CAP-WOOL");

    let ca = to_full_listing_for_marketplaces(&cap, &[CA_MARKETPLACE.to_string()]);
    let ca_price = ca.price.expect("a CA price");
    assert_eq!(ca_price.amount, rust_decimal::Decimal::new(3250, 2));
    assert_eq!(ca_price.currency, "CAD");
    assert_eq!(
        to_listing_for_marketplaces(&cap, &[CA_MARKETPLACE.to_string()]).price,
        Some(rust_decimal::Decimal::new(3250, 2))
    );

    let us = to_full_listing_for_marketplaces(&cap, &[US_MARKETPLACE.to_string()]);
    let us_price = us.price.expect("a US price");
    assert_eq!(us_price.amount, rust_decimal::Decimal::new(2400, 2));
    assert_eq!(us_price.currency, "USD");

    // Order of the returned offers must not change the selection.
    let mut reversed = cap.clone();
    reversed.offers.reverse();
    let ca_reversed = to_full_listing_for_marketplaces(&reversed, &[CA_MARKETPLACE.to_string()]);
    assert_eq!(
        ca_reversed.price.expect("a CA price").amount,
        rust_decimal::Decimal::new(3250, 2)
    );

    // Configured order wins when several configured marketplaces have offers.
    let both = to_full_listing_for_marketplaces(
        &cap,
        &[CA_MARKETPLACE.to_string(), US_MARKETPLACE.to_string()],
    );
    assert_eq!(both.price.expect("a price").currency, "CAD");

    // A configured marketplace with no offer yields no price, rather than
    // another marketplace's price under the wrong currency.
    let unknown = to_full_listing_for_marketplaces(&cap, &["A1F83G8C2ARO7P".to_string()]);
    assert!(unknown.price.is_none());
    assert!(
        to_listing_for_marketplaces(&cap, &["A1F83G8C2ARO7P".to_string()])
            .price
            .is_none()
    );
}

#[test]
fn a_listing_with_no_summary_falls_back_to_its_sku_for_a_title() {
    let draft = to_listing(&item("DRAFT-ONLY"));
    assert_eq!(draft.title, "DRAFT-ONLY");
    assert!(draft.image_url.is_none());
    assert!(draft.price.is_none());
}

#[test]
fn a_full_listing_links_to_the_asin() {
    let full = to_full_listing(&item("PANTS-RIPSTOP-M"));
    assert_eq!(
        full.url.as_deref(),
        Some("https://www.amazon.com/dp/B000EXAMPLE1")
    );
}

#[test]
fn a_full_listing_with_no_asin_has_no_url() {
    assert!(to_full_listing(&item("DRAFT-ONLY")).url.is_none());
}

#[test]
fn a_full_listing_carries_the_main_image_as_its_only_photo() {
    let full = to_full_listing(&item("PANTS-RIPSTOP-M"));
    assert_eq!(full.photos.len(), 1);
    assert_eq!(full.photos[0].position, 0);
    assert_eq!(
        full.photos[0].url,
        "https://images.example.test/pants-main.jpg"
    );

    assert!(
        to_full_listing(&item("BOOTS-SUPPRESSED")).photos.is_empty(),
        "a null mainImage yields no photos rather than an empty-url photo"
    );
}

#[test]
fn amazon_full_listings_never_carry_a_description() {
    for sku in ["PANTS-RIPSTOP-M", "CAP-WOOL"] {
        assert!(to_full_listing(&item(sku)).description.is_none());
    }
}

#[test]
fn error_issues_are_carried_through_extras() {
    // Replaces `listing_issues_are_not_surfaced_anywhere_in_the_mapping`. The
    // suppressed boots carry an ERROR-severity issue explaining why they are
    // not selling; it must be visible in the full listing's `extras` so the UI
    // does not show a normal listing with zero stock and no reason.
    let raw = item("BOOTS-SUPPRESSED");
    assert_eq!(raw.issues.len(), 1);
    assert_eq!(raw.issues[0].severity.as_deref(), Some("ERROR"));

    let full = to_full_listing(&raw);
    assert_eq!(
        full.extras
            .get("amazon_error_issue_count")
            .map(String::as_str),
        Some("1")
    );
    let joined = full
        .extras
        .get("amazon_error_issues")
        .expect("ERROR issues are carried through extras");
    assert!(joined.contains("90220"), "issue code is visible: {joined}");
    assert!(
        joined.contains("country_of_origin"),
        "issue message is visible: {joined}"
    );
    assert_eq!(
        full.extras
            .get("amazon_error_issue_0_code")
            .map(String::as_str),
        Some("90220")
    );

    // Listings without ERROR issues carry no error extras.
    assert!(to_full_listing(&item("PANTS-RIPSTOP-M")).extras.is_empty());
    assert!(to_full_listing(&item("CAP-WOOL")).extras.is_empty());
}
