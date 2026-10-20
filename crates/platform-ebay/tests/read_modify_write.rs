//! Fixture tests for lossless eBay read-modify-write (N-5.2).
//!
//! `createOrReplaceInventoryItem` and `updateOffer` are both full
//! replaces: whatever the PUT body omits is deleted server-side. These
//! tests pin that a description edit built from a richly populated item
//! carries every unmodelled field through, that quantity is never
//! written back (it may be stale by PUT time), and that an offer update
//! is either a complete body or an explicit error — never the partial
//! body `update_listing` used to send.

use nisaba_ebay::client::{
    build_inventory_item_put_body, build_offer_put_body, validate_complete_offer_body,
    InventoryItemOverrides, OfferOverrides,
};

fn desc_edit(description: &str) -> InventoryItemOverrides<'_> {
    InventoryItemOverrides {
        description: Some(description),
        ..Default::default()
    }
}
use nisaba_ebay::types::{EbayInventoryItem, EbayOffer, EbayOffersResponse};

fn rich_item() -> EbayInventoryItem {
    serde_json::from_str(include_str!("fixtures/inventory_item_rich.json"))
        .expect("rich inventory fixture parses")
}

fn rich_offer() -> EbayOffer {
    serde_json::from_str(include_str!("fixtures/offer_rich.json"))
        .expect("rich offer fixture parses")
}

// ── Inventory item: description edit is lossless ──────────────────────

#[test]
fn a_description_edit_round_trips_unmodelled_product_fields() {
    let body =
        build_inventory_item_put_body(&rich_item(), &desc_edit("<p>Edited description.</p>"));

    // The edit itself landed.
    assert_eq!(body["product"]["description"], "<p>Edited description.</p>");

    // Modelled-but-previously-dropped product identifiers survive.
    assert_eq!(body["product"]["brand"], "Example Outfitters");
    assert_eq!(body["product"]["mpn"], "MPN-12345");
    assert_eq!(body["product"]["upc"], "012345678905");
    assert_eq!(body["product"]["ean"], "5901234123457");
    assert_eq!(body["product"]["isbn"], "9780000000000");
    assert_eq!(body["product"]["epid"], "123456789");

    // Fields the adapter already modelled survive untouched.
    assert_eq!(body["product"]["title"], "Richly Described Widget");
    assert_eq!(
        body["product"]["imageUrls"][0],
        "https://images.example.test/widget-1.jpg"
    );
    assert_eq!(body["product"]["aspects"]["Size"][0], "L");

    // A product field nobody modelled survives via the flatten capture.
    assert_eq!(body["product"]["futureProductField"][0], "do-not-drop-me");
}

#[test]
fn a_description_edit_round_trips_unmodelled_item_fields() {
    let body = build_inventory_item_put_body(&rich_item(), &desc_edit("new"));

    assert_eq!(body["condition"], "USED_GOOD");
    // `conditionDescription` used to be dropped on the set_description path.
    assert_eq!(
        body["conditionDescription"],
        "Light scratch on the underside, works perfectly."
    );
    assert_eq!(body["conditionDescriptors"][0]["name"], "3000");
    assert_eq!(
        body["conditionDescriptors"][0]["values"][0],
        "No visible wear from a distance"
    );
    assert_eq!(body["packageWeightAndSize"]["weight"]["value"], "1.5");
    assert_eq!(body["packageWeightAndSize"]["dimensions"]["length"], "10");
    // Unknown top-level fields survive via the flatten capture.
    assert_eq!(body["locale"], "en_US");
    assert_eq!(
        body["futureTopLevelField"]["introducedByEbay"],
        "after this adapter was written"
    );
    // The SKU is the PUT path, not a body field.
    assert!(body.get("sku").is_none());
}

#[test]
fn a_description_edit_never_writes_back_quantity() {
    // The fixture item reports quantity 5. A sale between the read and the
    // PUT would make writing 5 back resurrect sold stock, so the body
    // must carry no quantity at all; quantity is owned by the bulk
    // price/quantity endpoint.
    let body = build_inventory_item_put_body(&rich_item(), &desc_edit("new"));

    let ship = &body["availability"]["shipToLocationAvailability"];
    assert!(
        ship.get("quantity").is_none(),
        "shipToLocationAvailability must not write quantity back: {ship}"
    );
    assert!(
        ship["availabilityDistributions"][0]
            .get("quantity")
            .is_none(),
        "per-location distribution quantity must not be written back either"
    );
    // The rest of the availability block is not collateral damage.
    assert_eq!(
        ship["availabilityDistributions"][0]["merchantLocationKey"],
        "loc-1"
    );
    assert_eq!(
        body["availability"]["pickupAtLocationAvailability"]["merchantLocationKey"],
        "loc-1"
    );
    assert!(
        body["availability"]["pickupAtLocationAvailability"]
            .get("quantity")
            .is_none(),
        "pickup quantity must not be written back either"
    );
}

