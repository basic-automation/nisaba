use ratatui::prelude::*;
use ratatui::widgets::*;

use nisaba_core::types::Platform;

use crate::app::{App, ProductsView};

pub fn render(f: &mut Frame, app: &App, area: Rect) {
    match app.products_view {
        ProductsView::Mapped => render_mapped(f, app, area),
        ProductsView::Unmapped => render_unmapped(f, app, area),
    }

    // Render import popup on top if active
    if app.import_popup {
        render_import_popup(f, app);
    }
}

fn render_mapped(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Search / header
            Constraint::Min(0),   // Products table
            Constraint::Length(2), // Help line
        ])
        .split(area);

    // Search bar
    let search_text = if app.searching {
        format!(" Search: {}_ ", app.search_query)
    } else if !app.search_query.is_empty() {
        format!(" Filter: {} ", app.search_query)
    } else {
        " Mapped Products ".to_string()
    };

    let search = Paragraph::new(search_text).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Products (v=toggle view) "),
    );
    f.render_widget(search, chunks[0]);

    // Products table with platform mapping indicators
    let products = app.filtered_products();

    let rows: Vec<Row> = products
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let selected = i == app.products_selected;
            let row_style = if selected {
                Style::default().bg(Color::DarkGray)
            } else {
                Style::default()
            };

            // Look up actual mappings for this product
            let mappings = app.product_mappings.get(&p.id);

            let has_ebay = mappings
                .map(|m| m.iter().any(|pm| pm.platform == Platform::Ebay))
                .unwrap_or(false);
            let has_sq = mappings
                .map(|m| m.iter().any(|pm| pm.platform == Platform::Squarespace))
                .unwrap_or(false);
            let has_xmr = mappings
                .map(|m| m.iter().any(|pm| pm.platform == Platform::XmrBazaar))
                .unwrap_or(false);

            let check = |b: bool| if b { "Y" } else { "-" };

            Row::new(vec![
                Cell::from(format!(" {}", p.name)),
                Cell::from(p.canonical_sku.clone()),
                Cell::from(p.quantity.to_string()),
                Cell::from(check(has_ebay)),
                Cell::from(check(has_sq)),
                Cell::from(check(has_xmr)),
            ])
            .style(row_style)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Min(20),
            Constraint::Length(12),
            Constraint::Length(6),
            Constraint::Length(6),
            Constraint::Length(6),
            Constraint::Length(6),
        ],
    )
    .header(
        Row::new(vec!["Name", "SKU", "Qty", "eBay", "SqSp", "XMR"])
            .style(Style::default().add_modifier(Modifier::BOLD)),
    )
    .block(Block::default().borders(Borders::ALL));

    f.render_widget(table, chunks[1]);

    // Help line
    let help_text = if app.link_mode {
        " LINK MODE: v=unmapped view  select listing  Enter=link | Esc=cancel "
    } else {
        " j/k=navigate  v=unmapped  a=add mapping  d=delete  /=search  f=fetch "
    };
    let help = Paragraph::new(help_text).style(Style::default().fg(Color::DarkGray));
    f.render_widget(help, chunks[2]);
}

