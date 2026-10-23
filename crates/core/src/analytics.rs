use crate::db::Db;
use crate::error::SyncError;
use crate::types::{Platform, ProductAnalytics, TimeWindow};

/// Compute analytics for a single product over a given time window.
pub async fn compute_product_analytics(
    db: &Db,
    product_id: &str,
    window: TimeWindow,
) -> Result<ProductAnalytics, SyncError> {
    let product = db
        .get_product(product_id)
        .await?
        .ok_or_else(|| SyncError::Other(format!("Product {product_id} not found")))?;

    let history = db
        .get_inventory_history(product_id, window.sql_interval())
        .await?;

    let history_points: Vec<(String, i64)> = history
        .iter()
        .map(|e| (e.recorded_at.clone(), e.quantity))
        .collect();

    // Calculate sales velocity: total decrease in quantity / number of days
    let sales_velocity = if history.len() >= 2 {
        let first_qty = history.first().map(|e| e.quantity).unwrap_or(0);
        let last_qty = history.last().map(|e| e.quantity).unwrap_or(0);
        let total_decrease = first_qty - last_qty; // positive if stock went down

        let days = match window {
            TimeWindow::Day => 1.0,
            TimeWindow::Week => 7.0,
            TimeWindow::Month => 30.0,
            TimeWindow::All => {
                // Estimate from the actual time span
                let count = history.len() as f64;
                // Rough estimate: assume ~288 data points per day (every 5 min)
                (count / 288.0).max(1.0)
            }
        };

        if total_decrease > 0 {
            total_decrease as f64 / days
        } else {
            0.0
        }
    } else {
        0.0
    };

    // Platform-level sales breakdown from sync_events
    let platform_sales = compute_platform_sales(db, product_id, window).await?;

    Ok(ProductAnalytics {
        product_id: product.id,
        product_name: product.name,
        current_quantity: product.quantity,
        low_stock_threshold: product.low_stock_threshold,
        history: history_points,
        sales_velocity,
        platform_sales,
    })
}

/// Compute per-platform sales (total units pushed) from sync_events.
async fn compute_platform_sales(
    db: &Db,
    product_id: &str,
    window: TimeWindow,
) -> Result<Vec<(Platform, i64)>, SyncError> {
    let conn = db.connect().await?;

    let (sql, use_interval) = match window.sql_interval() {
        Some(_interval) => (
            "SELECT source_platform, SUM(old_quantity - new_quantity) as sold
             FROM sync_events
             WHERE product_id = ?1
               AND event_type = 'quantity_change'
               AND old_quantity > new_quantity
               AND created_at >= datetime('now', ?2)
             GROUP BY source_platform",
            true,
        ),
        None => (
            "SELECT source_platform, SUM(old_quantity - new_quantity) as sold
             FROM sync_events
             WHERE product_id = ?1
               AND event_type = 'quantity_change'
               AND old_quantity > new_quantity
             GROUP BY source_platform",
            false,
        ),
    };

    let mut rows = if use_interval {
        conn.query(
            sql,
            turso::params![product_id, window.sql_interval().unwrap_or("")],
        )
        .await?
    } else {
        conn.query(sql, turso::params![product_id]).await?
    };

    let mut result = Vec::new();
    while let Some(row) = rows.next().await? {
        let platform_str = row.get::<String>(0)?;
        let sold = row.get::<i64>(1)?;
        if let Some(platform) = Platform::from_str_loose(&platform_str) {
            result.push((platform, sold));
        }
    }

    Ok(result)
}

