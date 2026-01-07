//! Deterministic event replay engine.
//!
//! Replays recorded events for backtesting and debugging.

use mtrader_core::events::CoreEvent;
use std::collections::VecDeque;

/// Replay mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayMode {
    /// Real-time replay (respects event timestamps).
    RealTime,
    /// Fast-forward (process as fast as possible).
    FastForward,
    /// Step-by-step (manual advance).
    Step,
}

/// Event replay engine.
pub struct ReplayEngine {
    /// Events to replay.
    events: VecDeque<CoreEvent>,
    /// Current replay mode.
    mode: ReplayMode,
    /// Current simulated time (nanoseconds).
    current_time_ns: u64,
    /// Replay speed multiplier (for real-time mode).
    speed_multiplier: f64,
    /// Events processed count.
    events_processed: u64,
}

impl ReplayEngine {
    pub fn new(mode: ReplayMode) -> Self {
        Self {
            events: VecDeque::new(),
            mode,
            current_time_ns: 0,
            speed_multiplier: 1.0,
            events_processed: 0,
        }
    }

    /// Load events for replay.
    pub fn load_events(&mut self, events: impl IntoIterator<Item = CoreEvent>) {
        self.events = events.into_iter().collect();
        self.current_time_ns = 0;
        self.events_processed = 0;
    }

    /// Set replay mode.
    pub fn set_mode(&mut self, mode: ReplayMode) {
        self.mode = mode;
    }

    /// Set speed multiplier (for real-time mode).
    pub fn set_speed(&mut self, multiplier: f64) {
        self.speed_multiplier = multiplier.max(0.1).min(100.0);
    }

    /// Get the next event without consuming it.
    pub fn peek(&self) -> Option<&CoreEvent> {
        self.events.front()
    }

    /// Get the next event.
    pub fn next_event(&mut self) -> Option<CoreEvent> {
        let event = self.events.pop_front()?;
        self.update_time(&event);
        self.events_processed += 1;
        Some(event)
    }

    /// Get events up to a certain time.
    pub fn events_until(&mut self, target_time_ns: u64) -> Vec<CoreEvent> {
        let mut events = Vec::new();
        while let Some(event) = self.events.front() {
            let event_time = Self::event_timestamp(event);
            if event_time <= target_time_ns {
                let event = self.events.pop_front().unwrap();
                self.update_time(&event);
                self.events_processed += 1;
                events.push(event);
            } else {
                break;
            }
        }
        events
    }

    /// Advance time by a duration.
    pub fn advance_time(&mut self, duration_ns: u64) -> Vec<CoreEvent> {
        let target = self.current_time_ns + duration_ns;
        self.events_until(target)
    }

    /// Check if replay is complete.
    pub fn is_complete(&self) -> bool {
        self.events.is_empty()
    }

    /// Get current simulated time.
    pub fn current_time_ns(&self) -> u64 {
        self.current_time_ns
    }

    /// Get number of events processed.
    pub fn events_processed(&self) -> u64 {
        self.events_processed
    }

    /// Get number of events remaining.
    pub fn events_remaining(&self) -> usize {
        self.events.len()
    }

    /// Reset replay to beginning.
    pub fn reset(&mut self) {
        // Note: This doesn't reload events, just resets counters
        self.current_time_ns = 0;
        self.events_processed = 0;
    }

    /// Skip to a specific time.
    pub fn skip_to(&mut self, target_time_ns: u64) {
        while let Some(event) = self.events.front() {
            if Self::event_timestamp(event) < target_time_ns {
                let event = self.events.pop_front().unwrap();
                self.update_time(&event);
                self.events_processed += 1;
            } else {
                break;
            }
        }
        self.current_time_ns = target_time_ns;
    }

    fn update_time(&mut self, event: &CoreEvent) {
        let ts = Self::event_timestamp(event);
        if ts > self.current_time_ns {
            self.current_time_ns = ts;
        }
    }

    fn event_timestamp(event: &CoreEvent) -> u64 {
        // Use process timestamp as canonical
        match event {
            CoreEvent::BookUpdate(e) => e.ts_process_mono_ns,
            CoreEvent::Trade(e) => e.ts_process_mono_ns,
            CoreEvent::OrderAck(e) => e.ts_process_mono_ns,
            CoreEvent::OrderFill(e) => e.ts_process_mono_ns,
            CoreEvent::OrderCancel(e) => e.ts_process_mono_ns,
            CoreEvent::OrderReject(e) => e.ts_process_mono_ns,
            CoreEvent::StrategySignal(e) => e.timestamp_ns,
            CoreEvent::RiskEvent(e) => e.timestamp_ns,
            CoreEvent::SystemHealth(e) => e.timestamp_ns,
        }
    }
}

/// Replay statistics.
#[derive(Debug, Clone, Default)]
pub struct ReplayStats {
    pub total_events: u64,
    pub book_updates: u64,
    pub trades: u64,
    pub order_acks: u64,
    pub fills: u64,
    pub cancels: u64,
    pub rejects: u64,
    pub first_event_time_ns: Option<u64>,
    pub last_event_time_ns: Option<u64>,
}