#[test]
fn an_item_with_no_availability_does_not_gain_a_zero_quantity() {
    // The old body construction fell back to quantity 0 when availability
    // was absent, zeroing stock on a description edit.
    let item: EbayInventoryItem = serde_json::from_str(
        r#"{"sku":"BARE-1","condition":"NEW","product":{"title":"Bare","description":"old"}}"#,
    )
    .expect("bare item parses");
    let body = build_inventory_item_put_body(&item, &desc_edit("new"));
    assert!(body.get("availability").is_none(), "body was: {body}");
    assert_eq!(body["product"]["description"], "new");
}

#[test]
fn explicit_overrides_win_and_everything_else_survives() {
    let aspects = std::collections::HashMap::from([("Size".to_string(), vec!["XL".to_string()])]);
    let body = build_inventory_item_put_body(
        &rich_item(),
        &InventoryItemOverrides {
            title: Some("Renamed Widget"),
            aspects: Some(&aspects),
            condition: Some("USED_EXCELLENT"),
            condition_description: Some("New condition note."),
            ..Default::default()
        },
    );
    assert_eq!(body["product"]["title"], "Renamed Widget");
    assert_eq!(body["product"]["aspects"]["Size"][0], "XL");
    assert_eq!(body["condition"], "USED_EXCELLENT");
    assert_eq!(body["conditionDescription"], "New condition note.");
    // Untouched fields still round-trip alongside the overrides.
    assert_eq!(body["product"]["brand"], "Example Outfitters");
    assert_eq!(
        body["product"]["description"],
        "<p>Original description.</p>"
    );
}

// ── Offer: complete body or explicit error ────────────────────────────

#[test]
fn an_offer_update_sends_a_complete_body_built_from_the_fetched_offer() {
    let body = build_offer_put_body(
        &rich_offer(),
        &OfferOverrides {
            category_id: Some("12345"),
            fulfillment_policy_id: Some("fp_new"),
            ..Default::default()
        },
    )
    .expect("a complete fetched offer yields a complete body");

    // The overrides landed.
    assert_eq!(body["categoryId"], "12345");
    assert_eq!(body["listingPolicies"]["fulfillmentPolicyId"], "fp_new");

    // Everything a full replace needs that was NOT being changed is
    // carried from the fetched offer, not dropped.
    assert_eq!(body["sku"], "WIDGET-RICH-1");
    assert_eq!(body["marketplaceId"], "EBAY_US");
    assert_eq!(body["format"], "FIXED_PRICE");
    assert_eq!(body["availableQuantity"], 5);
    assert_eq!(body["pricingSummary"]["price"]["value"], "48.00");
    assert_eq!(body["pricingSummary"]["price"]["currency"], "USD");
    assert_eq!(body["listingPolicies"]["paymentPolicyId"], "pp_1");
    assert_eq!(body["listingPolicies"]["returnPolicyId"], "rp_1");
    assert_eq!(
        body["listingDescription"],
        "<p>Offer-level description.</p>"
    );

    // Unmodelled fields round-trip; read-only fields do not go back.
    assert_eq!(body["listingDuration"], "GTC");
    assert_eq!(body["merchantLocationKey"], "loc-1");
    assert_eq!(body["tax"]["thirdPartyTaxCategory"], "ELECTRONICS");
    assert_eq!(
        body["pricingSummary"]["minimumAdvertisedPrice"]["value"],
        "40.00"
    );
    assert!(body.get("offerId").is_none());
    assert!(body.get("listingId").is_none());
    assert!(body.get("status").is_none());

    validate_complete_offer_body(&body).expect("the built body is complete");
}

#[test]
fn an_offer_missing_required_fields_is_an_explicit_error_not_a_partial_body() {
    // The first fixture offer is unpublished and carries no pricingSummary:
    // there is no complete full-replace body to build from it.
    let response: EbayOffersResponse =
        serde_json::from_str(include_str!("fixtures/offers.json")).expect("offers fixture parses");
    let no_price = response
        .offers
        .expect("offers")
        .into_iter()
        .find(|o| o.offer_id.as_deref() == Some("offer_no_price"))
        .expect("the no-price offer");

    let err = build_offer_put_body(
        &no_price,
        &OfferOverrides {
            category_id: Some("999"),
            ..Default::default()
        },
    )
    .expect_err("an incomplete offer must not produce a PUT body");
    assert!(
        err.to_string().contains("pricingSummary"),
        "error should name the missing field, got: {err}"
    );
}

#[test]
fn the_old_partial_update_listing_body_is_rejected() {
    // Exactly what `update_listing` used to PUT to updateOffer: only the
    // changed category and a partial policies object.
    let partial = serde_json::json!({
        "categoryId": "57988",
        "listingPolicies": { "fulfillmentPolicyId": "fp_9" }
    });
    let err = validate_complete_offer_body(&partial)
        .expect_err("a partial body must be rejected before it is sent");
    assert!(
        err.to_string().contains("partial"),
        "error should call the body partial, got: {err}"
    );
}