/// Compute analytics for all tracked products.
pub async fn compute_all_analytics(
    db: &Db,
    window: TimeWindow,
) -> Result<Vec<ProductAnalytics>, SyncError> {
    let products = db.list_products().await?;
    let mut all_analytics = Vec::new();

    for product in products {
        if product.is_tracked {
            let analytics = compute_product_analytics(db, &product.id, window).await?;
            all_analytics.push(analytics);
        }
    }

    Ok(all_analytics)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{NewSyncEvent, Product};

    async fn test_db() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path().join("analytics-test.db").to_str().unwrap())
            .await
            .unwrap();
        db.migrate().await.unwrap();
        (dir, db)
    }

    async fn insert_tracked_product(db: &Db, id: &str) {
        db.insert_product(&Product {
            id: id.to_string(),
            canonical_sku: format!("SKU-{id}"),
            name: format!("Product {id}"),
            quantity: 10,
            low_stock_threshold: None,
            is_tracked: true,
            has_variants: false,
            created_at: String::new(),
            updated_at: String::new(),
        })
        .await
        .unwrap();
    }

    async fn insert_quantity_change(
        db: &Db,
        product_id: &str,
        source_platform: &str,
        old_quantity: i64,
        new_quantity: i64,
    ) {
        db.insert_sync_event(&NewSyncEvent {
            product_id,
            source_platform,
            target_platform: source_platform,
            old_quantity,
            new_quantity,
            event_type: "quantity_change",
            sync_cycle_id: "c1",
            error_message: None,
        })
        .await
        .unwrap();
    }

    fn sorted_sales(sales: &[(Platform, i64)]) -> Vec<(String, i64)> {
        let mut out: Vec<(String, i64)> = sales
            .iter()
            .map(|(p, s)| (p.as_str().to_string(), *s))
            .collect();
        out.sort_unstable();
        out
    }

    #[tokio::test]
    async fn engine_written_events_group_per_platform() {
        // N-11.8: the sync engine records the real platform on the events
        // it writes, so the breakdown is non-empty and grouped per platform.
        let (_dir, db) = test_db().await;
        insert_tracked_product(&db, "p1").await;

        insert_quantity_change(&db, "p1", "ebay", 10, 7).await; // 3 sold
        insert_quantity_change(&db, "p1", "ebay", 7, 5).await; // 2 sold
        insert_quantity_change(&db, "p1", "amazon", 10, 6).await; // 4 sold

        let analytics = compute_product_analytics(&db, "p1", TimeWindow::All)
            .await
            .unwrap();
        assert_eq!(
            sorted_sales(&analytics.platform_sales),
            [("amazon".to_string(), 4), ("ebay".to_string(), 5),]
        );
    }

    #[tokio::test]
    async fn legacy_engine_sentinel_rows_are_excluded_from_the_breakdown() {
        // Rows written before N-11.8 carry the "sync_engine" sentinel as
        // source_platform; `from_str_loose` cannot map it, so they must be
        // dropped rather than attributed to a bogus platform.
        let (_dir, db) = test_db().await;
        insert_tracked_product(&db, "p1").await;

        insert_quantity_change(&db, "p1", "sync_engine", 10, 4).await;
        insert_quantity_change(&db, "p1", "ebay", 10, 8).await;

        let analytics = compute_product_analytics(&db, "p1", TimeWindow::All)
            .await
            .unwrap();
        assert_eq!(
            sorted_sales(&analytics.platform_sales),
            [("ebay".to_string(), 2)]
        );
    }

    #[tokio::test]
    async fn non_sales_events_do_not_feed_the_breakdown() {
        // Restocks (old < new) and non-quantity_change events carry no
        // units-sold, so the breakdown stays empty.
        let (_dir, db) = test_db().await;
        insert_tracked_product(&db, "p1").await;

        // Restock: quantity went up.
        insert_quantity_change(&db, "p1", "ebay", 4, 10).await;
        // push_error: not a quantity_change.
        db.insert_sync_event(&NewSyncEvent {
            product_id: "p1",
            source_platform: "ebay",
            target_platform: "ebay",
            old_quantity: 10,
            new_quantity: 10,
            event_type: "push_error",
            sync_cycle_id: "c1",
            error_message: Some("boom"),
        })
        .await
        .unwrap();

        let analytics = compute_product_analytics(&db, "p1", TimeWindow::All)
            .await
            .unwrap();
        assert!(analytics.platform_sales.is_empty());
    }
}
