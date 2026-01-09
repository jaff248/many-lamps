//! MTrader CLI - Polymarket trading system.
//!
//! A human-first CLI for trading on Polymarket markets.
//! Run with no arguments for interactive mode, or use commands directly.

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use std::io::{self, Write};

mod config;
mod commands;
mod fee_profile;
mod logging;

use commands::{backtest, market, paper, record, replay, status, tui};

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn get_available_strategies() -> Vec<&'static str> {
    vec!["maker_mm", "bundle_maker", "unaffected_arb", "rebalancing_arb"]
}

#[derive(Parser)]
#[command(name = "mtrader")]
#[command(author = "MTrader Team")]
#[command(version = VERSION)]
#[command(about = "Polymarket trading system", long_about = None)]
#[command(after_help = "EXAMPLES:
    mtrader                    # Start interactive mode
    mtrader tui                # Launch full-screen TUI
    mtrader paper --market xyz # Paper trade
    mtrader list strategies    # Show strategies
    mtrader status             # Check system status
    
For more help: mtrader --help <command>")]
struct Cli {
    #[arg(short, long, default_value = "config.toml", global = true)]
    config: String,
    #[arg(short, long, default_value = "info", global = true)]
    log_level: String,
    #[arg(long, global = true)]
    json_logs: bool,
    #[arg(short, long, global = true)]
    verbose: bool,
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// 🟢 Paper trading mode (safe, no real orders)
    Paper {
        #[arg(short, long)]
        market: Option<String>,
        #[arg(short, long, default_value = "maker_mm")]
        strategy: String,
        #[arg(long)]
        record: bool,
    },
    /// 📼 Record market data
    Record {
        #[arg(short, long)]
        market: Option<String>,
        #[arg(short, long, default_value = "data/recordings")]
        output: String,
        #[arg(short, long)]
        duration: Option<String>,
    },
    /// ▶️ Replay recorded data
    Replay {
        #[arg(short, long)]
        input: Option<String>,
        #[arg(short, long, default_value = "maker_mm")]
        strategy: String,
        #[arg(long, default_value = "0")]
        speed: f64,
        #[arg(short, long)]
        report: Option<String>,
    },
    /// 🧪 Backtest strategy
    BacktestAuto {
        #[arg(short, long)]
        input: Option<String>,
        #[arg(long)]
        shares: u64,
        #[arg(long, default_value = "0.95")]
        sum_target: f64,
        #[arg(long, default_value = "0.15")]
        dip_threshold: f64,
        #[arg(long, default_value = "2")]
        window_minutes: u64,
        #[arg(long, default_value = "3000")]
        dip_window_ms: u64,
        #[arg(long, default_value = "50")]
        fee_rate_bps: u16,
        #[arg(long, default_value = "200")]
        spread_bps: f64,
        #[arg(long, default_value = "100")]
        leg2_timeout_seconds: u64,
        #[arg(long, default_value = "1000")]
        starting_balance: f64,
        #[arg(short, long)]
        report: Option<String>,
    },
    /// 📊 Show market info
    Market {
        #[arg(short, long)]
        market: Option<String>,
    },
    /// 📋 List resources
    List {
        #[arg(value_name = "RESOURCE")]
        resource: Option<String>,
    },
    /// 💚 Check status
    Status {
        #[arg(short, long)]
        verbose: bool,
    },
    /// 🖥️ Launch interactive TUI
    Tui,
    /// ✅ Validate config
    ValidateConfig,
}

fn prompt_value(prompt: &str, default: Option<&str>, required: bool) -> Result<String> {
    let default_str = default.map(|s| format!(" [{}]", s)).unwrap_or_default();
    print!("{}{}: ", prompt, default_str);
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim().to_string();
    if input.is_empty() {
        if let Some(d) = default {
            Ok(d.to_string())
        } else if required {
            bail!("{} is required", prompt);
        } else {
            Ok(input)
        }
    } else {
        Ok(input)
    }
}

fn print_example(cmd: &str, desc: &str) {
    println!("  → {}", cmd);
    println!("    {}", desc);
}

