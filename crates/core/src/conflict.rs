use crate::types::{QuantityDelta, ResolvedQuantity};

/// Resolve quantity conflicts for a single product by summing all platform deltas.
///
/// Each delta represents an independent real-world change (e.g., a sale on eBay and
/// a sale on Squarespace happening in the same sync cycle). Summing them gives the
/// total change to apply to the canonical quantity.
pub fn resolve_deltas(
    product_id: &str,
    canonical_quantity: i64,
    deltas: Vec<QuantityDelta>,
) -> ResolvedQuantity {
    let total_delta: i64 = deltas.iter().map(|d| d.delta).sum();
    let new_canonical = (canonical_quantity + total_delta).max(0);

    ResolvedQuantity {
        product_id: product_id.to_string(),
        old_canonical: canonical_quantity,
        new_canonical,
        total_delta,
        deltas,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Platform;

    #[test]
    fn test_single_platform_decrease() {
        let deltas = vec![QuantityDelta {
            product_id: "p1".into(),
            platform: Platform::Ebay,
            previous: 10,
            current: 8,
            delta: -2,
        }];

        let result = resolve_deltas("p1", 10, deltas);
        assert_eq!(result.new_canonical, 8);
        assert_eq!(result.total_delta, -2);
    }

    #[test]
    fn test_multi_platform_conflict() {
        // eBay sold 2, Squarespace sold 1 in same cycle
        let deltas = vec![
            QuantityDelta {
                product_id: "p1".into(),
                platform: Platform::Ebay,
                previous: 10,
                current: 8,
                delta: -2,
            },
            QuantityDelta {
                product_id: "p1".into(),
                platform: Platform::Squarespace,
                previous: 10,
                current: 9,
                delta: -1,
            },
        ];

        let result = resolve_deltas("p1", 10, deltas);
        assert_eq!(result.new_canonical, 7); // 10 + (-2) + (-1) = 7
        assert_eq!(result.total_delta, -3);
    }

    #[test]
    fn test_no_changes() {
        let deltas = vec![];
        let result = resolve_deltas("p1", 10, deltas);
        assert_eq!(result.new_canonical, 10);
        assert_eq!(result.total_delta, 0);
    }

    #[test]
    fn test_quantity_floor_at_zero() {
        let deltas = vec![QuantityDelta {
            product_id: "p1".into(),
            platform: Platform::Ebay,
            previous: 2,
            current: 0,
            delta: -5, // oversell scenario
        }];

        let result = resolve_deltas("p1", 2, deltas);
        assert_eq!(result.new_canonical, 0); // floored at 0
    }

    #[test]
    fn test_restock_increase() {
        let deltas = vec![QuantityDelta {
            product_id: "p1".into(),
            platform: Platform::Ebay,
            previous: 5,
            current: 15,
            delta: 10,
        }];

        let result = resolve_deltas("p1", 5, deltas);
        assert_eq!(result.new_canonical, 15);
        assert_eq!(result.total_delta, 10);
    }

    #[test]
    fn test_mixed_increase_and_decrease() {
        // Manual restock on eBay (+10), sale on Squarespace (-1)
        let deltas = vec![
            QuantityDelta {
                product_id: "p1".into(),
                platform: Platform::Ebay,
                previous: 5,
                current: 15,
                delta: 10,
            },
            QuantityDelta {
                product_id: "p1".into(),
                platform: Platform::Squarespace,
                previous: 5,
                current: 4,
                delta: -1,
            },
        ];

        let result = resolve_deltas("p1", 5, deltas);
        assert_eq!(result.new_canonical, 14); // 5 + 10 + (-1) = 14
        assert_eq!(result.total_delta, 9);
    }
}
