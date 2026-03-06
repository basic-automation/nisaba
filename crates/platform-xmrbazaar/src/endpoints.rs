use nisaba_core::config::EndpointConfig;
use std::collections::HashMap;

/// Provides endpoint URLs and methods, with config-driven overrides.
pub struct Endpoints {
    base_url: String,
    overrides: HashMap<String, EndpointConfig>,
}

impl Endpoints {
    pub fn new(base_url: String, overrides: HashMap<String, EndpointConfig>) -> Self {
        Self {
            base_url,
            overrides,
        }
    }

    fn get_config(&self, name: &str) -> (&str, String) {
        if let Some(cfg) = self.overrides.get(name) {
            (cfg.method.as_str(), format!("{}{}", self.base_url, cfg.path))
        } else {
            let (method, path) = Self::defaults(name);
            (method, format!("{}{}", self.base_url, path))
        }
    }

    fn defaults(name: &str) -> (&'static str, &'static str) {
        match name {
            "login" => ("POST", "/login/"),
            "my_listings" => ("GET", "/my-listings/"),
            "my_sales" => ("GET", "/my-sales/"),
            "edit_listing" => ("POST", "/edit-listing/{id}/"),
            "listing" => ("GET", "/listing/{id}/"),
            "new_listing" => ("POST", "/new-listing/selling/sell-product/"),
            _ => ("GET", "/"),
        }
    }

    pub fn login(&self) -> (&str, String) {
        self.get_config("login")
    }

    pub fn my_listings(&self) -> (&str, String) {
        self.get_config("my_listings")
    }

    pub fn my_sales(&self) -> (&str, String) {
        self.get_config("my_sales")
    }

    pub fn edit_listing(&self, id: &str) -> (&str, String) {
        let (method, url) = self.get_config("edit_listing");
        (method, url.replace("{id}", id))
    }

    pub fn listing(&self, id: &str) -> (&str, String) {
        let (method, url) = self.get_config("listing");
        (method, url.replace("{id}", id))
    }

    pub fn new_listing(&self) -> (&str, String) {
        self.get_config("new_listing")
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }
}
