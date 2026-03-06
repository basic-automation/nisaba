mod app;
mod events;
mod ui;

use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{Event, KeyCode, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::prelude::*;
use tokio::sync::mpsc;
use tracing_subscriber::prelude::*;

use nisaba_core::config::AppConfig;
use nisaba_core::db::Db;
use nisaba_core::sync_engine::{SyncCommand, SyncEngine, SyncEngineEvent};
use nisaba_core::traits::PlatformAdapter;
use nisaba_core::types::Platform;

use crate::app::{App, ProductsView, Tab};
use crate::events::{spawn_sync_event_bridge, spawn_terminal_event_reader, spawn_tick, AppEvent};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tui-logger for capturing log events
    tui_logger::init_logger(tui_logger::LevelFilter::Debug).unwrap();
    tui_logger::set_default_level(tui_logger::LevelFilter::Info);

    // Suppress noisy internal crate logging
    tui_logger::set_level_for_target("turso_core", tui_logger::LevelFilter::Warn);
    tui_logger::set_level_for_target("turso", tui_logger::LevelFilter::Warn);
    tui_logger::set_level_for_target("hyper_util", tui_logger::LevelFilter::Warn);
    tui_logger::set_level_for_target("reqwest", tui_logger::LevelFilter::Warn);
    tui_logger::set_level_for_target("h2", tui_logger::LevelFilter::Warn);
    tui_logger::set_level_for_target("rustls", tui_logger::LevelFilter::Warn);

    // Initialize tracing with tui-logger as a layer
    tracing_subscriber::registry()
        .with(tui_logger::tracing_subscriber_layer())
        .init();

    // Load config — check CLI arg, then cwd, then %APPDATA%/nisaba/ (creates default if missing)
    let explicit = std::env::args().nth(1);
    let (config, config_path) = AppConfig::resolve_and_load(explicit.as_deref())
        .context("Failed to load config")?;

    // Open database
    let db = Arc::new(
        Db::open(&config.database.path)
            .await
            .context("Failed to open database")?,
    );
    db.migrate().await.context("Failed to run migrations")?;

    // Build platform adapters (shared so TUI can rebuild on config change)
    let shared_adapters: nisaba_core::sync_engine::SharedAdapters =
        Arc::new(tokio::sync::RwLock::new(
            build_adapters(&config, db.clone()).await?,
        ));

    // Set up channels
    let (sync_event_tx, sync_event_rx) = mpsc::channel::<SyncEngineEvent>(64);
    let (cmd_tx, cmd_rx) = mpsc::channel::<SyncCommand>(16);
    let (app_event_tx, mut app_event_rx) = mpsc::channel::<AppEvent>(128);

    // Spawn sync engine
    let engine = SyncEngine::new(
        db.clone(),
        shared_adapters.clone(),
        sync_event_tx,
        config.general.max_retries,
    );
    tokio::spawn(async move {
        engine.run(cmd_rx, 300).await;
    });

    // Spawn event sources
    spawn_terminal_event_reader(app_event_tx.clone());
    spawn_sync_event_bridge(sync_event_rx, app_event_tx.clone());
    spawn_tick(app_event_tx.clone(), Duration::from_millis(250));

    // Create app state
    let mut app = App::new(config, config_path.clone(), shared_adapters.clone(), db, cmd_tx, app_event_tx.clone());

    // Initial data load
    app.refresh_products().await;

    // Set up panic hook to restore terminal on crash
    let default_panic = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        default_panic(info);
    }));

    // Set up terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Main loop
    while app.running {
        terminal.draw(|f| ui::render(f, &app))?;

        if let Some(event) = app_event_rx.recv().await {
            match event {
                AppEvent::Terminal(Event::Key(key))
                    if key.kind == crossterm::event::KeyEventKind::Press =>
                {
                    handle_key(&mut app, key).await;
                }
                AppEvent::Terminal(_) => {} // Mouse, resize, etc.
                AppEvent::Sync(sync_event) => {
                    app.handle_sync_event(sync_event);
                }
                AppEvent::ListingsFetched(listings) => {
                    app.apply_fetched_listings(listings);
                }
                AppEvent::ListingsFetchError(msg) => {
                    app.status_message = Some(msg);
                }
                AppEvent::Tick => {
                    // Rebuild adapters if config changed
                    if app.needs_adapter_rebuild {
                        app.needs_adapter_rebuild = false;
                        let new_adapters = build_adapters(&app.config, app.db.clone()).await;
                        match new_adapters {
                            Ok(map) => {
                                *app.adapters.write().await = map;
                                let _ = app.cmd_tx.send(SyncCommand::Reload).await;
                                app.status_message =
                                    Some("Config saved, adapters reloaded".into());
                            }
                            Err(e) => {
                                app.status_message =
                                    Some(format!("Failed to rebuild adapters: {e}"));
                            }
                        }
                    }

                    // Act on deferred refresh flags
                    if app.needs_product_refresh {
                        app.needs_product_refresh = false;
                        app.refresh_products().await;
                        if app.tab == Tab::Inventory {
                            app.refresh_analytics().await;
                        }
                    }
                }
            }
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}

async fn handle_key(app: &mut App, key: crossterm::event::KeyEvent) {
    // Import popup intercepts all keys
    if app.import_popup {
        handle_import_popup_key(app, key).await;
        return;
    }

    // Config editing mode intercepts all keys
    if app.config_editing {
        handle_config_edit_key(app, key);
        return;
    }

    // Search mode intercepts keys
    if app.searching {
        handle_search_key(app, key);
        return;
    }

    // Global keybindings
    match key.code {
        KeyCode::Char('q') => {
            app.running = false;
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.running = false;
        }
        KeyCode::Char('s') => {
            let _ = app.cmd_tx.send(SyncCommand::SyncNow).await;
            app.status_message = Some("Sync triggered".into());
        }
        KeyCode::F(5) => {
            app.refresh_products().await;
            app.refresh_analytics().await;
            app.status_message = Some("Refreshed".into());
        }
        KeyCode::Char('p') => {
            app.sync_paused = !app.sync_paused;
            let cmd = if app.sync_paused {
                SyncCommand::Pause
            } else {
                SyncCommand::Resume
            };
            let _ = app.cmd_tx.send(cmd).await;
        }
        KeyCode::Char('1') => app.tab = Tab::Dashboard,
        KeyCode::Char('2') => app.tab = Tab::Products,
        KeyCode::Char('3') => {
            app.tab = Tab::Inventory;
            app.refresh_analytics().await;
        }
        KeyCode::Char('4') => app.tab = Tab::Config,
        KeyCode::Char('5') => app.tab = Tab::Logs,
        KeyCode::Tab => {
            app.tab = app.tab.next();
            if app.tab == Tab::Inventory {
                app.refresh_analytics().await;
            }
        }
        _ => {
            // Tab-specific keybindings
            match app.tab {
                Tab::Dashboard => handle_dashboard_key(app, key).await,
                Tab::Products => handle_products_key(app, key).await,
                Tab::Inventory => handle_inventory_key(app, key).await,
                Tab::Config => handle_config_key(app, key),
                Tab::Logs => handle_logs_key(app, key),
            }
        }
    }
}

async fn handle_dashboard_key(app: &mut App, key: crossterm::event::KeyEvent) {
    let count = app.filtered_products().len();
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            if app.dashboard_selected > 0 {
                app.dashboard_selected -= 1;
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if count > 0 && app.dashboard_selected < count - 1 {
                app.dashboard_selected += 1;
            }
        }
        _ => {}
    }
}

