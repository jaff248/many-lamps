//! Real-time trading dashboard using ratatui TUI.
//!
//! Shows:
//! - Market status (best bid/ask, spread)
//! - Position & PnL
//! - Active orders
//! - Recent activity log
//! - Backtest results

use chrono::Local;
use many_lamps_book::ArrayBook;
use many_lamps_core::{Side, Size, Tick};
use mtrader_risk::{PnLSnapshot, Position};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Widget},
    Terminal,
};
use std::io::{self, Stdout};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// Dashboard state for real-time updates
#[derive(Clone, Debug)]
pub struct DashboardState {
    /// Current timestamp
    pub timestamp: String,
    /// Market ID
    pub market_id: String,
    /// Best bid
    pub best_bid: Option<Tick>,
    /// Best ask
    pub best_ask: Option<Tick>,
    /// Spread in ticks
    pub spread_ticks: Option<u16>,
    /// Position
    pub position: i64,
    /// Realized PnL (micro USDC)
    pub realized_pnl: i64,
    /// Unrealized PnL (micro USDC)
    pub unrealized_pnl: i64,
    /// Total fees paid (micro USDC)
    pub total_fees: i64,
    /// Active bid orders count
    pub active_bids: usize,
    /// Active ask orders count
    pub active_asks: usize,
    /// Recent actions log
    pub recent_actions: Vec<String>,
    /// Strategy name
    pub strategy_name: String,
    /// Connection status
    pub connected: bool,
    /// Trades count
    pub trades_count: u64,
}

impl Default for DashboardState {
    fn default() -> Self {
        Self {
            timestamp: Local::now().format("%H:%M:%S").to_string(),
            market_id: "".to_string(),
            best_bid: None,
            best_ask: None,
            spread_ticks: None,
            position: 0,
            realized_pnl: 0,
            unrealized_pnl: 0,
            total_fees: 0,
            active_bids: 0,
            active_asks: 0,
            recent_actions: Vec::with_capacity(20),
            strategy_name: "".to_string(),
            connected: false,
            trades_count: 0,
        }
    }
}

impl DashboardState {
    /// Create new state
    pub fn new(market_id: String, strategy_name: String) -> Self {
        Self {
            market_id,
            strategy_name,
            ..Default::default()
        }
    }

    /// Update from book
    pub fn update_book(&mut self, book: &ArrayBook) {
        self.best_bid = book.best_bid();
        self.best_ask = book.best_ask();
        self.spread_ticks = match (self.best_bid, self.best_ask) {
            (Some(bid), Some(ask)) if ask > bid => Some(ask - bid),
            _ => None,
        };
    }

    /// Update from PnL snapshot
    pub fn update_pnl(&mut self, pnl: &PnLSnapshot) {
        self.realized_pnl = pnl.realized_pnl;
        self.unrealized_pnl = pnl.unrealized_pnl;
        self.total_fees = pnl.total_fees;
    }

    /// Update position
    pub fn update_position(&mut self, position: &Position) {
        self.position = position.net_size;
    }

    /// Add action to log
    pub fn log_action(&mut self, action: &str) {
        let timestamp = Local::now().format("%H:%M:%S.%3f").to_string();
        let entry = format!("[{}] {}", timestamp, action);
        self.recent_actions.insert(0, entry);
        if self.recent_actions.len() > 50 {
            self.recent_actions.pop();
        }
    }

    /// Log a fill
    pub fn log_fill(&mut self, side: Side, price: Tick, size: Size) {
        let side_str = match side {
            Side::Buy => "BUY",
            Side::Sell => "SELL",
        };
        self.trades_count += 1;
        self.log_action(&format!(
            "{} {} @ {} ({} total trades)",
            side_str,
            size_to_str(size),
            price_to_str(price),
            self.trades_count
        ));
    }

    /// Log order placed
    pub fn log_order(&mut self, side: Side, price: Tick, size: Size) {
        let side_str = match side {
            Side::Buy => "BID",
            Side::Sell => "ASK",
        };
        self.log_action(&format!(
            "Place {} {} @ {}",
            side_str,
            size_to_str(size),
            price_to_str(price)
        ));
    }

    /// Log order canceled
    pub fn log_cancel(&mut self, client_order_id: &str) {
        self.log_action(&format!("Cancel order {}", client_order_id));
    }

    /// Set connection status
    pub fn set_connected(&mut self, connected: bool) {
        self.connected = connected;
    }
}

/// Convert tick to price string (e.g., "0.5500")
pub fn price_to_str(tick: Tick) -> String {
    format!("{:.4}", tick as f64 / 10000.0)
}

/// Convert size to readable string
pub fn size_to_str(size: Size) -> String {
    let shares = size as f64 / 1_000_000.0;
    if shares >= 1.0 {
        format!("{:.2}", shares)
    } else {
        format!("{:.4}", shares)
    }
}

/// Dashboard widget
pub struct DashboardWidget {
    state: DashboardState,
}

