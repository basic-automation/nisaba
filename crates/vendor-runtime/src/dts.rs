//! Generates `plugins/template/nisaba.d.ts` from the Rust plugin types (N-12.2).
//!
//! Every data type in the published `.d.ts` is derived from the Rust structs
//! via `ts_rs::TS`, so the TypeScript surface cannot drift from the host.
//! `cargo test -p nisaba-vendor-runtime --test nisaba_dts_sync` fails when the
//! checked-in file differs from what this module produces; regenerate with
//! `cargo run -p nisaba-vendor-runtime --example gen_nisaba_dts`.
//!
//! Two deliberate hand-touches, both documented where they happen:
//! - fields whose Rust side carries `#[serde(default)]` on a non-`Option` type
//!   (`images`, `extras`, …) are emitted optional — the host fills them in,
//!   so plugin authors may omit them;
//! - the `Nisaba` host object is appended from `nisaba_host_api.ts`, which
//!   mirrors `src/runtime_js.js` by hand (the shim creates it, not Rust).

use ts_rs::TS;

use crate::types::{
    ConfigField, FetchListingsArgs, FetchListingsResult, ListingDelta, PluginInfo, PluginMetadata,
    VendorListing, HOST_PLUGIN_API_VERSION,
};
use nisaba_core::types::{Dimensions, Weight};

/// (`type name`, `fields`) whose Rust fields carry `#[serde(default)]` on a
/// non-`Option` type: the host supplies the default, so authors may omit them.
/// `ts_rs` only marks `Option<T>` fields optional, hence this explicit list.
const OPTIONAL_BY_SERDE_DEFAULT: &[(&str, &[&str])] = &[
    ("VendorListing", &["images", "extras", "variant_attributes"]),
    (
        "PluginMetadata",
        &["description", "config_fields", "category"],
    ),
    ("ConfigField", &["required", "secret"]),
    ("ListingDelta", &["deleted"]),
];

/// Hand-written host API surface (the `Nisaba` global), mirroring
/// `src/runtime_js.js`. Kept as a separate `.ts` file so it stays readable;
/// it is concatenated verbatim after the generated types.
const HOST_API_TS: &str = include_str!("nisaba_host_api.ts");

