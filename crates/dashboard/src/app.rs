//! MTrader TUI Application

use crate::format_backtest_report;
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode},
    terminal::{disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Widget},
    Terminal,
};
use std::{
    io::{self, Stdout},
    time::Duration,
};

const VERSION: &str = "1.0.0";

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum MenuItem {
    #[default]
    MainMenu,
    PaperTrading,
    Record,
    Backtest,
    Replay,
    MarketBrowser,
    Help,
    Quit,
}

#[derive(Clone, Debug, Default)]
pub struct AppState {
    pub current_menu: MenuItem,
    pub selected_index: usize,
    pub market_id: String,
    pub strategy_name: String,
    pub is_recording: bool,
    pub is_connected: bool,
    pub backtest_results: Option<String>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            current_menu: MenuItem::MainMenu,
            selected_index: 0,
            market_id: "btc-updown-15m-1767933000".to_string(),
            strategy_name: "maker_mm".to_string(),
            is_recording: false,
            is_connected: false,
            backtest_results: None,
        }
    }
}

pub struct App {
    state: AppState,
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl App {
    pub fn new() -> Result<Self> {
        enable_raw_mode()?;
        let stdout = io::stdout();
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;
        Ok(Self { state: AppState::new(), terminal })
    }

    pub fn run(&mut self) -> Result<()> {
        self.terminal.clear()?;
        loop {
            self.terminal.draw(|f| {
                f.render_widget(&TuiApp { state: &self.state }, f.size());
            })?;
            if event::poll(Duration::from_millis(16))? {
                if let Event::Key(key) = event::read()? {
                    self.handle_input(key);
                }
            }
            if self.state.current_menu == MenuItem::Quit {
                break;
            }
        }
        disable_raw_mode()?;
        Ok(())
    }

    fn handle_input(&mut self, key: event::KeyEvent) {
        match self.state.current_menu {
            MenuItem::MainMenu => match key.code {
                KeyCode::Up | KeyCode::Char('k') => self.state.selected_index = self.state.selected_index.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => { self.state.selected_index = self.state.selected_index.saturating_add(1); if self.state.selected_index > 6 { self.state.selected_index = 0; } }
                KeyCode::Enter => self.state.current_menu = match self.state.selected_index {
                    0 => MenuItem::PaperTrading, 1 => MenuItem::Record, 2 => MenuItem::Backtest,
                    3 => MenuItem::Replay, 4 => MenuItem::MarketBrowser, 5 => MenuItem::Help, _ => MenuItem::Quit,
                },
                KeyCode::Esc | KeyCode::Char('q') => self.state.current_menu = MenuItem::Quit,
                _ => {}
            },
            MenuItem::PaperTrading => match key.code {
                KeyCode::Char('r') => self.state.is_recording = !self.state.is_recording,
                KeyCode::Char('s') => match self.state.strategy_name.as_str() {
                    "maker_mm" => self.state.strategy_name = "bundle_maker".to_string(),
                    "bundle_maker" => self.state.strategy_name = "unaffected_arb".to_string(),
                    "unaffected_arb" => self.state.strategy_name = "rebalancing_arb".to_string(),
                    _ => self.state.strategy_name = "maker_mm".to_string(),
                },
                KeyCode::Char('q') | KeyCode::Esc => self.state.current_menu = MenuItem::MainMenu,
                _ => {}
            },
            MenuItem::Backtest => match key.code {
                KeyCode::Enter => {
                    let report = format_backtest_report(1000.0, 1047.32, 4.73, 156, 89, 67, 3, 12);
                    self.state.backtest_results = Some(report);
                }
                KeyCode::Esc | KeyCode::Char('q') => self.state.current_menu = MenuItem::MainMenu,
                _ => {}
            },
            MenuItem::Help => if key.code == KeyCode::Esc { self.state.current_menu = MenuItem::MainMenu; },
            _ => if key.code == KeyCode::Esc || key.code == KeyCode::Char('q') { self.state.current_menu = MenuItem::MainMenu; }
        }
    }
}

/// TUI App Widget
pub struct TuiApp<'a> {
    state: &'a AppState,
}

