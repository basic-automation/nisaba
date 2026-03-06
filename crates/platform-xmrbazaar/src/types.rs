/// A parsed listing from XMR Bazaar's HTML.
#[derive(Debug, Clone)]
pub struct XmrListing {
    pub id: String,
    pub title: String,
    pub price: Option<f64>,
    pub quantity: i64,
    pub image_url: Option<String>,
    pub edit_url: String,
    pub description: Option<String>,
    pub photos: Vec<String>,
}

/// Form fields extracted from a listing's edit page.
#[derive(Debug, Clone)]
pub struct EditFormData {
    pub csrf_token: String,
    /// The form field name for the token (e.g. "validation", "csrfmiddlewaretoken")
    pub csrf_field_name: String,
    pub fields: Vec<(String, String)>,
}

/// Detailed information extracted from a listing's edit page.
#[derive(Debug, Clone)]
pub struct XmrListingDetail {
    pub description: Option<String>,
    pub photos: Vec<String>,
    pub price: Option<f64>,
    pub currency: Option<String>,
}

/// A sale record parsed from the /my-purchases/ page.
#[derive(Debug, Clone)]
pub struct XmrSaleRecord {
    pub order_id: String,
    pub listing_id: String,
    pub title: String,
    pub quantity: i64,
    pub status: String,
    pub completed_at: Option<String>,
}

/// Describes a stock-mode field discovered in a listing edit form.
#[derive(Debug, Clone)]
pub struct StockModeField {
    pub field_name: String,
    pub unlimited_value: String,
    pub onetime_value: String,
}
