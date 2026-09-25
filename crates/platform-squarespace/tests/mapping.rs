//! Fixture-based tests for the Squarespace mapping layer.
//!
//! Squarespace splits what Nisaba treats as one thing across two endpoints:
//! `/commerce/products` knows names, images and prices, `/commerce/inventory`
//! knows quantities, and the two are joined on `variantId`. The join is where
//! the mistakes live — a variant the inventory endpoint did not return, or one
//! marked unlimited, has to be handled deliberately rather than by accident,
//! because whatever comes out of here is what the sync engine reconciles
//! against every other platform.
//!
//! The fixtures are the response *shapes* with invented data. No real store,
//! API key, product or order appears in them.

use nisaba_squarespace::mapping::{to_full_listing, to_inventory_item, to_listings};
use nisaba_squarespace::types::{SquarespaceInventoryResponse, SquarespaceProductsResponse};

fn products() -> SquarespaceProductsResponse {
    serde_json::from_str(include_str!("fixtures/products.json")).expect("products fixture parses")
}

fn inventory() -> SquarespaceInventoryResponse {
    serde_json::from_str(include_str!("fixtures/inventory.json")).expect("inventory fixture parses")
}

// ── to_inventory_item ─────────────────────────────────────────

#[test]
fn unlimited_inventory_is_not_quantity_trackable() {
    let inv = inventory();
    let gift = inv
        .inventory
        .iter()
        .find(|i| i.variant_id == "var_gift_25")
        .unwrap();
    assert!(
        to_inventory_item(gift).is_none(),
        "an unlimited variant has no quantity to reconcile; reporting its 0 would \
         drag the canonical quantity to zero on every cycle"
    );
}

#[test]
fn finite_inventory_maps_straight_through() {
    let inv = inventory();
    let cap = inv
        .inventory
        .iter()
        .find(|i| i.variant_id == "var_cap_default")
        .unwrap();
    let item = to_inventory_item(cap).expect("a finite variant maps");
    assert_eq!(item.platform_item_id, "var_cap_default");
    assert_eq!(item.quantity, 12);
}

#[test]
fn a_finite_zero_is_a_real_zero_not_an_absence() {
    let inv = inventory();
    let medium = inv
        .inventory
        .iter()
        .find(|i| i.variant_id == "var_pants_m")
        .unwrap();
    let item = to_inventory_item(medium).expect("out of stock is still trackable");
    assert_eq!(item.quantity, 0);
}

// ── to_listings ───────────────────────────────────────────────

#[test]
fn every_finite_variant_becomes_its_own_listing() {
    let listings = to_listings(&products().products, &inventory().inventory);
    let ids: Vec<&str> = listings
        .iter()
        .map(|l| l.platform_item_id.as_str())
        .collect();

    assert_eq!(
        ids,
        vec![
            "var_cap_default",
            "var_pants_s",
            "var_pants_m",
            "var_pants_l"
        ],
        "one listing per finite variant, in product then variant order"
    );
}

#[test]
fn an_unlimited_variant_is_left_out_entirely() {
    let listings = to_listings(&products().products, &inventory().inventory);
    assert!(
        !listings.iter().any(|l| l.platform_item_id == "var_gift_25"),
        "the gift card is unlimited and cannot be reconciled against other platforms"
    );
}

#[test]
fn a_product_with_no_variants_contributes_nothing() {
    let listings = to_listings(&products().products, &inventory().inventory);
    assert!(!listings
        .iter()
        .any(|l| l.group_key.as_deref() == Some("prod_no_variants")));
}

#[test]
fn a_single_variant_product_keeps_the_plain_product_name() {
    let listings = to_listings(&products().products, &inventory().inventory);
    let cap = listings
        .iter()
        .find(|l| l.platform_item_id == "var_cap_default")
        .unwrap();
    assert_eq!(cap.title, "Wool Watch Cap");
    assert_eq!(cap.sku.as_deref(), Some("CAP-001"));
    assert_eq!(cap.quantity, 12);
    assert_eq!(cap.price, Some(24.00));
    assert_eq!(cap.group_key.as_deref(), Some("prod_single"));
}

#[test]
fn a_multi_variant_product_suffixes_the_variant_name() {
    let listings = to_listings(&products().products, &inventory().inventory);
    let small = listings
        .iter()
        .find(|l| l.platform_item_id == "var_pants_s")
        .unwrap();
    assert_eq!(
        small.title, "Ripstop Field Pants [Small]",
        "the inventory endpoint's variantName is preferred for the suffix"
    );
    assert_eq!(small.quantity, 3);
}