impl DashboardWidget {
    /// Create new dashboard
    pub fn new(state: DashboardState) -> Self {
        Self { state }
    }
}

impl Widget for &DashboardWidget {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let chunks = Layout::vertical([
            Constraint::Length(3),  // Header
            Constraint::Length(8),  // Market data & Position
            Constraint::Length(10), // Orders
            Constraint::Min(10),    // Activity log
        ])
        .split(area);

        self.render_header(chunks[0], buf);
        self.render_market_position(chunks[1], buf);
        self.render_orders(chunks[2], buf);
        self.render_activity(chunks[3], buf);
    }
}

impl DashboardWidget {
    fn render_header(&self, area: Rect, buf: &mut Buffer) {
        let title = format!(
            " MTrader - Paper Trading | {} | {} | {} ",
            self.state.market_id,
            self.state.strategy_name,
            if self.state.connected { "CONNECTED" } else { "DISCONNECTED" }
        );
        let style = Style::default()
            .bg(Color::Blue)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD);

        Paragraph::new(title).style(style).render(area, buf);
    }

    fn render_market_position(&self, area: Rect, buf: &mut Buffer) {
        let chunks = Layout::horizontal([
            Constraint::Percentage(50),
            Constraint::Percentage(50),
        ])
        .split(area);

        let market_block = Block::default()
            .title(" Market Data ")
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::White));
        let inner = market_block.inner(chunks[0]);
        market_block.render(chunks[0], buf);

        let bid_str = self.state.best_bid.map(price_to_str).unwrap_or_else(|| "--".to_string());
        let ask_str = self.state.best_ask.map(price_to_str).unwrap_or_else(|| "--".to_string());
        let spread_str = self.state.spread_ticks.map(|s| s.to_string()).unwrap_or_else(|| "--".to_string());

        let market_content = vec![
            Line::from(vec![Span::raw("Bid: "), Span::raw(bid_str).green().bold()]),
            Line::from(vec![Span::raw("Ask: "), Span::raw(ask_str).red().bold()]),
            Line::from(vec![Span::raw("Spread: "), Span::raw(spread_str).yellow()]),
            Line::from(vec![Span::raw("Time: "), Span::raw(&self.state.timestamp)]),
        ];
        Paragraph::new(market_content).render(inner, buf);

        let pnl_block = Block::default()
            .title(" Position & PnL ")
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::White));
        let inner = pnl_block.inner(chunks[1]);
        pnl_block.render(chunks[1], buf);

        let position_color = if self.state.position > 0 { Color::Green } else if self.state.position < 0 { Color::Red } else { Color::White };

        let realized_pnl_str = format_pnl_micro(self.state.realized_pnl);
        let unrealized_pnl_str = format_pnl_micro(self.state.unrealized_pnl);
        let fees_str = format_pnl_micro(self.state.total_fees);

        let pnl_content = vec![
            Line::from(vec![Span::raw("Position: "), Span::raw(format!("{}", self.state.position)).fg(position_color).bold()]),
            Line::from(vec![Span::raw("Realized PnL: "), Span::raw(realized_pnl_str).fg(pnl_color(self.state.realized_pnl))]),
            Line::from(vec![Span::raw("Unrealized PnL: "), Span::raw(unrealized_pnl_str).fg(pnl_color(self.state.unrealized_pnl))]),
            Line::from(vec![Span::raw("Total Fees: "), Span::raw(fees_str).yellow()]),
            Line::from(vec![Span::raw("Trades: "), Span::raw(self.state.trades_count.to_string())]),
        ];
        Paragraph::new(pnl_content).render(inner, buf);
    }

    fn render_orders(&self, area: Rect, buf: &mut Buffer) {
        let chunks = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(area);

        let bids_block = Block::default()
            .title(format!(" Active Bids ({}) ", self.state.active_bids))
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::Green));
        let inner = bids_block.inner(chunks[0]);
        bids_block.render(chunks[0], buf);

        if self.state.active_bids > 0 {
            let bid_item = ListItem::new("Active bid orders pending...".italic());
            List::new([bid_item]).style(Style::default().fg(Color::Green)).render(inner, buf);
        } else {
            Paragraph::new("No active bids".italic().dim()).style(Style::default().fg(Color::Gray)).render(inner, buf);
        }

        let asks_block = Block::default()
            .title(format!(" Active Asks ({}) ", self.state.active_asks))
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::Red));
        let inner = asks_block.inner(chunks[1]);
        asks_block.render(chunks[1], buf);

        if self.state.active_asks > 0 {
            let ask_item = ListItem::new("Active ask orders pending...".italic());
            List::new([ask_item]).style(Style::default().fg(Color::Red)).render(inner, buf);
        } else {
            Paragraph::new("No active asks".italic().dim()).style(Style::default().fg(Color::Gray)).render(inner, buf);
        }
    }

    fn render_activity(&self, area: Rect, buf: &mut Buffer) {
        let block = Block::default().title(" Activity Log ").borders(Borders::ALL).style(Style::default().fg(Color::White));
        let inner = block.inner(area);
        block.render(area, buf);

        let items: Vec<ListItem> = self.state.recent_actions.iter().take(20).map(|action| {
            let style = if action.contains("BUY") {
                Style::default().fg(Color::Green)
            } else if action.contains("SELL") {
                Style::default().fg(Color::Red)
            } else if action.contains("Place") {
                Style::default().fg(Color::Cyan)
            } else if action.contains("Cancel") {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::Gray)
            };
            ListItem::new(action.clone()).style(style)
        }).collect();

        List::new(items).render(inner, buf);
    }
}

