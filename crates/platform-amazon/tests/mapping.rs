//! Fixture-based tests for the Amazon mapping layer.
//!
//! The SP-API Listings Items resource returns arrays where Nisaba wants a
//! single value — a listing can be stocked on several fulfilment channels and
//! offered in several marketplaces at once — so most of what `mapping.rs` does
//! is choose. These tests pin down what it currently chooses, including two
//! places where "first element" is doing more work than it should.
//!
//! The fixture is the response shape with invented data: no seller, token,
//! real ASIN or order.

use nisaba_amazon::mapping::{to_full_listing, to_inventory_item, to_listing};
use nisaba_amazon::types::{AmazonListingItem, AmazonListingsResponse};

fn items() -> Vec<AmazonListingItem> {
    let response: AmazonListingsResponse =
        serde_json::from_str(include_str!("fixtures/listings_items.json"))
            .expect("listings fixture parses");
    response.items
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
fn a_single_channel_listing_maps_its_quantity_straight_through() {
    let mapped = to_inventory_item(&item("PANTS-RIPSTOP-M"));
    assert_eq!(mapped.platform_item_id, "PANTS-RIPSTOP-M");
    assert_eq!(mapped.quantity, 7);
}

#[test]
fn only_the_first_fulfilment_channel_is_counted() {
    // Documenting current behaviour, and it is a real gap. The wool cap is
    // stocked on two channels — 2 merchant-fulfilled and 11 FBA — and the
    // mapping reports 2, because it takes `fulfillmentAvailability[0]`. Amazon
    // does not guarantee that array's order, so which number Nisaba sees is not
    // even stable, and the sync engine resolves the minimum across platforms:
    // a listing with most of its stock in FBA will drag canonical stock down
    // and push that low number to eBay and Squarespace.
    let mapped = to_inventory_item(&item("CAP-WOOL"));
    assert_eq!(mapped.quantity, 2, "2 of 13 real units");
}

#[test]
fn a_listing_with_no_fulfilment_availability_reports_zero() {
    // A suppressed listing carries no availability at all. Zero is the safe
    // reading — it will not cause an oversell — but it is indistinguishable
    // from a real zero, so a suppression looks like a sellout.
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
    assert_eq!(pants.price, Some(48.00));
    assert_eq!(
        pants.image_url.as_deref(),
        Some("https://images.example.test/pants-main.jpg")
    );
}

#[test]
fn only_the_first_marketplace_offer_sets_the_price() {
    // The cap is offered at USD 48.00-equivalent in the US and CAD 32.50 in
    // Canada. The mapping reports the first offer's amount with no marketplace
    // filtering, so the price Nisaba shows depends on array order — and a
    // non-USD amount is reported as a bare number with its currency dropped on
    // `to_listing`.
    let cap = to_listing(&item("CAP-WOOL"));
    assert_eq!(cap.price, Some(24.00));

    let full = to_full_listing(&item("CAP-WOOL"));
    let price = full.price.expect("a price");
    assert_eq!(price.amount, 24.00);
    assert_eq!(price.currency, "USD");
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
    // The Listings Items API returns just `mainImage` in the summary, so this
    // is one photo at most — not the listing's full image set.
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
    // The Listings Items API does not expose one, which is why
    // `can_fetch_description` is false for this adapter.
    for sku in ["PANTS-RIPSTOP-M", "CAP-WOOL"] {
        assert!(to_full_listing(&item(sku)).description.is_none());
    }
}

#[test]
fn listing_issues_are_not_surfaced_anywhere_in_the_mapping() {
    // Documenting a gap rather than asserting it is right. The suppressed boots
    // carry an ERROR-severity issue explaining exactly why they are not selling,
    // and none of the three mappings carry it through — `extras` is empty — so
    // the UI shows a normal listing with zero stock and no reason.
    let raw = item("BOOTS-SUPPRESSED");
    assert_eq!(raw.issues.len(), 1);
    assert_eq!(raw.issues[0].severity.as_deref(), Some("ERROR"));

    let full = to_full_listing(&raw);
    assert!(full.extras.is_empty());
}
