use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::app::App;

pub fn render(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),   // Config fields
            Constraint::Length(2), // Help
        ])
        .split(area);

    let rows: Vec<Row> = app
        .config_fields
        .iter()
        .enumerate()
        .map(|(i, field)| {
            let selected = i == app.config_selected;
            let row_style = if selected {
                Style::default().bg(Color::DarkGray)
            } else {
                Style::default()
            };

            let display_value = if field.secret && !app.config_editing {
                if field.value.is_empty() {
                    "(not set)".to_string()
                } else {
                    "********".to_string()
                }
            } else if selected && app.config_editing {
                format!("{}|", field.value) // cursor indicator
            } else {
                field.value.clone()
            };

            Row::new(vec![
                Cell::from(format!(" {}", field.label)),
                Cell::from(display_value),
            ])
            .style(row_style)
        })
        .collect();

    let table = Table::new(
        rows,
        [Constraint::Length(24), Constraint::Min(30)],
    )
    .header(
        Row::new(vec!["Setting", "Value"])
            .style(Style::default().add_modifier(Modifier::BOLD)),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Configuration "),
    );

    f.render_widget(table, chunks[0]);

    let help_text = if app.config_editing {
        " Type to edit  Enter=save  Esc=cancel "
    } else {
        " j/k=navigate  Enter=edit  Changes saved to config.toml "
    };

    let help = Paragraph::new(help_text).style(Style::default().fg(Color::DarkGray));
    f.render_widget(help, chunks[1]);
}
