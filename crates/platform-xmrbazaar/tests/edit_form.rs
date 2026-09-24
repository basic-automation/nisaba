//! Golden-file tests for `parse_edit_form`.
//!
//! XMR Bazaar has no API. Every write is "GET the edit page, keep every field
//! the form carries, override the one we care about, POST the whole thing back
//! as `application/x-www-form-urlencoded`". That makes `parse_edit_form` the
//! most consequential parser in the tree: a field it reports wrongly is a field
//! the adapter silently rewrites on a live listing.
//!
//! So these tests assert browser form-submission semantics, not just "did it
//! parse". The fixture in `fixtures/` is hand-built from the documented field
//! list — it holds no real listing, wallet or session.

use nisaba_xmrbazaar::scraper::{
    detect_stock_mode_field, extract_description_from_form, parse_edit_form,
};
use nisaba_xmrbazaar::types::EditFormData;

const EDIT_FORM: &str = include_str!("fixtures/edit-listing-form.html");

fn parsed() -> EditFormData {
    parse_edit_form(EDIT_FORM).expect("the fixture carries a validation token and a form")
}

fn value_of<'a>(form: &'a EditFormData, name: &str) -> Option<&'a str> {
    form.fields
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
}

fn count_of(form: &EditFormData, name: &str) -> usize {
    form.fields.iter().filter(|(n, _)| n == name).count()
}

#[test]
fn finds_the_csrf_token_under_xmr_bazaars_field_name() {
    let form = parsed();
    assert_eq!(form.csrf_field_name, "validation");
    assert_eq!(form.csrf_token, "tok_abcdef0123456789");
    assert_eq!(
        count_of(&form, "validation"),
        0,
        "the token is tracked separately and must not be duplicated into fields"
    );
}

#[test]
fn keeps_plain_text_and_textarea_values() {
    let form = parsed();
    assert_eq!(value_of(&form, "title"), Some("Ripstop Field Pants"));
    assert_eq!(value_of(&form, "price"), Some("48.00"));
    assert_eq!(value_of(&form, "tags"), Some("pants,ripstop,field"));
    assert_eq!(
        value_of(&form, "city"),
        Some(""),
        "an empty text field must round-trip as empty, not vanish"
    );

    let description = value_of(&form, "description").expect("description field");
    assert!(description.starts_with("Six-pocket ripstop field pants."));
    assert!(description.contains("neither HTML nor markdown"));
}

#[test]
fn reads_the_selected_option_from_a_select_not_the_first_one() {
    let form = parsed();
    assert_eq!(
        value_of(&form, "category"),
        Some("clothes"),
        "the selected option wins over the leading placeholder"
    );
    assert_eq!(value_of(&form, "currency"), Some("USD"));
    assert_eq!(value_of(&form, "status"), Some("active"));
}

#[test]
fn an_unchecked_checkbox_is_not_submitted() {
    // A browser sends nothing at all for an unchecked box. If the adapter
    // re-posts one, a routine quantity update silently switches the option on:
    // `international_shipping` and `private_listing` are both unchecked here
    // and both change what a buyer sees.
    let form = parsed();
    for name in [
        "flexible_price",
        "in_person_pickup",
        "international_shipping",
        "private_listing",
    ] {
        assert_eq!(
            count_of(&form, name),
            0,
            "unchecked checkbox `{name}` must not be re-submitted"
        );
    }
}

#[test]
fn a_checked_checkbox_is_submitted_with_its_value() {
    let form = parsed();
    assert_eq!(value_of(&form, "domestic_shipping"), Some("1"));
    assert_eq!(value_of(&form, "terms"), Some("1"));
}

#[test]
fn a_radio_group_contributes_exactly_the_checked_option() {
    // Every option in a radio group carries the same `name`. Collecting all of
    // them puts `stock=unlimited&stock=one-time` on the wire and lets the
    // server pick — which is how a listing flips payment method or delivery
    // type during an unrelated update.
    let form = parsed();
    for (name, expected) in [
        ("delivery", "physical"),
        ("stock", "unlimited"),
        ("payment_method", "direct"),
    ] {
        assert_eq!(
            count_of(&form, name),
            1,
            "radio group `{name}` must contribute exactly one value"
        );
        assert_eq!(value_of(&form, name), Some(expected));
    }
}

#[test]
fn file_inputs_are_dropped() {
    // A file input cannot be expressed in a form-urlencoded body. Sending the
    // name with an empty value asks the server to clear the photo.
    let form = parsed();
    assert_eq!(count_of(&form, "photo"), 0);
    assert_eq!(count_of(&form, "additional_photo_1"), 0);
}

#[test]
fn disabled_fields_are_dropped() {
    let form = parsed();
    assert_eq!(
        count_of(&form, "seller_handle"),
        0,
        "a disabled control is not successful and is never submitted"
    );
}

#[test]
fn submit_controls_are_dropped() {
    // Only the control the user actually activated is submitted. Posting both
    // `save_draft` and `publish` lets the server choose which action ran.
    let form = parsed();
    assert_eq!(count_of(&form, "save_draft"), 0);
    assert_eq!(count_of(&form, "publish"), 0);
}

#[test]
fn fields_from_other_forms_on_the_page_are_not_mixed_in() {
    let form = parsed();
    assert_eq!(
        count_of(&form, "q"),
        0,
        "the site-search box lives in its own <form> and must not ride along"
    );
}

#[test]
fn the_round_trip_carries_every_field_the_listing_needs() {
    // The whole point of the scrape is that re-posting is lossless for the
    // fields the listing actually depends on.
    let form = parsed();
    for name in [
        "title",
        "category",
        "price",
        "currency",
        "description",
        "tags",
        "status",
        "delivery",
        "stock",
        "payment_method",
        "country",
        "city",
        "domestic_shipping",
        "domestic_shipping_cost",
        "international_shipping_cost",
        "monero_address",
        "terms",
    ] {
        assert_eq!(
            count_of(&form, name),
            1,
            "`{name}` must survive the round trip"
        );
    }
}

#[test]
fn description_is_recovered_from_the_parsed_fields() {
    let form = parsed();
    let description = extract_description_from_form(&form).expect("a description field");
    assert!(description.starts_with("Six-pocket ripstop field pants."));
}

#[test]
fn the_stock_radio_is_recognised_as_the_stock_mode_field() {
    let form = parsed();
    let field = detect_stock_mode_field(&form).expect("the `stock` radio group");
    assert_eq!(field.field_name, "stock");
    assert_eq!(
        field.unlimited_value, "unlimited",
        "the fixture's listing is currently on unlimited stock"
    );
    assert_eq!(field.onetime_value, "one-time");
}

#[test]
fn a_page_with_no_token_is_not_treated_as_a_form() {
    // A logged-out page still contains a <form>; without a token it is a login
    // wall, and parsing it as an edit form would post garbage.
    let logged_out =
        r#"<html><body><form action="/login/"><input name="username"></form></body></html>"#;
    assert!(parse_edit_form(logged_out).is_none());
}

#[test]
fn an_empty_token_value_is_not_accepted() {
    let blank = r#"<html><body><form><input type="hidden" name="validation" value="">
      <input name="title" value="x"></form></body></html>"#;
    assert!(parse_edit_form(blank).is_none());
}