impl<'a> TuiApp<'a> {
    pub fn new(state: &'a AppState) -> Self {
        Self { state }
    }
}

impl<'a> Widget for &TuiApp<'a> {
    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        match self.state.current_menu {
            MenuItem::MainMenu => render_main_menu(self.state, area, buf),
            MenuItem::PaperTrading => render_paper_trading(self.state, area, buf),
            MenuItem::Record => render_record(self.state, area, buf),
            MenuItem::Backtest => render_backtest(self.state, area, buf),
            MenuItem::Help => render_help(self.state, area, buf),
            _ => render_placeholder(self.state, area, buf),
        }
    }
}

fn render_main_menu(state: &AppState, area: Rect, buf: &mut ratatui::buffer::Buffer) {
    let chunks = Layout::vertical([Constraint::Length(5), Constraint::Min(10), Constraint::Length(8)]).split(area);
    let title_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let header = Paragraph::new(Line::from(vec![Span::raw("MTrader"), Span::styled(format!(" v{}", VERSION), title_style)])).alignment(Alignment::Center).style(Style::default().bg(Color::Blue).fg(Color::White));
    header.render(chunks[0], buf);
    
    let menu_items = [
        ("Paper Trading", "Practice trading with fake money (safe!)"),
        ("Record Market", "Record market data for later analysis"),
        ("Backtest", "Test strategies against historical data"),
        ("Replay", "Replay recorded sessions"),
        ("Market Browser", "Browse and search markets"),
        ("Help", "View keyboard shortcuts"),
        ("Quit", "Exit MTrader"),
    ];
    
    let mut menu_lines = Vec::new();
    for (i, (title, desc)) in menu_items.iter().enumerate() {
        let prefix = if i == state.selected_index { " > " } else { "   " };
        let style = if i == state.selected_index { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::White) };
        let line = Line::from(vec![Span::raw(prefix), Span::styled(*title, style), Span::raw(" - "), Span::raw(*desc)]);
        menu_lines.push(ListItem::new(line));
    }
    let menu = List::new(menu_lines).block(Block::default().title(" Choose an Action ").borders(Borders::ALL));
    menu.render(chunks[1], buf);
    let footer = Paragraph::new(" [Up/Down] Navigate  [Enter] Select  [Esc] Exit ").style(Style::default().bg(Color::DarkGray).fg(Color::White)).alignment(Alignment::Center);
    footer.render(chunks[2], buf);
}

fn render_paper_trading(state: &AppState, area: Rect, buf: &mut ratatui::buffer::Buffer) {
    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(10), Constraint::Length(3)]).split(area);
    let status = if state.is_connected { "CONNECTED" } else { "DISCONNECTED" };
    let recording = if state.is_recording { " REC" } else { "" };
    let header = Paragraph::new(format!(" Paper Trading | Market: {} | {} | [r] Record [s] Switch [q] Menu {}", state.market_id, status, recording)).style(Style::default().bg(Color::Blue).fg(Color::White)).alignment(Alignment::Center);
    header.render(chunks[0], buf);
    let dashboard = Paragraph::new("Live trading dashboard\n\n- Order book with best bid/ask\n- Position and PnL\n- Active orders\n- Activity log\n\nPress [r] to toggle recording\nPress [s] to switch strategy\nPress [q] to return to menu").block(Block::default().title(" Dashboard ").borders(Borders::ALL)).alignment(Alignment::Center);
    dashboard.render(chunks[1], buf);
    let status_bar = Paragraph::new(" [r] Record  [s] Switch Strategy  [q] Menu ").style(Style::default().bg(Color::DarkGray).fg(Color::White));
    status_bar.render(chunks[2], buf);
}

