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
    let conn = db.connect()?;

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
