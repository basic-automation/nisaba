//! Fixture-based tests for the eBay mapping layer.
//!
//! The adapter reads eBay through two very different doors — the Sell Inventory
//! API (JSON, SKU-keyed, price lives on a separate offer resource) and the
//! Trading API (XML, item-id-keyed, price inline) — and `mapping.rs` has to
//! land both on the same `PlatformListing` / `FullListing`. The fixtures are
//! the response shapes with invented data: no seller, token, real listing or
//! order appears in them.

use nisaba_ebay::mapping::{
    to_full_listing, to_inventory_item, to_listing, trading_to_full_listing, trading_to_listing,
};
use nisaba_ebay::types::{
    EbayInventoryItem, EbayInventoryItemsResponse, EbayOffer, EbayOffersResponse,
    TradingGetMyeBaySellingResponse, TradingItem,
};

fn inventory_items() -> Vec<EbayInventoryItem> {
    let response: EbayInventoryItemsResponse =
        serde_json::from_str(include_str!("fixtures/inventory_items.json"))
            .expect("inventory fixture parses");
    response.inventory_items.expect("the fixture has items")
}

fn offers() -> Vec<EbayOffer> {
    let response: EbayOffersResponse =
        serde_json::from_str(include_str!("fixtures/offers.json")).expect("offers fixture parses");
    response.offers.expect("the fixture has offers")
}

fn item(sku: &str) -> EbayInventoryItem {
    inventory_items()
        .into_iter()
        .find(|i| i.sku == sku)
        .unwrap_or_else(|| panic!("no fixture item with sku {sku}"))
}

fn trading_items() -> Vec<TradingItem> {
    let response: TradingGetMyeBaySellingResponse =
        quick_xml::de::from_str(include_str!("fixtures/get_my_ebay_selling.xml"))
            .expect("GetMyeBaySelling fixture parses");
    response
        .active_list
        .expect("an ActiveList")
        .item_array
        .expect("an ItemArray")
        .items
}

fn trading_item(item_id: &str) -> TradingItem {
    trading_items()
        .into_iter()
        .find(|i| i.item_id == item_id)
        .unwrap_or_else(|| panic!("no fixture item with id {item_id}"))
}

// ── Inventory API ─────────────────────────────────────────────

#[test]
fn inventory_quantity_comes_from_ship_to_location_availability() {
    let mapped = to_inventory_item(&item("PANTS-RIPSTOP-M"));
    assert_eq!(mapped.platform_item_id, "PANTS-RIPSTOP-M");
    assert_eq!(mapped.quantity, 7);
}

#[test]
fn a_zero_quantity_survives_as_zero() {
    assert_eq!(to_inventory_item(&item("CAP-WOOL")).quantity, 0);
}

#[test]
fn an_item_with_no_availability_block_reports_zero() {
    // eBay omits `availability` entirely for items that have never had stock
    // set. Reporting 0 is what keeps the sync engine from pushing a phantom
    // quantity at it, but it is indistinguishable from a real zero here.
    assert_eq!(to_inventory_item(&item("BARE-SKU-ONLY")).quantity, 0);
}

#[test]
fn the_sku_is_the_platform_item_id_on_the_inventory_api() {
    // eBay's Inventory API is SKU-keyed; there is no numeric item id until an
    // offer is published. Mappings stored against these ids are SKUs.
    for sku in ["PANTS-RIPSTOP-M", "CAP-WOOL", "BARE-SKU-ONLY"] {
        assert_eq!(to_inventory_item(&item(sku)).platform_item_id, sku);
        assert_eq!(to_listing(&item(sku)).platform_item_id, sku);
    }
}

#[test]
fn a_listing_falls_back_to_the_sku_when_there_is_no_title() {
    let bare = to_listing(&item("BARE-SKU-ONLY"));
    assert_eq!(bare.title, "BARE-SKU-ONLY");
    assert_eq!(bare.sku.as_deref(), Some("BARE-SKU-ONLY"));
    assert!(bare.image_url.is_none());
}

#[test]
fn a_listing_carries_the_title_and_first_image() {
    let pants = to_listing(&item("PANTS-RIPSTOP-M"));
    assert_eq!(pants.title, "Ripstop Field Pants — Medium");
    assert_eq!(
        pants.image_url.as_deref(),
        Some("https://images.example.test/pants-front.jpg")
    );
}

#[test]
fn a_listing_from_the_inventory_api_has_no_price() {
    // Price lives on the offer resource, not the inventory item, so the list
    // view genuinely cannot show one without a second call.
    assert!(to_listing(&item("PANTS-RIPSTOP-M")).price.is_none());
}

#[test]
fn a_full_listing_takes_the_first_offer_that_actually_has_a_price() {
    // The first offer in the fixture is unpublished and carries no
    // pricingSummary. Taking offers[0] blindly would report no price at all.
    let full = to_full_listing(&item("PANTS-RIPSTOP-M"), &offers());
    let price = full.price.expect("the published offer's price");
    assert_eq!(price.amount, 48.00);
    assert_eq!(price.currency, "USD");
}

#[test]
fn a_full_listing_url_comes_from_the_first_offer_with_a_listing_id() {
    let full = to_full_listing(&item("PANTS-RIPSTOP-M"), &offers());
    assert_eq!(
        full.url.as_deref(),
        Some("https://www.ebay.com/itm/110512345678")
    );
}

#[test]
fn a_full_listing_with_no_offers_has_neither_price_nor_url() {
    let full = to_full_listing(&item("CAP-WOOL"), &[]);
    assert!(full.price.is_none());
    assert!(full.url.is_none());
}