fn render_record(state: &AppState, area: Rect, buf: &mut ratatui::buffer::Buffer) {
    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(10), Constraint::Length(3)]).split(area);
    let header = Paragraph::new(" Record Market Data ").style(Style::default().bg(Color::DarkGray).fg(Color::White)).alignment(Alignment::Center);
    header.render(chunks[0], buf);
    let content = Paragraph::new(format!("Market ID: {}\n\nRecording will capture:\n- Order book updates\n- Trade events\n- Price changes\n\nPress [Enter] to start recording\nPress [q] to return to menu", state.market_id)).block(Block::default().title(" Setup ").borders(Borders::ALL)).alignment(Alignment::Center);
    content.render(chunks[1], buf);
    let status_bar = Paragraph::new(" [Enter] Start  [q] Menu ").style(Style::default().bg(Color::DarkGray).fg(Color::White));
    status_bar.render(chunks[2], buf);
}

fn render_backtest(state: &AppState, area: Rect, buf: &mut ratatui::buffer::Buffer) {
    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(10), Constraint::Length(3)]).split(area);
    let header = Paragraph::new(" Backtest Strategies ").style(Style::default().bg(Color::DarkGray).fg(Color::White)).alignment(Alignment::Center);
    header.render(chunks[0], buf);
    let content = if let Some(ref results) = state.backtest_results {
        Paragraph::new(results.as_str()).block(Block::default().title(" Results ").borders(Borders::ALL))
    } else {
        Paragraph::new("Configure and run backtests on historical data.\n\nSettings:\n- Input directory (JSONL snapshots)\n- Shares per leg\n- Sum target threshold\n- Dip detection parameters\n\nPress [Enter] to run sample backtest\nResults will show ROI, PnL, and trade statistics").block(Block::default().title(" Configuration ").borders(Borders::ALL))
    };
    content.render(chunks[1], buf);
    let status_bar = Paragraph::new(" [Enter] Run Backtest  [q] Menu ").style(Style::default().bg(Color::DarkGray).fg(Color::White));
    status_bar.render(chunks[2], buf);
}

fn render_help(_state: &AppState, area: Rect, buf: &mut ratatui::buffer::Buffer) {
    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(10), Constraint::Length(3)]).split(area);
    let header = Paragraph::new(" Keyboard Shortcuts ").style(Style::default().bg(Color::DarkGray).fg(Color::White)).alignment(Alignment::Center);
    header.render(chunks[0], buf);
    let help_text = "KEYBOARD SHORTCUTS\n\nUp/Down    Navigate menus and lists\nEnter      Select / Confirm\nEsc        Go back / Cancel\nq          Quit current screen / Menu\nr          Toggle recording (paper trading)\ns          Switch strategy\nCtrl+C     Emergency exit";
    let content = Paragraph::new(help_text).block(Block::default().title(" Help ").borders(Borders::ALL));
    content.render(chunks[1], buf);
    let status_bar = Paragraph::new(" [Esc] Back to Menu ").style(Style::default().bg(Color::DarkGray).fg(Color::White));
    status_bar.render(chunks[2], buf);
}

fn render_placeholder(_state: &AppState, area: Rect, buf: &mut ratatui::buffer::Buffer) {
    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(10), Constraint::Length(3)]).split(area);
    let header = Paragraph::new(" Coming Soon ").style(Style::default().bg(Color::DarkGray).fg(Color::White)).alignment(Alignment::Center);
    header.render(chunks[0], buf);
    let content = Paragraph::new("This feature is coming soon!\n\nIn the meantime, use the command line:\n  mtrader paper --market <TOKEN_ID>\n  mtrader backtest --input <DIR>").block(Block::default().title(" Work in Progress ").borders(Borders::ALL)).alignment(Alignment::Center);
    content.render(chunks[1], buf);
    let status_bar = Paragraph::new(" [q] Menu ").style(Style::default().bg(Color::DarkGray).fg(Color::White));
    status_bar.render(chunks[2], buf);
}

/// Run the TUI application
pub fn run_tui() -> Result<()> {
    let mut app = App::new()?;
    app.run()?;
    Ok(())
}
