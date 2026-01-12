//! MTrader TUI - Full-screen interactive trading interface

use chrono::Local;
use many_lamps_book::ArrayBook;
use mtrader_risk::{PnLSnapshot, Position};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Widget},
    Terminal,
};
use std::{
    io,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

mod app;
mod screens;

pub use app::{App, AppState, MenuItem, TuiApp, Market, MarketSelection, PolymarketMarket, MarketSortBy, AutoTradingState};
pub use screens::{BacktestConfig, MarketInfo, StrategyInfo};

pub use self::app::run_tui;

const VERSION: &str = "1.0.0";
const TICK_RATE: Duration = Duration::from_millis(50);

/// Dashboard state for real-time updates
#[derive(Clone, Debug, Default)]
pub struct DashboardState {
    pub timestamp: String,
    pub market_id: String,
    pub best_bid: Option<u64>,
    pub best_ask: Option<u64>,
    pub spread_ticks: Option<u16>,
    pub position: i64,
    pub realized_pnl: i64,
    pub unrealized_pnl: i64,
    pub total_fees: i64,
    pub active_bids: usize,
    pub active_asks: usize,
    pub recent_actions: Vec<String>,
    pub strategy_name: String,
    pub connected: bool,
    pub trades_count: u64,
    pub recording: bool,
}

impl DashboardState {
    pub fn new(market_id: String, strategy_name: String) -> Self {
        Self {
            market_id,
            strategy_name,
            ..Default::default()
        }
    }

    pub fn log_action(&mut self, action: &str) {
        let timestamp = Local::now().format("%H:%M:%S.%3f").to_string();
        let entry = format!("[{}] {}", timestamp, action);
        self.recent_actions.insert(0, entry);
        if self.recent_actions.len() > 50 {
            self.recent_actions.pop();
        }
    }

    pub fn set_connected(&mut self, connected: bool) {
        self.connected = connected;
    }
}

/// Utility functions
fn format_pnl_micro(micro: i64) -> String {
    let usdc = micro as f64 / 1_000_000.0;
    format!("${:.2}", usdc)
}

fn pnl_style(pnl: i64) -> Style {
    if pnl > 0 {
        Style::default().fg(Color::Green)
    } else if pnl < 0 {
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(Color::Gray)
    }
}

/// Dashboard Widget
pub struct DashboardWidget {
    state: DashboardState,
}

impl DashboardWidget {
    pub fn new(state: DashboardState) -> Self {
        Self { state }
    }
}

impl Widget for &DashboardWidget {
    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        let chunks = Layout::vertical([
            Constraint::Length(3),
            Constraint::Length(10),
            Constraint::Length(8),
            Constraint::Min(10),
        ])
        .split(area);

        self.render_header(chunks[0], buf);
        self.render_market_position(chunks[1], buf);
        self.render_orders(chunks[2], buf);
        self.render_activity(chunks[3], buf);
    }
}

impl DashboardWidget {
    fn render_header(&self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        let status = if self.state.connected {
            "CONNECTED"
        } else {
            "DISCONNECTED"
        };
        let recording = if self.state.recording { " REC" } else { "" };

        let title = format!(
            " MTrader | {} | {} | {}{} ",
            self.state.market_id, self.state.strategy_name, status, recording
        );
        let style = Style::default()
            .bg(Color::Blue)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD);

