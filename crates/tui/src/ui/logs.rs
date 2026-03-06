use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::app::App;

pub fn render(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Filter bar
            Constraint::Min(0),   // Log content
            Constraint::Length(2), // Help
        ])
        .split(area);

    // Filter bar
    let filter_text = match &app.log_level_filter {
        Some(level) => format!(" Filter: {} ", level),
        None => " All Levels ".to_string(),
    };

    let filter_bar = Paragraph::new(filter_text).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Log Filters "),
    );
    f.render_widget(filter_bar, chunks[0]);

    // Log content — use tui-logger widget
    let tui_log = tui_logger::TuiLoggerSmartWidget::default()
        .style_error(Style::default().fg(Color::Red))
        .style_warn(Style::default().fg(Color::Yellow))
        .style_info(Style::default().fg(Color::Cyan))
        .style_debug(Style::default().fg(Color::Green))
        .style_trace(Style::default().fg(Color::DarkGray))
        .border_type(BorderType::Rounded)
        .title_log(" Logs ");

    f.render_widget(tui_log, chunks[1]);

    // Help
    let help = Paragraph::new(
        " PageUp/PageDown=scroll  0=show all levels ",
    )
    .style(Style::default().fg(Color::DarkGray));
    f.render_widget(help, chunks[2]);
}