async fn handle_products_key(app: &mut App, key: crossterm::event::KeyEvent) {
    match app.products_view {
        ProductsView::Mapped => handle_products_mapped_key(app, key).await,
        ProductsView::Unmapped => handle_products_unmapped_key(app, key).await,
    }
}

async fn handle_products_mapped_key(app: &mut App, key: crossterm::event::KeyEvent) {
    match key.code {
        KeyCode::Char('v') => {
            app.products_view = ProductsView::Unmapped;
        }
        KeyCode::Char('/') => {
            app.searching = true;
            app.search_query.clear();
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if app.products_selected > 0 {
                app.products_selected -= 1;
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let count = app.filtered_products().len();
            if count > 0 && app.products_selected < count - 1 {
                app.products_selected += 1;
            }
        }
        KeyCode::Char('a') => {
            // Enter link mode — switch to unmapped view to pick a listing to link
            let selected = app.filtered_products()
                .get(app.products_selected)
                .map(|p| (p.id.clone(), p.name.clone()));
            if let Some((id, name)) = selected {
                app.link_mode = true;
                app.link_product_id = Some(id);
                app.products_view = ProductsView::Unmapped;
                app.status_message = Some(format!("Select a listing to link to '{name}'"));
            } else {
                app.status_message = Some("No product selected".into());
            }
        }
        KeyCode::Char('d') => {
            // Delete the selected product
            let selected = app.filtered_products()
                .get(app.products_selected)
                .map(|p| (p.id.clone(), p.name.clone()));
            if let Some((id, name)) = selected {
                if let Err(e) = app.db.delete_product(&id).await {
                    app.status_message = Some(format!("Failed to delete: {e}"));
                } else {
                    app.status_message = Some(format!("Deleted '{name}'"));
                    app.needs_product_refresh = true;
                    if app.products_selected > 0 {
                        app.products_selected -= 1;
                    }
                }
            }
        }
        KeyCode::Char('f') => {
            app.status_message = Some("Fetching listings from all platforms...".into());
            app.start_fetch_listings();
        }
        KeyCode::Esc => {
            if app.link_mode {
                app.link_mode = false;
                app.link_product_id = None;
                app.status_message = Some("Link mode cancelled".into());
            }
        }
        _ => {}
    }
}

async fn handle_products_unmapped_key(app: &mut App, key: crossterm::event::KeyEvent) {
    match key.code {
        KeyCode::Char('v') => {
            app.products_view = ProductsView::Mapped;
            // Don't cancel link mode automatically — user might switch back
        }
        KeyCode::Esc => {
            if app.link_mode {
                app.link_mode = false;
                app.link_product_id = None;
                app.products_view = ProductsView::Mapped;
                app.status_message = Some("Link mode cancelled".into());
            } else {
                app.products_view = ProductsView::Mapped;
            }
        }
        KeyCode::Left | KeyCode::Char('h') => {
            if app.unmapped_col > 0 {
                app.unmapped_col -= 1;
                app.unmapped_row = 0;
            }
        }
        KeyCode::Right | KeyCode::Char('l') => {
            if app.unmapped_col < 2 {
                app.unmapped_col += 1;
                app.unmapped_row = 0;
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if app.unmapped_row > 0 {
                app.unmapped_row -= 1;
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let count = app.current_column_listings().len();
            if count > 0 && app.unmapped_row < count - 1 {
                app.unmapped_row += 1;
            }
        }
        KeyCode::Enter => {
            let platforms = app.platform_columns();
            let platform = platforms[app.unmapped_col];
            // Clone the listing to avoid borrow issues
            let listing_opt = app.current_column_listings()
                .get(app.unmapped_row)
                .cloned();

            if let Some(listing) = listing_opt {
                if app.link_mode {
                    // Link this listing to the existing product
                    if let Some(product_id) = app.link_product_id.clone() {
                        app.link_listing_to_product(&product_id, platform, &listing)
                            .await;
                        app.link_mode = false;
                        app.link_product_id = None;
                        app.products_view = ProductsView::Mapped;
                        app.refresh_products().await;
                    }
                } else {
                    // Open import popup to create a new product
                    app.import_name = listing.title.clone();
                    app.import_sku = listing.sku.clone().unwrap_or_default();
                    app.import_field = 0;
                    app.import_popup = true;
                }
            } else {
                app.status_message = Some("No listing selected".into());
            }
        }
        KeyCode::Char('f') => {
            app.status_message = Some("Fetching listings from all platforms...".into());
            app.start_fetch_listings();
        }
        _ => {}
    }
}

async fn handle_import_popup_key(app: &mut App, key: crossterm::event::KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            app.import_popup = false;
        }
        KeyCode::Tab => {
            app.import_field = if app.import_field == 0 { 1 } else { 0 };
        }
        KeyCode::Enter => {
            // Create the product
            let platforms = app.platform_columns();
            let platform = platforms[app.unmapped_col];
            let listings = app
                .unmapped_listings
                .get(&platform)
                .map(|v| v.as_slice())
                .unwrap_or(&[]);

            if let Some(listing) = listings.get(app.unmapped_row).cloned() {
                let name = app.import_name.clone();
                let sku = if app.import_sku.is_empty() {
                    // Generate a simple SKU from the name
                    name.chars()
                        .filter(|c| c.is_alphanumeric() || *c == ' ')
                        .collect::<String>()
                        .split_whitespace()
                        .take(3)
                        .collect::<Vec<_>>()
                        .join("-")
                        .to_uppercase()
                } else {
                    app.import_sku.clone()
                };

                app.import_popup = false;
                app.import_listing_as_product(platform, &listing, &name, &sku)
                    .await;
                app.refresh_products().await;
            }
        }
        KeyCode::Backspace => {
            if app.import_field == 0 {
                app.import_name.pop();
            } else {
                app.import_sku.pop();
            }
        }
        KeyCode::Char(c) => {
            if app.import_field == 0 {
                app.import_name.push(c);
            } else {
                app.import_sku.push(c);
            }
        }
        _ => {}
    }
}

async fn handle_inventory_key(app: &mut App, key: crossterm::event::KeyEvent) {
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            if app.inventory_selected > 0 {
                app.inventory_selected -= 1;
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if !app.analytics.is_empty() && app.inventory_selected < app.analytics.len() - 1 {
                app.inventory_selected += 1;
            }
        }
        KeyCode::Char('d') => {
            app.inventory_time_window = app.inventory_time_window.next();
            app.refresh_analytics().await;
        }
        KeyCode::Enter => {
            app.inventory_expanded = !app.inventory_expanded;
        }
        _ => {}
    }
}