/// Build the full text of `plugins/template/nisaba.d.ts`.
pub fn generate_nisaba_dts() -> String {
    let mut out = String::new();
    out.push_str(
        "// nisaba.d.ts — the Nisaba plugin API v1, generated from the Rust types.\n\
         //\n\
         // DO NOT EDIT BY HAND. Regenerate with:\n\
         //   cargo run -p nisaba-vendor-runtime --example gen_nisaba_dts\n\
         //\n\
         // Every data type below is derived from the Rust structs via ts-rs\n\
         // (`crates/vendor-runtime/src/dts.rs`), so this file cannot drift from\n\
         // the host. The `Nisaba` host object at the end is hand-written to\n\
         // mirror `crates/vendor-runtime/src/runtime_js.js` — the JS shim\n\
         // creates it, not a Rust type, so keep the two in sync by hand.\n\
         \n",
    );

    for (name, decl) in generated_decls() {
        out.push_str(&render_decl(name, &decl));
        out.push('\n');
    }

    out.push_str(&format!(
        "/**\n\
         \x20* The plugin API major version this host implements. A host refuses\n\
         \x20* to install a plugin whose `metadata.api_version` differs.\n\
         \x20*/\n\
         export declare const HOST_PLUGIN_API_VERSION: {HOST_PLUGIN_API_VERSION};\n\
         \n"
    ));

    out.push_str(
        "/**\n\
         \x20* What the host passes as the second argument to `fetchListings`.\n\
         \x20*\n\
         \x20* `since` is the UTC timestamp of the last successful sync (`null` on the\n\
         \x20* first run — return the whole catalog then). `cursor` is the opaque cursor\n\
         \x20* your plugin returned on the previous run (`null` when it returned none).\n\
         \x20* A plugin may ignore both and keep returning the full catalog; the host\n\
         \x20* then keeps full-pull semantics.\n\
         \x20*/\n\
         export type FetchListingsOptions = FetchListingsArgs;\n\
         \n\
         /**\n\
         \x20* The incremental alternative to returning the whole catalog. The host\n\
         \x20* merges it into the cached catalog: `changes` are upserted by\n\
         \x20* `vendor_item_id`, ids in `deleted` are removed, and everything else is\n\
         \x20* left untouched. Return a new `cursor` so the next run can pick up where\n\
         \x20* this one stopped.\n\
         \x20*/\n\
         export type ListingChanges = ListingDelta;\n\
         \n",
    );

    out.push_str(HOST_API_TS);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// The generated declarations in file order: `(TS name, ts_rs decl)`.
fn generated_decls() -> Vec<(&'static str, String)> {
    // ts-rs 12 requires an explicit `Config` for `decl()`; keep the frontend's
    // integer convention (all integers as `number`, matching generated.ts).
    let cfg = ts_rs::Config::new().with_large_int("number");
    vec![
        ("Weight", Weight::decl(&cfg)),
        ("Dimensions", Dimensions::decl(&cfg)),
        ("VendorListing", VendorListing::decl(&cfg)),
        ("FetchListingsArgs", FetchListingsArgs::decl(&cfg)),
        ("ListingDelta", ListingDelta::decl(&cfg)),
        ("FetchListingsResult", FetchListingsResult::decl(&cfg)),
        ("PluginMetadata", PluginMetadata::decl(&cfg)),
        ("ConfigField", ConfigField::decl(&cfg)),
        ("PluginInfo", PluginInfo::decl(&cfg)),
    ]
}

/// Render one `ts_rs` decl (`type Name = …;`) as an exported `.d.ts`
/// declaration, pretty-printed and with serde-defaulted fields optional.
fn render_decl(name: &str, decl: &str) -> String {
    let decl = decl.trim();
    let body = decl
        .strip_prefix("type ")
        .and_then(|d| d.strip_suffix(';'))
        .unwrap_or(decl);
    let body = body.strip_prefix(name).unwrap_or(body).trim();
    let body = body.strip_prefix('=').unwrap_or(body).trim();

    // Object type → exported interface; anything else → exported type alias.
    if let Some(fields) = body.strip_prefix('{').and_then(|b| b.strip_suffix('}')) {
        let mut out = format!("export interface {name} {{\n");
        for mut field in split_fields(fields) {
            mark_optional_by_serde_default(name, &mut field);
            let field = field.trim_end_matches(',').trim();
            let mut lines: Vec<String> = field.lines().map(str::to_string).collect();
            if let Some(last) = lines.last_mut() {
                last.push(';');
            }
            for line in lines {
                out.push_str("  ");
                out.push_str(&line);
                out.push('\n');
            }
        }
        out.push_str("}\n");
        out
    } else {
        format!("export type {name} = {body};\n")
    }
}

/// Split an interface body into field chunks at top-level commas, skipping
/// over `/* … */` comments, string literals and nested `<>()[]{}`.
fn split_fields(body: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    let chars: Vec<char> = body.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2;
            continue;
        }
        if c == '"' || c == '\'' || c == '`' {
            let q = c;
            i += 1;
            while i < chars.len() && chars[i] != q {
                if chars[i] == '\\' {
                    i += 1;
                }
                i += 1;
            }
            i += 1;
            continue;
        }
        match c {
            '(' | '[' | '{' | '<' => depth += 1,
            ')' | ']' | '}' | '>' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                let chunk: String = chars[start..i].iter().collect();
                if !chunk.trim().is_empty() {
                    fields.push(chunk);
                }
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    let tail: String = chars[start..].iter().collect();
    if !tail.trim().is_empty() {
        fields.push(tail);
    }
    fields
}

/// Append `?` to fields listed in [`OPTIONAL_BY_SERDE_DEFAULT`] that ts_rs
/// left required (non-`Option` fields with `#[serde(default)]`).
fn mark_optional_by_serde_default(type_name: &str, field: &mut String) {
    let optional_fields = OPTIONAL_BY_SERDE_DEFAULT
        .iter()
        .find(|(t, _)| *t == type_name)
        .map(|(_, f)| *f)
        .unwrap_or(&[]);
    if optional_fields.is_empty() {
        return;
    }
    // Skip a leading `/** … */` doc comment to find the field name.
    let mut rest = field.as_str();
    if let Some(end) = rest.find("*/") {
        rest = &rest[end + 2..];
    }
    let rest = rest.trim_start();
    let name_end = rest
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(rest.len());
    let field_name = &rest[..name_end];
    if !optional_fields.contains(&field_name) || rest[name_end..].starts_with('?') {
        return;
    }
    // Insert `?` right after the field name in the original chunk.
    let insert_at = field.len() - rest.len() + name_end;
    field.insert(insert_at, '?');
}