        Paragraph::new(title).style(style).render(area, buf);
    }

    fn render_market_position(&self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        let chunks = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area);

        let market_block = Block::default()
            .title(" Market Data ")
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::White));
        let inner = market_block.inner(chunks[0]);
        market_block.render(chunks[0], buf);

        let bid_str = self
            .state
            .best_bid
            .map(|b| format!("{:.4}", b as f64 / 10000.0))
            .unwrap_or_else(|| "--".to_string());
        let ask_str = self
            .state
            .best_ask
            .map(|a| format!("{:.4}", a as f64 / 10000.0))
            .unwrap_or_else(|| "--".to_string());
        let spread_str = self
            .state
            .spread_ticks
            .map(|s| s.to_string())
            .unwrap_or_else(|| "--".to_string());

        let market_content = vec![
            Line::from(vec![
                Span::raw("Bid:  "),
                Span::raw(bid_str).style(
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::raw("Ask:  "),
                Span::raw(ask_str)
                    .style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
            ]),
            Line::from(vec![
                Span::raw("Spread: "),
                Span::raw(spread_str).style(Style::default().fg(Color::Yellow)),
            ]),
            Line::from(vec![Span::raw("Time: "), Span::raw(&self.state.timestamp)]),
        ];
        Paragraph::new(market_content).render(inner, buf);

        let pnl_block = Block::default()
            .title(" Position & PnL ")
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::White));
        let inner = pnl_block.inner(chunks[1]);
        pnl_block.render(chunks[1], buf);

        let position_style = if self.state.position > 0 {
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD)
        } else if self.state.position < 0 {
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        };

        let pnl_content = vec![
            Line::from(vec![
                Span::raw("Position:     "),
                Span::raw(format!("{}", self.state.position)).style(position_style),
            ]),
            Line::from(vec![
                Span::raw("Realized PnL: "),
                Span::raw(format_pnl_micro(self.state.realized_pnl))
                    .style(pnl_style(self.state.realized_pnl)),
            ]),
            Line::from(vec![
                Span::raw("Unrealized:   "),
                Span::raw(format_pnl_micro(self.state.unrealized_pnl))
                    .style(pnl_style(self.state.unrealized_pnl)),
            ]),
            Line::from(vec![
                Span::raw("Total Fees:   "),
                Span::raw(format_pnl_micro(self.state.total_fees))
                    .style(Style::default().fg(Color::Yellow)),
            ]),
            Line::from(vec![
                Span::raw("Trades:       "),
                Span::raw(self.state.trades_count.to_string()),
            ]),
        ];
        Paragraph::new(pnl_content).render(inner, buf);
    }

    fn render_orders(&self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        let chunks = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area);

        let bids_block = Block::default()
            .title(format!(" Active Bids ({}) ", self.state.active_bids))
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::Green));
        let inner = bids_block.inner(chunks[0]);
        bids_block.render(chunks[0], buf);

        let bid_text = if self.state.active_bids > 0 {
            "Active bid orders..."
        } else {
            "No active bids"
        };
        Paragraph::new(bid_text)
            .style(Style::default().fg(Color::Gray))
            .render(inner, buf);

        let asks_block = Block::default()
            .title(format!(" Active Asks ({}) ", self.state.active_asks))
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::Red));
        let inner = asks_block.inner(chunks[1]);
        asks_block.render(chunks[1], buf);

        let ask_text = if self.state.active_asks > 0 {
            "Active ask orders..."
        } else {
            "No active asks"
        };
        Paragraph::new(ask_text)
            .style(Style::default().fg(Color::Gray))
            .render(inner, buf);
    }

    fn render_activity(&self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        let log_area = Rect::new(area.x, area.y, area.width, area.height - 3);
        let status_area = Rect::new(area.x, area.y + area.height - 3, area.width, 3);

        let block = Block::default()
            .title(" Activity Log ")
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::White));
        let inner = block.inner(log_area);
        block.render(log_area, buf);

        let items: Vec<ListItem> = self
            .state
            .recent_actions
            .iter()
            .take(15)
            .map(|action| {
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
            })
            .collect();

        List::new(items).render(inner, buf);

        let help_text = " [Enter] Select [r] Record [s] Switch [q] Quit ";
        let help_style = Style::default().bg(Color::DarkGray).fg(Color::White);
        Paragraph::new(help_text)
            .style(help_style)
            .render(status_area, buf);
    }
}

/// Run dashboard thread with terminal rendering
fn run_dashboard_thread(receiver: mpsc::Receiver<DashboardState>) {
    let stdout = io::stdout();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).expect("Failed to create terminal");
    terminal.clear().expect("Failed to clear terminal");

    loop {
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(new_state) => {
                terminal
                    .draw(|f| {
                        let widget = DashboardWidget::new(new_state);
                        f.render_widget(&widget, f.area());
                    })
                    .ok();
            }
            Err(_) => {
                if thread::panicking() {
                    break;
                }
            }
        }
    }
}