fn handle_config_key(app: &mut App, key: crossterm::event::KeyEvent) {
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            if app.config_selected > 0 {
                app.config_selected -= 1;
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.config_selected < app.config_fields.len() - 1 {
                app.config_selected += 1;
            }
        }
        KeyCode::Enter => {
            app.config_editing = true;
        }
        _ => {}
    }
}

fn handle_config_edit_key(app: &mut App, key: crossterm::event::KeyEvent) {
    match key.code {
        KeyCode::Enter => {
            app.config_editing = false;
            // Sync edited field value back into AppConfig
            if let Some(field) = app.config_fields.get(app.config_selected) {
                apply_config_field(&mut app.config, &field.key, &field.value);
            }
            // Save config and rebuild adapters
            if let Err(e) = app.config.save(&app.config_path) {
                app.status_message = Some(format!("Failed to save config: {e}"));
            } else {
                app.needs_adapter_rebuild = true;
                app.status_message = Some("Config saved, reloading adapters...".into());
            }
        }
        KeyCode::Esc => {
            app.config_editing = false;
        }
        KeyCode::Backspace => {
            if let Some(field) = app.config_fields.get_mut(app.config_selected) {
                field.value.pop();
            }
        }
        KeyCode::Char(c) => {
            if let Some(field) = app.config_fields.get_mut(app.config_selected) {
                field.value.push(c);
            }
        }
        _ => {}
    }
}