fn render_unmapped(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header
            Constraint::Min(0),   // Three-column layout
            Constraint::Length(2), // Help
        ])
        .split(area);

    let mode_label = if app.link_mode {
        format!(
            " LINK MODE - Select listing to link | {} unmapped ",
            app.unmapped_count
        )
    } else {
        format!(
            " Unmapped Listings ({} total) - Press f to fetch ",
            app.unmapped_count
        )
    };

    let header = Paragraph::new(mode_label).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Unmapped (v=toggle view, h/l=columns) "),
    );
    f.render_widget(header, chunks[0]);

    // Three-column layout for platforms
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(33),
            Constraint::Percentage(34),
            Constraint::Percentage(33),
        ])
        .split(chunks[1]);

    let platforms = app.platform_columns();
    let platform_labels = ["eBay", "Squarespace", "XMR Bazaar"];

    for (col_idx, (platform, col_area)) in platforms.iter().zip(cols.iter()).enumerate() {
        let is_selected_col = col_idx == app.unmapped_col;

        let listings = app
            .unmapped_listings
            .get(platform)
            .map(|v| v.as_slice())
            .unwrap_or(&[]);

        let border_style = if is_selected_col {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        };

        let title = format!(
            " {} ({}) ",
            platform_labels[col_idx],
            listings.len()
        );

        if listings.is_empty() {
            let placeholder = Paragraph::new(" No unmapped listings\n (press f to fetch)")
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(border_style)
                        .title(title),
                )
                .style(Style::default().fg(Color::DarkGray));
            f.render_widget(placeholder, *col_area);
        } else {
            let items: Vec<ListItem> = listings
                .iter()
                .enumerate()
                .map(|(i, listing)| {
                    let is_selected = is_selected_col && i == app.unmapped_row;
                    let sku_str = listing
                        .sku
                        .as_deref()
                        .map(|s| format!(" [{}]", s))
                        .unwrap_or_default();
                    let price_str = listing
                        .price
                        .map(|p| format!(" ${:.2}", p))
                        .unwrap_or_default();
                    let text = format!(
                        " {} qty:{}{}{} ",
                        truncate_str(&listing.title, 30),
                        listing.quantity,
                        sku_str,
                        price_str,
                    );
                    let style = if is_selected {
                        Style::default().bg(Color::DarkGray).fg(Color::White)
                    } else {
                        Style::default()
                    };
                    ListItem::new(text).style(style)
                })
                .collect();

            let list = List::new(items).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(border_style)
                    .title(title),
            );
            f.render_widget(list, *col_area);
        }
    }

    let help_text = if app.link_mode {
        " h/l=column  j/k=navigate  Enter=link to product  Esc=cancel "
    } else {
        " h/l=column  j/k=navigate  Enter=import as product  f=fetch  v=mapped  Esc=back "
    };
    let help = Paragraph::new(help_text).style(Style::default().fg(Color::DarkGray));
    f.render_widget(help, chunks[2]);
}

fn render_import_popup(f: &mut Frame, app: &App) {
    let area = f.area();
    let popup_width = 60.min(area.width.saturating_sub(4));
    let popup_height = 10.min(area.height.saturating_sub(4));

    let popup_area = Rect {
        x: (area.width - popup_width) / 2,
        y: (area.height - popup_height) / 2,
        width: popup_width,
        height: popup_height,
    };

    // Clear the popup area
    f.render_widget(Clear, popup_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .title(" Import as New Product ");

    let inner = block.inner(popup_area);
    f.render_widget(block, popup_area);

    let fields = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Instructions
            Constraint::Length(1), // Spacer
            Constraint::Length(1), // Name label + value
            Constraint::Length(1), // SKU label + value
            Constraint::Length(1), // Spacer
            Constraint::Length(1), // Help
        ])
        .split(inner);

    let instructions =
        Paragraph::new(" Edit name and SKU, then press Enter to create");
    f.render_widget(instructions, fields[0]);

    let name_style = if app.import_field == 0 {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    let sku_style = if app.import_field == 1 {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };

    let name_cursor = if app.import_field == 0 { "_" } else { "" };
    let sku_cursor = if app.import_field == 1 { "_" } else { "" };

    let name_line = Paragraph::new(format!(" Name: {}{}", app.import_name, name_cursor))
        .style(name_style);
    f.render_widget(name_line, fields[2]);

    let sku_line = Paragraph::new(format!(" SKU:  {}{}", app.import_sku, sku_cursor))
        .style(sku_style);
    f.render_widget(sku_line, fields[3]);

    let help = Paragraph::new(" Tab=switch field  Enter=create  Esc=cancel")
        .style(Style::default().fg(Color::DarkGray));
    f.render_widget(help, fields[5]);
}

fn truncate_str(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_len.saturating_sub(3)).collect();
        format!("{}...", truncated)
    }
}
