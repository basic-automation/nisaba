use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::app::App;
use nisaba_core::types::Platform;

pub fn render(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5), // Platform status
            Constraint::Min(0),   // Products table
            Constraint::Length(1), // Help
        ])
        .split(area);

    render_platform_status(f, app, chunks[0]);
    render_products_table(f, app, chunks[1]);

    let help = Paragraph::new(" j/k=navigate product list")
        .style(Style::default().fg(Color::DarkGray));
    f.render_widget(help, chunks[2]);
}

fn render_platform_status(f: &mut Frame, app: &App, area: Rect) {
    let platforms = [Platform::Ebay, Platform::Squarespace, Platform::XmrBazaar];

    let rows: Vec<Row> = platforms
        .iter()
        .map(|p| {
            let status = app.platform_status.get(p);
            let connected = status.map(|s| s.connected).unwrap_or(false);
            let items = status.map(|s| s.items_count).unwrap_or(0);
            let error = status
                .and_then(|s| s.last_error.as_deref())
                .unwrap_or("-");

            let status_str = if connected { "OK" } else { "---" };
            let status_style = if connected {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            Row::new(vec![
                Cell::from(format!(" {}", p)),
                Cell::from(status_str).style(status_style),
                Cell::from(items.to_string()),
                Cell::from(error.to_string()),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(16),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Min(20),
        ],
    )
    .header(
        Row::new(vec!["Platform", "Status", "Items", "Last Error"])
            .style(Style::default().add_modifier(Modifier::BOLD))
            .bottom_margin(0),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Platform Status "),
    );

    f.render_widget(table, area);
}

fn render_products_table(f: &mut Frame, app: &App, area: Rect) {
    let products = app.filtered_products();

    let rows: Vec<Row> = products
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let low = p
                .low_stock_threshold
                .unwrap_or(app.config.alerts.default_low_stock_threshold);
            let qty_style = if p.quantity <= low {
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
            } else if p.quantity <= low * 2 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            };

            let selected = i == app.dashboard_selected;
            let row_style = if selected {
                Style::default().bg(Color::DarkGray)
            } else {
                Style::default()
            };

            Row::new(vec![
                Cell::from(format!(" {}", p.name)),
                Cell::from(p.canonical_sku.clone()),
                Cell::from(p.quantity.to_string()).style(qty_style),
                Cell::from(if p.is_tracked { "Yes" } else { "No" }),
            ])
            .style(row_style)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Min(20),
            Constraint::Length(14),
            Constraint::Length(8),
            Constraint::Length(8),
        ],
    )
    .header(
        Row::new(vec!["Product", "SKU", "Qty", "Tracked"])
            .style(Style::default().add_modifier(Modifier::BOLD))
            .bottom_margin(0),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" Products ({}) ", products.len())),
    );

    f.render_widget(table, area);
}