fn handle_search_key(app: &mut App, key: crossterm::event::KeyEvent) {
    match key.code {
        KeyCode::Enter | KeyCode::Esc => {
            app.searching = false;
        }
        KeyCode::Backspace => {
            app.search_query.pop();
        }
        KeyCode::Char(c) => {
            app.search_query.push(c);
            app.products_selected = 0; // Reset selection on filter change
        }
        _ => {}
    }
}

fn handle_logs_key(app: &mut App, key: crossterm::event::KeyEvent) {
    match key.code {
        KeyCode::PageUp => {
            app.log_scroll = app.log_scroll.saturating_add(10);
        }
        KeyCode::PageDown => {
            app.log_scroll = app.log_scroll.saturating_sub(10);
        }
        KeyCode::Char('0') => {
            app.log_level_filter = None;
        }
        _ => {}
    }
}

fn apply_config_field(config: &mut AppConfig, key: &str, value: &str) {
    match key {
        "general.sync_schedule" => config.general.sync_schedule = value.to_string(),
        "general.log_level" => config.general.log_level = value.to_string(),
        "general.max_retries" => {
            if let Ok(v) = value.parse() {
                config.general.max_retries = v;
            }
        }
        "database.path" => config.database.path = value.to_string(),
        "ebay.enabled" => config.ebay.enabled = value == "true",
        "ebay.client_id" => config.ebay.client_id = value.to_string(),
        "ebay.client_secret" => config.ebay.client_secret = value.to_string(),
        "ebay.environment" => config.ebay.environment = value.to_string(),
        "squarespace.enabled" => config.squarespace.enabled = value == "true",
        "squarespace.api_key" => config.squarespace.api_key = value.to_string(),
        "xmrbazaar.enabled" => config.xmrbazaar.enabled = value == "true",
        "xmrbazaar.username" => config.xmrbazaar.username = value.to_string(),
        "xmrbazaar.password" => config.xmrbazaar.password = value.to_string(),
        "alerts.default_low_stock_threshold" => {
            if let Ok(v) = value.parse() {
                config.alerts.default_low_stock_threshold = v;
            }
        }
        _ => {}
    }
}

async fn build_adapters(
    config: &AppConfig,
    db: Arc<Db>,
) -> Result<HashMap<Platform, Arc<dyn PlatformAdapter>>> {
    let mut adapters: HashMap<Platform, Arc<dyn PlatformAdapter>> = HashMap::new();

    if config.ebay.enabled {
        let adapter = nisaba_ebay::EbayAdapter::new(&config.ebay, db.clone()).await?;
        adapters.insert(Platform::Ebay, Arc::new(adapter));
    }

    if config.squarespace.enabled {
        let adapter = nisaba_squarespace::SquarespaceAdapter::new(&config.squarespace);
        adapters.insert(Platform::Squarespace, Arc::new(adapter));
    }

    if config.xmrbazaar.enabled {
        let adapter = nisaba_xmrbazaar::XmrBazaarAdapter::new(&config.xmrbazaar);
        adapters.insert(Platform::XmrBazaar, Arc::new(adapter));
    }

    Ok(adapters)
}
