pub mod config;
pub mod dashboard;
pub mod inventory;
pub mod logs;
pub mod products;

use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::app::{App, Tab};

/// Render the full application frame.
pub fn render(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Tab bar
            Constraint::Min(0),   // Content
            Constraint::Length(1), // Status bar
        ])
        .split(f.area());

    render_tabs(f, app, chunks[0]);

    match app.tab {
        Tab::Dashboard => dashboard::render(f, app, chunks[1]),
        Tab::Products => products::render(f, app, chunks[1]),
        Tab::Inventory => inventory::render(f, app, chunks[1]),
        Tab::Config => config::render(f, app, chunks[1]),
        Tab::Logs => logs::render(f, app, chunks[1]),
    }

    render_status_bar(f, app, chunks[2]);
}

fn render_tabs(f: &mut Frame, app: &App, area: Rect) {
    let tab_titles: Vec<Line> = [
        Tab::Dashboard,
        Tab::Products,
        Tab::Inventory,
        Tab::Config,
        Tab::Logs,
    ]
    .iter()
    .enumerate()
    .map(|(i, t)| {
        let mut label = format!(" {} {} ", i + 1, t.label());
        if *t == Tab::Products && app.unmapped_count > 0 {
            label.push_str(&format!("({})", app.unmapped_count));
        }
        Line::from(label)
    })
    .collect();

    let tabs = Tabs::new(tab_titles)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Nisaba ─ Inventory Sync "),
        )
        .select(app.tab.index())
        .style(Style::default().fg(Color::White))
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );

    f.render_widget(tabs, area);
}

fn render_status_bar(f: &mut Frame, app: &App, area: Rect) {
    let sync_status = if app.sync_paused {
        "PAUSED"
    } else {
        "RUNNING"
    };

    let last = app.last_sync.as_deref().unwrap_or("never");

    let msg = app.status_message.as_deref().unwrap_or("");

    let text = format!(
        " Sync: {} | Last: {} | {msg} | q=quit  s=sync  p=pause  F5=refresh  1-5=tabs  Tab=next",
        sync_status, last
    );

    let bar = Paragraph::new(text).style(
        Style::default()
            .bg(Color::DarkGray)
            .fg(Color::White),
    );

    f.render_widget(bar, area);
}
