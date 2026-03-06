use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::app::App;

pub fn render(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header with time window
            Constraint::Min(0),   // Content
            Constraint::Length(2), // Help
        ])
        .split(area);

    // Header
    let header = Paragraph::new(format!(
        " Inventory Analytics ─ Window: {} ",
        app.inventory_time_window.label()
    ))
    .block(Block::default().borders(Borders::ALL).title(" Inventory "));
    f.render_widget(header, chunks[0]);

    if app.analytics.is_empty() {
        let empty = Paragraph::new("\n  No data yet. Run a sync cycle to start collecting history.")
            .block(Block::default().borders(Borders::ALL))
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(empty, chunks[1]);
    } else {
        // Split content into table + sparklines
        let content_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(50), // Products table
                Constraint::Percentage(50), // Sparklines / detail
            ])
            .split(chunks[1]);

        render_analytics_table(f, app, content_chunks[0]);
        render_sparklines(f, app, content_chunks[1]);
    }

    // Help
    let help = Paragraph::new(
        " j/k=select  d=cycle window (24h/7d/30d/all)  Enter=expand detail ",
    )
    .style(Style::default().fg(Color::DarkGray));
    f.render_widget(help, chunks[2]);
}

fn render_analytics_table(f: &mut Frame, app: &App, area: Rect) {
    let rows: Vec<Row> = app
        .analytics
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let selected = i == app.inventory_selected;
            let low = a.low_stock_threshold.unwrap_or(
                app.config.alerts.default_low_stock_threshold,
            );

            let qty_style = if a.current_quantity <= low {
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            let alert = if a.current_quantity <= low {
                "LOW"
            } else {
                ""
            };

            let row_style = if selected {
                Style::default().bg(Color::DarkGray)
            } else {
                Style::default()
            };

            Row::new(vec![
                Cell::from(format!(" {}", a.product_name)),
                Cell::from(a.current_quantity.to_string()).style(qty_style),
                Cell::from(format!("{:.1}/day", a.sales_velocity)),
                Cell::from(alert).style(Style::default().fg(Color::Red)),
            ])
            .style(row_style)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Min(16),
            Constraint::Length(6),
            Constraint::Length(10),
            Constraint::Length(5),
        ],
    )
    .header(
        Row::new(vec!["Product", "Qty", "Velocity", "Alert"])
            .style(Style::default().add_modifier(Modifier::BOLD)),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Stock Levels "),
    );

    f.render_widget(table, area);
}

fn render_sparklines(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Trends ");

    let inner = block.inner(area);
    f.render_widget(block, area);

    if let Some(analytics) = app.analytics.get(app.inventory_selected) {
        if analytics.history.is_empty() {
            let msg = Paragraph::new("  No history data");
            f.render_widget(msg, inner);
            return;
        }

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Sparkline
                Constraint::Min(0),   // Platform breakdown
            ])
            .split(inner);

        // Sparkline of quantity over time
        let data: Vec<u64> = analytics
            .history
            .iter()
            .map(|(_, q)| (*q).max(0) as u64)
            .collect();

        let sparkline = Sparkline::default()
            .data(&data)
            .style(Style::default().fg(Color::Cyan))
            .block(Block::default().title(format!(" {} ", analytics.product_name)));
        f.render_widget(sparkline, chunks[0]);

        // Platform sales breakdown
        let breakdown_rows: Vec<Row> = analytics
            .platform_sales
            .iter()
            .map(|(platform, sold)| {
                Row::new(vec![
                    Cell::from(format!("  {}", platform)),
                    Cell::from(format!("{} sold", sold)),
                ])
            })
            .collect();

        if !breakdown_rows.is_empty() {
            let breakdown = Table::new(
                breakdown_rows,
                [Constraint::Length(16), Constraint::Min(10)],
            )
            .header(
                Row::new(vec!["Platform", "Sales"])
                    .style(Style::default().add_modifier(Modifier::BOLD)),
            );
            f.render_widget(breakdown, chunks[1]);
        }
    }
}
