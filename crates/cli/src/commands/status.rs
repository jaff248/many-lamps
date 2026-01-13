//! Status Command - Check system status

use anyhow::Result;

/// Check and display system status
pub fn run(config_path: &str, verbose: bool) -> Result<()> {
    println!("\n🟢 MTrader System Status\n");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    println!("\n📁 Configuration:");
    println!("  Config file: {}", config_path);

    println!("\n⚙️  Risk Limits:");
    println!("  Max position: ${}", 100.0); // Placeholder - would read from config
    println!("  Fee rate: {} bps", 50);

    println!("\n📊 Strategy Settings:");
    println!("  Spread ticks: 100");
    println!("  Order size: 30");
    println!("  Skew factor: 0.5");

    if verbose {
        println!("\n🔧 Gateway Settings:");
        println!("  WS URL: wss://clob.polymarket.com");
        println!("  REST URL: https://clob.polymarket.com");

        println!("\n📼 Recording:");
        println!("  Output dir: data/recordings");
    }

    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("\n💡 Tip: Run 'mtrader tui' for the interactive TUI");
    println!("       Run 'mtrader --help' for all commands\n");

    Ok(())
}
