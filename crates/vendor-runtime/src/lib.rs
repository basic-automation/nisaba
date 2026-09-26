pub mod ops;
pub mod runtime;
pub mod types;

use deno_core::extension;

extension!(
    nisaba_vendor_ext,
    ops = [ops::op_nisaba_fetch, ops::op_nisaba_sleep, ops::op_nisaba_log, ops::op_nisaba_set_result, ops::op_nisaba_emit_batch],
    esm_entry_point = "ext:nisaba_vendor_ext/runtime_js.js",
    esm = [dir "src", "runtime_js.js"],
);

pub use ops::BatchCallback;
pub use runtime::{PluginLimits, VendorRuntime};
pub use types::{ConfigField, PluginInfo, PluginMetadata, VendorListing};