impl ReplayStats {
    /// Calculate stats from events.
    pub fn from_events(events: &[CoreEvent]) -> Self {
        let mut stats = Self::default();
        stats.total_events = events.len() as u64;

        for event in events {
            match event {
                CoreEvent::BookUpdate(_) => stats.book_updates += 1,
                CoreEvent::Trade(_) => stats.trades += 1,
                CoreEvent::OrderAck(_) => stats.order_acks += 1,
                CoreEvent::OrderFill(_) => stats.fills += 1,
                CoreEvent::OrderCancel(_) => stats.cancels += 1,
                CoreEvent::OrderReject(_) => stats.rejects += 1,
                _ => {}
            }

            let ts = ReplayEngine::event_timestamp(event);
            match stats.first_event_time_ns {
                None => stats.first_event_time_ns = Some(ts),
                Some(first) if ts < first => stats.first_event_time_ns = Some(ts),
                _ => {}
            }
            match stats.last_event_time_ns {
                None => stats.last_event_time_ns = Some(ts),
                Some(last) if ts > last => stats.last_event_time_ns = Some(ts),
                _ => {}
            }
        }

        stats
    }

    /// Get duration in nanoseconds.
    pub fn duration_ns(&self) -> u64 {
        match (self.first_event_time_ns, self.last_event_time_ns) {
            (Some(first), Some(last)) => last.saturating_sub(first),
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mtrader_core::events::{BookUpdateEvent, TradeEvent};
    use mtrader_core::Side;

    fn make_book_event(ts: u64) -> CoreEvent {
        CoreEvent::BookUpdate(BookUpdateEvent {
            side: Side::Buy,
            price_tick: 5000,
            new_size: 1000,
            ts_exchange_ms: ts / 1_000_000,
            ts_recv_mono_ns: ts,
            ts_process_mono_ns: ts,
        })
    }

    fn make_trade_event(ts: u64) -> CoreEvent {
        CoreEvent::Trade(TradeEvent {
            side: Side::Buy,
            price_tick: 5000,
            size: 100,
            trade_id: "trade-1".into(),
            ts_exchange_ms: ts / 1_000_000,
            ts_recv_mono_ns: ts,
            ts_process_mono_ns: ts,
        })
    }

    #[test]
    fn test_replay_basic() {
        let mut engine = ReplayEngine::new(ReplayMode::FastForward);

        let events = vec![
            make_book_event(1_000_000),
            make_book_event(2_000_000),
            make_trade_event(3_000_000),
        ];

        engine.load_events(events);

        assert_eq!(engine.events_remaining(), 3);
        assert!(!engine.is_complete());

        let e1 = engine.next_event();
        assert!(matches!(e1, Some(CoreEvent::BookUpdate(_))));
        assert_eq!(engine.events_processed(), 1);

        let e2 = engine.next_event();
        assert!(matches!(e2, Some(CoreEvent::BookUpdate(_))));

        let e3 = engine.next_event();
        assert!(matches!(e3, Some(CoreEvent::Trade(_))));

        assert!(engine.is_complete());
        assert_eq!(engine.events_processed(), 3);
    }

    #[test]
    fn test_events_until() {
        let mut engine = ReplayEngine::new(ReplayMode::FastForward);

        let events = vec![
            make_book_event(1_000_000),
            make_book_event(2_000_000),
            make_trade_event(5_000_000),
            make_book_event(10_000_000),
        ];

        engine.load_events(events);

        let batch = engine.events_until(3_000_000);
        assert_eq!(batch.len(), 2);
        assert_eq!(engine.current_time_ns(), 2_000_000);

        let batch2 = engine.events_until(100_000_000);
        assert_eq!(batch2.len(), 2);
        assert!(engine.is_complete());
    }

    #[test]
    fn test_skip_to() {
        let mut engine = ReplayEngine::new(ReplayMode::FastForward);

        let events = vec![
            make_book_event(1_000_000),
            make_book_event(2_000_000),
            make_trade_event(5_000_000),
            make_book_event(10_000_000),
        ];

        engine.load_events(events);

        engine.skip_to(6_000_000);
        assert_eq!(engine.events_remaining(), 1);
        assert_eq!(engine.events_processed(), 3);
    }

    #[test]
    fn test_replay_stats() {
        let events = vec![
            make_book_event(1_000_000),
            make_book_event(2_000_000),
            make_trade_event(5_000_000),
            make_book_event(10_000_000),
        ];

        let stats = ReplayStats::from_events(&events);

        assert_eq!(stats.total_events, 4);
        assert_eq!(stats.book_updates, 3);
        assert_eq!(stats.trades, 1);
        assert_eq!(stats.first_event_time_ns, Some(1_000_000));
        assert_eq!(stats.last_event_time_ns, Some(10_000_000));
        assert_eq!(stats.duration_ns(), 9_000_000);
    }
}