fn print_header(text: &str) {
    println!();
    println!("┌─ {}", text);
    println!("│");
}

fn print_footer() {
    println!("│");
    println!("└───────────────────────────────────────────────");
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let log_level = if cli.verbose { "debug".to_string() } else { cli.log_level.clone() };
    logging::init(&log_level, cli.json_logs)?;

    let command = match cli.command {
        Some(cmd) => cmd,
        None => {
            println!("🟢  MTrader v{} - Polymarket Trading System", VERSION);
            println!();
            println!("  A safe, human-first trading system for Polymarket markets.");
            println!();
            println!("  QUICK START:");
            println!();
            print_example("mtrader", "Start interactive mode");
            print_example("mtrader tui", "Launch full-screen TUI");
            print_example("mtrader paper --market btc-updown-15m-1767933000", "Paper trade");
            print_example("mtrader list strategies", "See available strategies");
            println!();
            println!("  For more help: mtrader --help");
            println!();
            return Ok(());
        }
    };

    let config = config::load_config(&cli.config)?;

    match command {
        Commands::Paper { market, strategy, record } => {
            let market = match market {
                Some(m) => m,
                None => prompt_value("Market token ID", None, true)?,
            };
            paper::run(&config, &market, &strategy, record).await?;
        }
        Commands::Record { market, output, duration } => {
            let market = match market {
                Some(m) => m,
                None => prompt_value("Market token ID", None, true)?,
            };
            record::run(&config, &market, &output, duration.as_deref()).await?;
        }
        Commands::Replay { input, strategy, speed, report } => {
            let input = match input {
                Some(i) => i,
                None => prompt_value("Input file or directory", None, true)?,
            };
            replay::run(&config, &input, &strategy, speed, report.as_deref()).await?;
        }
        Commands::BacktestAuto { input, shares, sum_target, dip_threshold, window_minutes, dip_window_ms, fee_rate_bps, spread_bps, leg2_timeout_seconds, starting_balance, report } => {
            let input = match input {
                Some(i) => i,
                None => prompt_value("Input file or directory", None, true)?,
            };
            backtest::run_auto_backtest(&config, &input, shares, sum_target, dip_threshold, window_minutes, dip_window_ms, fee_rate_bps, spread_bps, leg2_timeout_seconds, starting_balance, report.as_deref())?;
        }
        Commands::Market { market } => {
            let market = match market {
                Some(m) => m,
                None => prompt_value("Market token ID", None, true)?,
            };
            market::show(&config, &market).await?;
        }
        Commands::List { resource } => {
            match resource.as_deref() {
                Some("strategies") | Some("strategy") => {
                    println!();
                    println!("📋 Available Strategies:");
                    print_header("Strategies");
                    for (i, s) in get_available_strategies().iter().enumerate() {
                        let desc = match *s {
                            "maker_mm" => "Market making - earn spread",
                            "bundle_maker" => "Bundle arbitrage",
                            "unaffected_arb" => "Unaffected asset arbitrage",
                            "rebalancing_arb" => "NO/YES rebalancing arb",
                            _ => "Unknown strategy",
                        };
                        println!("  {}  {}", i + 1, desc);
                    }
                    print_footer();
                }
                Some("markets") | Some("market") => {
                    println!();
                    println!("📈 Example Markets:");
                    print_header("Markets");
                    println!("  btc-updown-15m-1767933000  BTC 15-min up/down");
                    println!("  btc-updown-15m-1767994200  BTC 15-min up/down");
                    println!("  eth-updown-15m-1767933000  ETH 15-min up/down");
                    print_footer();
                }
                _ => {
                    println!("Usage: mtrader list <strategies|markets>");
                    println!();
                    println!("Available: strategies, markets");
                }
            }
        }
        Commands::Status { verbose } => {
            status::run(&cli.config, verbose)?;
        }
        Commands::Tui => {
            tui::run().await?;
        }
        Commands::ValidateConfig => {
            println!("Configuration valid: {}", cli.config);
            println!("{:#?}", config);
        }
    }

    Ok(())
}