/// Run dashboard thread with pre-created terminal
fn run_dashboard_thread_with_terminal(
    receiver: mpsc::Receiver<DashboardState>, 
    mut terminal: Terminal<CrosstermBackend<io::Stdout>>
) {
    terminal.clear().ok();

    loop {
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(new_state) => {
                terminal
                    .draw(|f| {
                        let widget = DashboardWidget::new(new_state);
                        f.render_widget(&widget, f.area());
                    })
                    .ok();
            }
            Err(_) => {
                if thread::panicking() {
                    break;
                }
            }
        }
    }
}

/// Run dashboard thread without terminal (headless mode)
fn run_dashboard_thread_no_tty(receiver: mpsc::Receiver<DashboardState>) {
    loop {
        match receiver.recv_timeout(Duration::from_millis(500)) {
            Ok(_new_state) => {
                // In headless mode, we just drain the channel
                // The paper trading command already logs to console
            }
            Err(_) => {
                if thread::panicking() {
                    break;
                }
            }
        }
    }
}

/// Dashboard Controller
pub struct DashboardController {
    sender: mpsc::Sender<DashboardState>,
    state: DashboardState,
    last_update: Instant,
}

impl DashboardController {
    pub fn new(market_id: String, strategy_name: String) -> Self {
        let (sender, receiver) = mpsc::channel();
        let state = DashboardState::new(market_id, strategy_name.clone());

        let controller = Self {
            sender,
            state: state.clone(),
            last_update: Instant::now(),
        };

        // Check if stdout is a TTY and spawn appropriate thread
        thread::spawn(move || {
            // Try to create a terminal; if it fails, run in headless mode
            let stdout = io::stdout();
            let backend = CrosstermBackend::new(stdout);
            
            // Try to create terminal and clear it
            if let Ok(mut terminal) = Terminal::new(backend) {
                if terminal.clear().is_ok() {
                    run_dashboard_thread_with_terminal(receiver, terminal);
                    return;
                }
            }
            
            // Fallback: run without terminal rendering, just drain the channel
            run_dashboard_thread_no_tty(receiver);
        });

        controller
    }

    pub fn log(&mut self, action: &str) {
        self.state.log_action(action);
        self.maybe_send();
    }

    pub fn update_book(&mut self, book: &ArrayBook) {
        self.state.best_bid = book.best_bid().map(|b| b as u64);
        self.state.best_ask = book.best_ask().map(|a| a as u64);
        self.state.spread_ticks = match (self.state.best_bid, self.state.best_ask) {
            (Some(bid), Some(ask)) if ask > bid => Some((ask - bid) as u16),
            _ => None,
        };
        self.maybe_send();
    }

    pub fn update_pnl(&mut self, pnl: &PnLSnapshot) {
        self.state.realized_pnl = pnl.realized_pnl;
        self.state.unrealized_pnl = pnl.unrealized_pnl;
        self.state.total_fees = pnl.total_fees;
        self.maybe_send();
    }

    pub fn update_position(&mut self, position: &Position) {
        self.state.position = position.net_size;
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

    pub fn set_connected(&mut self, connected: bool) {
        self.state.set_connected(connected);
        self.maybe_send();
    }

    fn maybe_send(&mut self) {
        let now = Instant::now();
        if now.duration_since(self.last_update) >= TICK_RATE {
            self.sender.send(self.state.clone()).ok();
            self.last_update = now;
        }
    }

    pub fn state(&self) -> &DashboardState {
        &self.state
    }
}

/// Format backtest report
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
==========================
     BACKTEST RESULTS
==========================

Starting Balance:    {}
Ending Balance:      {}
Net PnL:             {}
ROI:                 {}

Cycles Completed:    {}
Leg 1 Triggers:      {}
Leg 2 Triggers:      {}
Stop Losses:         {}
Round Losses:        {}
==========================
"#,
        start_str,
        end_str,
        pnl_str,
        roi_str,
        cycles,
        leg1_triggers,
        leg2_triggers,
        stop_losses,
        round_losses
    )
}