#[test]
fn the_variant_suffix_falls_back_to_sku_when_inventory_has_no_name() {
    // `var_pants_l` has no inventory row at all, so there is no variantName to
    // use and the SKU stands in.
    let listings = to_listings(&products().products, &inventory().inventory);
    let large = listings
        .iter()
        .find(|l| l.platform_item_id == "var_pants_l")
        .unwrap();
    assert_eq!(large.title, "Ripstop Field Pants [PANTS-L]");
}

#[test]
fn a_variant_missing_from_the_inventory_response_is_reported_as_zero() {
    // Documenting current behaviour, which is load-bearing: `var_pants_l` is in
    // the product catalogue but absent from the inventory response, and the
    // mapping reports quantity 0 rather than skipping it. Because the sync
    // engine resolves the *minimum* across platforms, a truncated or paginated
    // inventory response therefore pulls the canonical quantity to zero and
    // pushes that zero to every other platform. If that is not the intended
    // policy, this is the test to change.
    let listings = to_listings(&products().products, &inventory().inventory);
    let large = listings
        .iter()
        .find(|l| l.platform_item_id == "var_pants_l")
        .unwrap();
    assert_eq!(large.quantity, 0);
    assert_eq!(
        large.price,
        Some(52.50),
        "its price still comes from the product"
    );
}

#[test]
fn variant_attributes_and_the_first_image_ride_along() {
    let listings = to_listings(&products().products, &inventory().inventory);
    let medium = listings
        .iter()
        .find(|l| l.platform_item_id == "var_pants_m")
        .unwrap();
    assert_eq!(
        medium.variant_attributes.get("Size").map(String::as_str),
        Some("M")
    );
    assert_eq!(
        medium.image_url.as_deref(),
        Some("https://images.example.test/pants.jpg")
    );

    let cap = listings
        .iter()
        .find(|l| l.platform_item_id == "var_cap_default")
        .unwrap();
    assert_eq!(
        cap.image_url.as_deref(),
        Some("https://images.example.test/cap-front.jpg"),
        "only the first image is carried as the thumbnail"
    );
}

// ── to_full_listing ───────────────────────────────────────────

#[test]
fn a_full_listing_carries_price_description_and_every_photo() {
    let products = products();
    let inventory = inventory();
    let cap_product = products
        .products
        .iter()
        .find(|p| p.id == "prod_single")
        .unwrap();

    let full = to_full_listing(cap_product, "var_cap_default", &inventory.inventory)
        .expect("the variant exists");

    assert_eq!(full.platform_item_id, "var_cap_default");
    assert_eq!(full.title, "Wool Watch Cap");
    assert_eq!(full.quantity, 12);

    let price = full.price.expect("a base price");
    assert_eq!(price.amount, 24.00);
    assert_eq!(price.currency, "USD");

    let description = full.description.expect("a description");
    assert_eq!(description.html.as_deref(), Some("<p>Ribbed wool cap.</p>"));

    assert_eq!(
        full.photos.len(),
        2,
        "every product image, not just the first"
    );
    assert_eq!(full.photos[0].position, 0);
    assert_eq!(full.photos[1].position, 1);
    assert_eq!(
        full.photos[1].url,
        "https://images.example.test/cap-back.jpg"
    );

    assert_eq!(
        full.url.as_deref(),
        Some("https://example.test/store/wool-watch-cap")
    );
}

#[test]
fn a_full_listing_for_an_unknown_variant_is_none() {
    let products = products();
    let inventory = inventory();
    let cap_product = products
        .products
        .iter()
        .find(|p| p.id == "prod_single")
        .unwrap();

    assert!(to_full_listing(cap_product, "var_does_not_exist", &inventory.inventory).is_none());
}

#[test]
fn a_full_listing_for_a_product_with_no_variants_is_none() {
    let products = products();
    let inventory = inventory();
    let service = products
        .products
        .iter()
        .find(|p| p.id == "prod_no_variants")
        .unwrap();

    assert!(to_full_listing(service, "anything", &inventory.inventory).is_none());
}

#[test]
fn a_product_with_no_description_or_images_still_maps() {
    let products = products();
    let inventory = inventory();
    let gift = products
        .products
        .iter()
        .find(|p| p.id == "prod_unlimited")
        .unwrap();

    let full = to_full_listing(gift, "var_gift_25", &inventory.inventory).expect("maps");
    assert!(full.description.is_none());
    assert!(full.photos.is_empty());
    assert_eq!(
        full.quantity, 0,
        "an unlimited variant reports the raw 0 here; `to_inventory_item` is \
         what keeps it out of quantity reconciliation"
    );
}