fn format_pnl_micro(micro: i64) -> String {
    let usdc = micro as f64 / 1_000_000.0;
    format!("${:.2}", usdc)
}

fn pnl_color(pnl: i64) -> Color {
    if pnl > 0 { Color::Green } else if pnl < 0 { Color::Red } else { Color::Gray }
}

pub fn run_dashboard<F>(state: DashboardState, receiver: mpsc::Receiver<DashboardState>, _render_fn: F)
where
    F: Fn() + Send + 'static,
{
    use ratatui::backend::CrosstermBackend;

    thread::spawn(move || {
        let stdout = io::stdout();
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.clear().unwrap();

        loop {
            match receiver.recv_timeout(Duration::from_millis(100)) {
                Ok(new_state) => {
                    terminal.draw(|_f| {
                        let widget = DashboardWidget::new(new_state);
                        _f.render_widget(&widget, _f.size());
                    }).ok();
                }
                Err(_) => {}
            }
        }
    });
}

pub struct DashboardController {
    sender: mpsc::Sender<DashboardState>,
    state: DashboardState,
    last_update: std::time::Instant,
}

impl DashboardController {
    pub fn new(market_id: String, strategy_name: String) -> Self {
        let (sender, receiver) = mpsc::channel();
        let state = DashboardState::new(market_id, strategy_name.clone());

        let controller = Self {
            sender,
            state: state.clone(),
            last_update: std::time::Instant::now(),
        };

        run_dashboard(state, receiver, || {});
        controller
    }

    pub fn update_book(&mut self, book: &ArrayBook) {
        self.state.update_book(book);
        self.maybe_send();
    }

    pub fn update_pnl(&mut self, pnl: &PnLSnapshot) {
        self.state.update_pnl(pnl);
        self.maybe_send();
    }

    pub fn update_position(&mut self, position: &Position) {
        self.state.update_position(position);
        self.maybe_send();
    }

    pub fn log(&mut self, action: &str) {
        self.state.log_action(action);
        self.maybe_send();
    }

    pub fn log_fill(&mut self, side: Side, price: Tick, size: Size) {
        self.state.log_fill(side, price, size);
        self.maybe_send();
    }

    pub fn log_order(&mut self, side: Side, price: Tick, size: Size) {
        self.state.log_order(side, price, size);
        self.maybe_send();
    }

    pub fn log_cancel(&mut self, client_order_id: &str) {
        self.state.log_cancel(client_order_id);
        self.maybe_send();
    }

    pub fn set_connected(&mut self, connected: bool) {
        self.state.set_connected(connected);
        self.maybe_send();
    }

    pub fn update_active_orders(&mut self, bids: usize, asks: usize) {
        self.state.active_bids = bids;
        self.state.active_asks = asks;
        self.maybe_send();
    }

    pub fn update_timestamp(&mut self) {
        self.state.timestamp = Local::now().format("%H:%M:%S").to_string();
        self.maybe_send();
    }

    fn maybe_send(&mut self) {
        let now = std::time::Instant::now();
        if now.duration_since(self.last_update) >= Duration::from_millis(50) {
            self.sender.send(self.state.clone()).ok();
            self.last_update = now;
        }
    }

    pub fn state(&self) -> &DashboardState {
        &self.state
    }
}

pub fn format_backtest_report(
    starting_balance: f64,
    ending_balance: f64,
    roi_pct: f64,
    cycles: u64,
    leg1_triggers: u64,
    leg2_triggers: u64,
    stop_losses: u64,
    round_losses: u64,
) -> String {
    let pnl = ending_balance - starting_balance;
    let pnl_str = format!("${:.2}", pnl);
    let roi_str = format!("{:.2}%", roi_pct);
    let start_str = format!("${:.2}", starting_balance);
    let end_str = format!("${:.2}", ending_balance);

    format!(
        r#"
============================================================
                     BACKTEST RESULTS
============================================================

  Starting Balance:    ${:>10}
  Ending Balance:      ${:>10}
  Net PnL:             {:>10}
  ROI:                 {:>10}

  Cycles Completed:    {:>10}
  Leg 1 Triggers:      {:>10}
  Leg 2 Triggers:      {:>10}
  Stop Losses:         {:>10}
  Round Losses:        {:>10}

============================================================
"#,
        start_str, end_str, pnl_str, roi_str, cycles, leg1_triggers, leg2_triggers, stop_losses, round_losses
    )
}
