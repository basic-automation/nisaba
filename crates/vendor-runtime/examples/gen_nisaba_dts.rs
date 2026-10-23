//! Regenerate `plugins/template/nisaba.d.ts` from the Rust plugin types.
//!
//! Run: `cargo run -p nisaba-vendor-runtime --example gen_nisaba_dts`

fn main() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/template/nisaba.d.ts");
    let dts = nisaba_vendor_runtime::dts::generate_nisaba_dts();
    std::fs::write(&path, &dts).unwrap();
    println!("wrote {} ({} bytes)", path.display(), dts.len());
}
