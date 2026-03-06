mod scheduler;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tracing::info;
use tracing_subscriber::EnvFilter;

use nisaba_core::config::AppConfig;
use nisaba_core::db::Db;
use nisaba_core::sync_engine::{SyncEngine, SyncEngineEvent};
use nisaba_core::traits::PlatformAdapter;
use nisaba_core::types::Platform;

#[derive(Parser)]
#[command(name = "nisaba", about = "Cross-platform inventory sync service")]
struct Cli {
    /// Path to config file (default: %APPDATA%/nisaba/config.toml)
    #[arg(short, long)]
    config: Option<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Run a single sync cycle immediately
    SyncNow,
    /// Start interactive eBay OAuth flow
    EbayAuth,
    /// Run the sync scheduler (default if no subcommand)
    Run,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("nisaba=info".parse()?))
        .init();

    let (config, _config_path) = AppConfig::resolve_and_load(cli.config.as_deref())
        .context("Failed to load config")?;

    let db = Arc::new(
        Db::open(&config.database.path)
            .await
            .context("Failed to open database")?,
    );
    db.migrate().await.context("Failed to run migrations")?;

    let adapters = std::sync::Arc::new(tokio::sync::RwLock::new(
        build_adapters(&config, db.clone()).await?,
    ));

    match cli.command.unwrap_or(Commands::Run) {
        Commands::SyncNow => {
            info!("Running single sync cycle");
            let (event_tx, _event_rx) = tokio::sync::mpsc::channel::<SyncEngineEvent>(64);
            let engine = SyncEngine::new(db, adapters, event_tx, config.general.max_retries);
            let (synced, pushed) = engine.run_cycle().await?;
            info!(synced, pushed, "Sync cycle complete");
        }
        Commands::EbayAuth => {
            run_ebay_auth(&config, db).await?;
        }
        Commands::Run => {
            info!("Starting scheduled sync");
            scheduler::run_scheduled(
                db,
                adapters,
                &config.general.sync_schedule,
                config.general.max_retries,
            )
            .await?;
        }
    }

    Ok(())
}

async fn build_adapters(
    config: &AppConfig,
    db: Arc<Db>,
) -> Result<HashMap<Platform, Arc<dyn PlatformAdapter>>> {
    let mut adapters: HashMap<Platform, Arc<dyn PlatformAdapter>> = HashMap::new();

    if config.ebay.enabled {
        let adapter = nisaba_ebay::EbayAdapter::new(&config.ebay, db.clone()).await?;
        adapters.insert(Platform::Ebay, Arc::new(adapter));
        info!("eBay adapter enabled");
    }

    if config.squarespace.enabled {
        let adapter = nisaba_squarespace::SquarespaceAdapter::new(&config.squarespace);
        adapters.insert(Platform::Squarespace, Arc::new(adapter));
        info!("Squarespace adapter enabled");
    }

    if config.xmrbazaar.enabled {
        let adapter = nisaba_xmrbazaar::XmrBazaarAdapter::new(&config.xmrbazaar);
        adapters.insert(Platform::XmrBazaar, Arc::new(adapter));
        info!("XMR Bazaar adapter enabled");
    }

    Ok(adapters)
}

async fn run_ebay_auth(config: &AppConfig, db: Arc<Db>) -> Result<()> {
    let adapter = nisaba_ebay::EbayAdapter::new(&config.ebay, db).await?;

    println!("\n=== eBay OAuth Setup ===\n");
    println!("1. Open this URL in your browser:\n");
    println!("   {}\n", adapter.authorization_url());
    println!("2. Authorize the application");
    println!("3. You'll be redirected to a URL containing a 'code' parameter");
    println!("4. Paste the authorization code below:\n");

    let mut code = String::new();
    std::io::stdin().read_line(&mut code)?;
    let code = code.trim();

    adapter.complete_auth(code).await?;
    println!("\neBay authentication complete! Tokens saved to database.");

    Ok(())
}
