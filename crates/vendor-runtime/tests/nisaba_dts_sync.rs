//! `plugins/template/nisaba.d.ts` is generated from the Rust plugin types, so
//! the checked-in file must be exactly what the generator produces —
//! otherwise the published types have drifted from the host. Regenerate with
//! `cargo run -p nisaba-vendor-runtime --example gen_nisaba_dts`.

#[test]
fn nisaba_dts_is_in_sync_with_the_rust_types() {
    let generated = nisaba_vendor_runtime::dts::generate_nisaba_dts();
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/template/nisaba.d.ts");
    let checked_in = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "missing {} — run the gen_nisaba_dts example",
            path.display()
        )
    });
    assert_eq!(
        generated, checked_in,
        "plugins/template/nisaba.d.ts is stale: \
         run `cargo run -p nisaba-vendor-runtime --example gen_nisaba_dts`"
    );
}
