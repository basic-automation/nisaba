pub mod ops;
pub mod runtime;
pub mod types;

use deno_core::extension;

extension!(
    nisaba_vendor_ext,
    ops = [
        ops::op_nisaba_fetch,
        ops::op_nisaba_sleep,
        ops::op_nisaba_log,
        ops::op_nisaba_set_result,
        ops::op_nisaba_emit_batch,
        ops::op_nisaba_store_get,
        ops::op_nisaba_store_set,
        ops::op_nisaba_store_delete
    ],
    esm_entry_point = "ext:nisaba_vendor_ext/runtime_js.js",
    esm = [dir "src", "runtime_js.js"],
);

pub use ops::{
    BatchCallback, BlockPrivateHosts, FetchPolicy, PluginLogCollector, PluginStore,
    PluginStoreContext, StoreError, PLUGIN_STORE_CAP_BYTES, PLUGIN_STORE_MAX_KEY_BYTES,
};
pub use runtime::{PluginLimits, VendorRuntime};
pub use types::{
    ConfigField, FetchListingsArgs, FetchListingsResult, ListingDelta, PluginInfo, PluginMetadata,
    VendorListing, HOST_PLUGIN_API_VERSION,
};
