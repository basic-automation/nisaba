ALTER TABLE vendor_plugin_registry ADD COLUMN category TEXT NOT NULL DEFAULT 'vendor';
ALTER TABLE vendor_plugin_registry ADD COLUMN icon TEXT;
ALTER TABLE vendor_plugin_registry ADD COLUMN files_json TEXT;
UPDATE vendor_plugin_registry SET files_json = json_object('index.ts', code) WHERE code IS NOT NULL AND code != '';
