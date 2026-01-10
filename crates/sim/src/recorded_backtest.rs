//! Recorded snapshot backtest for the auto-hedge strategy.

use mtrader_core::fees::FeeModel;
use mtrader_core::{Size, Tick};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

const TICK_SCALE: f64 = 10000.0;
const MS_PER_SEC: u64 = 1000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedSnapshot {
    pub timestamp_ms: u64,
    pub round_slug: String,
    pub seconds_remaining: u64,
    pub up_token_id: String,
    pub down_token_id: String,
    pub up_best_ask: Tick,
    pub down_best_ask: Tick,
}

#[derive(Debug, Clone)]
pub struct BacktestConfig {
    pub starting_balance_micro: i64,
    pub leg_size: Size,
    pub sum_target: f64,
    pub dip_threshold: f64,
    pub dip_window_ms: u64,
    pub window_minutes: u64,
    pub leg2_timeout_seconds: u64,
    pub fee_rate_bps: u16,
    pub spread_bps: f64,
}

impl Default for BacktestConfig {
    fn default() -> Self {
        Self {
            starting_balance_micro: 1_000_000_000,
            leg_size: 20_000_000,
            sum_target: 0.95,
            dip_threshold: 0.15,
            dip_window_ms: 3_000,
            window_minutes: 2,
            leg2_timeout_seconds: 100,
            fee_rate_bps: 50,
            spread_bps: 200.0,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct BacktestReport {
    pub starting_balance_micro: i64,
    pub ending_balance_micro: i64,
    pub cycles: u64,
    pub leg1_triggers: u64,
    pub leg2_triggers: u64,
    pub stop_losses: u64,
    pub round_losses: u64,
    pub roi_pct: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum HedgeSide {
    Up,
    Down,
}

impl HedgeSide {
    fn opposite(self) -> Self {
        match self {
            HedgeSide::Up => HedgeSide::Down,
            HedgeSide::Down => HedgeSide::Up,
        }
    }
}

#[derive(Debug, Clone)]
struct Leg1State {
    side: HedgeSide,
    entry_tick: Tick,
    entry_time_ms: u64,
}

#[derive(Debug, Default)]
struct History {
    map: HashMap<HedgeSide, VecDeque<(u64, Tick)>>,
}

impl History {
    fn push(&mut self, side: HedgeSide, now_ms: u64, tick: Tick, window_ms: u64) {
        let history = self.map.entry(side).or_default();
        history.push_back((now_ms, tick));
        while let Some((ts, _)) = history.front() {
            if now_ms.saturating_sub(*ts) > window_ms {
                history.pop_front();
            } else {
                break;
            }
        }
    }

    fn max_tick(&self, side: HedgeSide) -> Option<Tick> {
        self.map
            .get(&side)
            .and_then(|history| history.iter().map(|(_, tick)| *tick).max())
    }
}

pub fn load_snapshots(path: &Path) -> Result<Vec<RecordedSnapshot>, anyhow::Error> {
    let mut snapshots = Vec::new();
    if path.is_dir() {
        let mut files: Vec<PathBuf> = fs::read_dir(path)?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| path.extension().map(|ext| ext == "jsonl").unwrap_or(false))
            .collect();
        files.sort();
        for file in files {
            snapshots.extend(load_snapshots(&file)?);
        }
        return Ok(snapshots);
    }

    let contents = fs::read_to_string(path)?;
    for line in contents.lines().filter(|line| !line.trim().is_empty()) {
        let snapshot: RecordedSnapshot = serde_json::from_str(line)?;
        snapshots.push(snapshot);
    }

    snapshots.sort_by_key(|snap| snap.timestamp_ms);
    Ok(snapshots)
}

pub fn run_backtest(snapshots: &[RecordedSnapshot], config: &BacktestConfig) -> BacktestReport {
    let mut balance = config.starting_balance_micro;
    let mut history = History::default();
    let mut leg1: Option<Leg1State> = None;
    let mut round_start_ms: Option<u64> = None;
    let mut round_slug: Option<String> = None;

    let mut cycles = 0;
    let mut leg1_triggers = 0;
    let mut leg2_triggers = 0;
    let mut stop_losses = 0;
    let mut round_losses = 0;

    let fee_model = FeeModel::new(config.fee_rate_bps);
    let sum_target_tick = (config.sum_target * TICK_SCALE).round() as Tick;
    let window_ms = config.window_minutes * 60 * MS_PER_SEC;
    let timeout_ms = config.leg2_timeout_seconds * MS_PER_SEC;

    for snapshot in snapshots {
        if round_slug.as_deref() != Some(snapshot.round_slug.as_str()) {
            if leg1.is_some() {
                round_losses += 1;
                leg1 = None;
            }
            round_slug = Some(snapshot.round_slug.clone());
            round_start_ms = Some(snapshot.timestamp_ms);
            history.map.clear();
        }

        let now_ms = snapshot.timestamp_ms;
        let in_window = round_start_ms
            .map(|start| now_ms.saturating_sub(start) <= window_ms)
            .unwrap_or(true);

        let up_ask = apply_spread(snapshot.up_best_ask, config.spread_bps, true);
        let down_ask = apply_spread(snapshot.down_best_ask, config.spread_bps, true);

        history.push(HedgeSide::Up, now_ms, up_ask, config.dip_window_ms);
        history.push(HedgeSide::Down, now_ms, down_ask, config.dip_window_ms);

        if let Some(leg1_state) = &leg1 {
            if now_ms.saturating_sub(leg1_state.entry_time_ms) >= timeout_ms {
                let bid_tick = match leg1_state.side {
                    HedgeSide::Up => apply_spread(up_ask, config.spread_bps, false),
                    HedgeSide::Down => apply_spread(down_ask, config.spread_bps, false),
                };
                balance += proceeds(bid_tick, config.leg_size) as i64;
                balance -= fee_model.calculate_fee(bid_tick, config.leg_size) as i64;
                leg1 = None;
                stop_losses += 1;
                continue;
            }

            let opp_ask = match leg1_state.side.opposite() {
                HedgeSide::Up => up_ask,
                HedgeSide::Down => down_ask,
            };
            if leg1_state.entry_tick as u32 + opp_ask as u32 <= sum_target_tick as u32 {
                balance -= cost(opp_ask, config.leg_size) as i64;
                balance -= fee_model.calculate_fee(opp_ask, config.leg_size) as i64;

                balance += config.leg_size as i64;
                leg1 = None;
                leg2_triggers += 1;
                cycles += 1;
            }

            continue;
        }

        if !in_window {
            continue;
        }

        for (side, ask_tick) in [(HedgeSide::Up, up_ask), (HedgeSide::Down, down_ask)] {
            let Some(max_tick) = history.max_tick(side) else {
                continue;
            };
            if max_tick == 0 {
                continue;
            }
            let drop = (max_tick as f64 - ask_tick as f64) / max_tick as f64;
            if drop >= config.dip_threshold {
                balance -= cost(ask_tick, config.leg_size) as i64;
                balance -= fee_model.calculate_fee(ask_tick, config.leg_size) as i64;
                leg1 = Some(Leg1State {
                    side,
                    entry_tick: ask_tick,
                    entry_time_ms: now_ms,
                });
                leg1_triggers += 1;
                break;
            }
        }
    }

    let roi_pct = if config.starting_balance_micro != 0 {
        (balance as f64 / config.starting_balance_micro as f64 - 1.0) * 100.0
    } else {
        0.0
    };

    BacktestReport {
        starting_balance_micro: config.starting_balance_micro,
        ending_balance_micro: balance,
        cycles,
        leg1_triggers,
        leg2_triggers,
        stop_losses,
        round_losses,
        roi_pct,
    }
}

fn cost(price_tick: Tick, size: Size) -> u64 {
    (price_tick as u64 * size) / 10000
}

fn proceeds(price_tick: Tick, size: Size) -> u64 {
    (price_tick as u64 * size) / 10000
}

fn apply_spread(price_tick: Tick, spread_bps: f64, is_ask: bool) -> Tick {
    let price = price_tick as f64 / TICK_SCALE;
    let spread = spread_bps / 10000.0;
    let adjusted = if is_ask {
        price * (1.0 + spread)
    } else {
        price * (1.0 - spread)
    };
    (adjusted.clamp(0.0, 1.0) * TICK_SCALE).round() as Tick
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backtest_runs() {
        let snapshots = vec![
            RecordedSnapshot {
                timestamp_ms: 0,
                round_slug: "round-1".to_string(),
                seconds_remaining: 900,
                up_token_id: "up".to_string(),
                down_token_id: "down".to_string(),
                up_best_ask: 6000,
                down_best_ask: 4000,
            },
            RecordedSnapshot {
                timestamp_ms: 1000,
                round_slug: "round-1".to_string(),
                seconds_remaining: 899,
                up_token_id: "up".to_string(),
                down_token_id: "down".to_string(),
                up_best_ask: 5000,
                down_best_ask: 4500,
            },
        ];

        let report = run_backtest(&snapshots, &BacktestConfig::default());
        assert!(report.leg1_triggers >= 1);
    }
}
