//! MTrader CLI - Polymarket trading system.

use anyhow::Result;
use clap::{Parser, Subcommand};

mod config;
mod commands;
mod fee_profile;
mod logging;

use commands::{backtest, paper, record, replay};

#[derive(Parser)]
#[command(name = "mtrader")]
#[command(about = "Polymarket trading system for BTC 15-minute markets", long_about = None)]
#[command(version)]
struct Cli {
    /// Configuration file path
    #[arg(short, long, default_value = "config.toml")]
    config: String,

    /// Log level (trace, debug, info, warn, error)
    #[arg(short, long, default_value = "info")]
    log_level: String,

    /// Output logs as JSON
    #[arg(long)]
    json_logs: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run in paper trading mode (no real orders)
    Paper {
        /// Market token ID
        #[arg(short, long)]
        market: String,

        /// Strategy to run (maker_mm, bundle_maker)
        #[arg(short, long, default_value = "maker_mm")]
        strategy: String,

        /// Enable recording of all events
        #[arg(long)]
        record: bool,
    },

    /// Record market data without trading
    Record {
        /// Market token ID
        #[arg(short, long)]
        market: String,

        /// Output directory for recordings
        #[arg(short, long, default_value = "data/recordings")]
        output: String,

        /// Duration to record (e.g., "1h", "30m", "24h")
        #[arg(short, long)]
        duration: Option<String>,
    },

    /// Replay recorded data for backtesting
    Replay {
        /// Input file or directory
        #[arg(short, long)]
        input: String,

        /// Strategy to run
        #[arg(short, long, default_value = "maker_mm")]
        strategy: String,

        /// Replay speed multiplier (1.0 = real-time)
        #[arg(long, default_value = "0")]
        speed: f64,

        /// Output report file
        #[arg(short, long)]
        report: Option<String>,
    },

    /// Backtest auto-hedge strategy using recorded snapshots
    BacktestAuto {
        /// Input file or directory of JSONL snapshots
        #[arg(short, long)]
        input: String,

        /// Shares per leg
        #[arg(long)]
        shares: u64,

        /// Sum target threshold (price_up + price_down)
        #[arg(long, default_value = "0.95")]
        sum_target: f64,

        /// Dump threshold (e.g. 0.15 = 15%)
        #[arg(long, default_value = "0.15")]
        dip_threshold: f64,

        /// Window minutes for leg 1
        #[arg(long, default_value = "2")]
        window_minutes: u64,

        /// Sliding window for dip detection (ms)
        #[arg(long, default_value = "3000")]
        dip_window_ms: u64,

        /// Fee rate (bps)
        #[arg(long, default_value = "50")]
        fee_rate_bps: u16,

        /// Spread (bps)
        #[arg(long, default_value = "200")]
        spread_bps: f64,

        /// Leg 2 timeout (seconds)
        #[arg(long, default_value = "100")]
        leg2_timeout_seconds: u64,

        /// Starting balance in USDC
        #[arg(long, default_value = "1000")]
        starting_balance: f64,

        /// Output report file
        #[arg(short, long)]
        report: Option<String>,
    },

    /// Show market information
    Market {
        /// Market token ID
        #[arg(short, long)]
        market: String,
    },

    /// Validate configuration file
    ValidateConfig,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging
    logging::init(&cli.log_level, cli.json_logs)?;

    // Load configuration
    let config = config::load_config(&cli.config)?;

    match cli.command {
        Commands::Paper {
            market,
            strategy,
            record,
        } => {
            paper::run(&config, &market, &strategy, record).await?;
        }

        Commands::Record {
            market,
            output,
            duration,
        } => {
            record::run(&config, &market, &output, duration.as_deref()).await?;
        }

        Commands::Replay {
            input,
            strategy,
            speed,
            report,
        } => {
            replay::run(&config, &input, &strategy, speed, report.as_deref()).await?;
        }

        Commands::BacktestAuto {
            input,
            shares,
            sum_target,
            dip_threshold,
            window_minutes,
            dip_window_ms,
            fee_rate_bps,
            spread_bps,
            leg2_timeout_seconds,
            starting_balance,
            report,
        } => {
            backtest::run_auto_backtest(
                &config,
                &input,
                shares,
                sum_target,
                dip_threshold,
                window_minutes,
                dip_window_ms,
                fee_rate_bps,
                spread_bps,
                leg2_timeout_seconds,
                starting_balance,
                report.as_deref(),
            )?;
        }

        Commands::Market { market } => {
            commands::market::show(&config, &market).await?;
        }

        Commands::ValidateConfig => {
            println!("Configuration valid: {}", cli.config);
            println!("{:#?}", config);
        }
    }

    Ok(())
}
