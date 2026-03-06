-- Create a default variant for every simple product that has no variants
INSERT INTO product_variants (id, product_id, sku, name, attributes_json, quantity, sort_order)
SELECT
  lower(hex(randomblob(4)) || '-' || hex(randomblob(2)) || '-4' ||
        substr(hex(randomblob(2)),2) || '-' ||
        substr('89ab', abs(random()) % 4 + 1, 1) ||
        substr(hex(randomblob(2)),2) || '-' || hex(randomblob(6))),
  id, canonical_sku, 'Default', '{}', quantity, 0
FROM products
WHERE has_variants = 0
  AND id NOT IN (SELECT DISTINCT product_id FROM product_variants);

UPDATE products SET has_variants = 1, updated_at = datetime('now') WHERE has_variants = 0;