#[test]
fn photos_keep_their_order() {
    let full = to_full_listing(&item("PANTS-RIPSTOP-M"), &offers());
    assert_eq!(full.photos.len(), 2);
    assert_eq!(full.photos[0].position, 0);
    assert_eq!(
        full.photos[0].url,
        "https://images.example.test/pants-front.jpg"
    );
    assert_eq!(full.photos[1].position, 1);
    assert_eq!(
        full.photos[1].url,
        "https://images.example.test/pants-back.jpg"
    );
}

#[test]
fn ebay_specific_metadata_is_preserved_for_round_tripping() {
    // `update_listing` reads these back out of `extras`, so anything dropped
    // here is a field that silently reverts on the next write.
    let full = to_full_listing(&item("PANTS-RIPSTOP-M"), &offers());

    assert_eq!(
        full.extras.get("ebay_condition").map(String::as_str),
        Some("NEW")
    );
    assert_eq!(
        full.extras.get("ebay_category_id").map(String::as_str),
        Some("57988")
    );
    assert_eq!(
        full.extras.get("ebay_format").map(String::as_str),
        Some("FIXED_PRICE")
    );

    // Policies come from the *first* offer, which here is the unpublished one —
    // not the published offer the price and URL came from.
    assert_eq!(
        full.extras
            .get("ebay_fulfillment_policy_id")
            .map(String::as_str),
        Some("fp_1")
    );

    let aspects: serde_json::Value =
        serde_json::from_str(full.extras.get("ebay_aspects").expect("aspects")).unwrap();
    assert_eq!(aspects["Size"][0], "M");
    assert_eq!(aspects["Material"][1], "Polyester");
}

#[test]
fn an_item_with_no_product_block_carries_no_extras_or_description() {
    let full = to_full_listing(&item("BARE-SKU-ONLY"), &[]);
    assert!(full.description.is_none());
    assert!(full.photos.is_empty());
    assert!(full.extras.is_empty());
}

// ── Trading API (XML) ─────────────────────────────────────────

#[test]
fn the_trading_xml_response_deserializes() {
    let items = trading_items();
    assert_eq!(items.len(), 3);
    assert_eq!(items[0].item_id, "110512345678");
    assert_eq!(items[0].sku.as_deref(), Some("PANTS-RIPSTOP-M"));
}

#[test]
fn trading_quantity_is_listed_minus_sold() {
    // The Trading API reports the quantity originally listed and how many have
    // sold; what Nisaba needs is what is left.
    let pants = trading_to_listing(&trading_item("110512345678"));
    assert_eq!(pants.quantity, 7, "10 listed, 3 sold");

    let cap = trading_to_listing(&trading_item("110598765432"));
    assert_eq!(cap.quantity, 4, "nothing sold yet");
}

#[test]
fn a_trading_item_reporting_more_sold_than_listed_goes_negative() {
    // Documenting current behaviour: the subtraction has no floor. eBay can
    // report this during a revision or for a multi-variant listing. Nothing
    // downstream writes it back (the sync engine floors the canonical quantity
    // at zero), but it does surface in the mapping UI as a negative stock
    // figure, and it makes this listing the minimum in any comparison.
    let odd = trading_to_listing(&trading_item("110511111111"));
    assert_eq!(odd.quantity, -3, "2 listed, 5 sold");
}

#[test]
fn a_trading_item_falls_back_to_its_item_id_for_a_title() {
    let odd = trading_to_listing(&trading_item("110511111111"));
    assert_eq!(odd.title, "110511111111");
    assert!(odd.sku.is_none());
    assert!(odd.price.is_none());
    assert!(odd.image_url.is_none());
}

#[test]
fn the_trading_price_is_read_from_the_currency_attribute_and_element_text() {
    let full = trading_to_full_listing(&trading_item("110512345678"));
    let price = full.price.expect("a current price");
    assert_eq!(price.amount, 48.00);
    assert_eq!(price.currency, "USD");
}

#[test]
fn a_trading_listing_prefers_the_gallery_url_for_its_thumbnail() {
    let pants = trading_to_listing(&trading_item("110512345678"));
    assert_eq!(
        pants.image_url.as_deref(),
        Some("https://images.example.test/pants-gallery.jpg")
    );

    // With no GalleryURL, the first PictureURL stands in.
    let cap = trading_to_listing(&trading_item("110598765432"));
    assert_eq!(
        cap.image_url.as_deref(),
        Some("https://images.example.test/cap.jpg")
    );
}

#[test]
fn a_trading_full_listing_carries_every_picture_url() {
    let full = trading_to_full_listing(&trading_item("110512345678"));
    assert_eq!(
        full.photos.len(),
        2,
        "PictureURL entries only — the gallery thumbnail is not a photo"
    );
    assert_eq!(full.photos[0].position, 0);
    assert_eq!(full.photos[1].position, 1);
}

#[test]
fn a_trading_full_listing_keys_on_the_item_id_not_the_sku() {
    // The two eBay doors disagree about what identifies a listing: the
    // Inventory API says SKU, the Trading API says ItemID. Mappings created
    // from one will not match the other.
    let full = trading_to_full_listing(&trading_item("110512345678"));
    assert_eq!(full.platform_item_id, "110512345678");
    assert_eq!(full.sku.as_deref(), Some("PANTS-RIPSTOP-M"));
    assert_eq!(
        full.url.as_deref(),
        Some("https://www.ebay.com/itm/110512345678")
    );

    let description = full.description.expect("a description");
    assert_eq!(
        description.html.as_deref(),
        Some("<p>Six-pocket ripstop field pants.</p>"),
        "the XML-escaped HTML is unescaped on the way in"
    );
}
