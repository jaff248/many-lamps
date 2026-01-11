#[cfg(test)]
mod tests {
    use crate::combinatorial_arb::{
        CombinatorialArbConfig, CombinatorialArbStrategy, Dependency, DependencyType,
    };
    use crate::{MarketSnapshot, MarketTokenKey, Strategy, StrategyContext};
    use mtrader_risk::{PnLSnapshot, Position};
    use std::collections::HashMap;

    #[test]
    fn test_implication_arb_detection() {
        let dep = Dependency {
            source_market_id: "m1".to_string(),
            source_token_id: "t1".to_string(),
            target_market_id: "m2".to_string(),
            target_token_id: "t2".to_string(),
            relation: DependencyType::Implication,
            min_profit_bps: 10,
        };

        let config = CombinatorialArbConfig {
            dependencies: vec![dep.clone()],
            ..Default::default()
        };

        let strategy = CombinatorialArbStrategy::new(config);

        // Case 1: No Arb (Source < Target)
        let actions = strategy.check_implication_arb(&dep, 0.40, 0.50);
        assert!(actions.is_none());

        // Case 2: Arb Exists (Source > Target by 5%)
        let actions = strategy.check_implication_arb(&dep, 0.55, 0.50);
        assert!(actions.is_some());
        let actions = actions.unwrap();
        assert_eq!(actions.len(), 2); // Sell Source, Buy Target

        assert_eq!(actions[0].clone().into_place_order_asset_id().as_deref(), Some("m1"));
        assert_eq!(actions[1].clone().into_place_order_asset_id().as_deref(), Some("m2"));
    }

    #[test]
    fn test_mutually_exclusive_arb_detection() {
        let dep = Dependency {
            source_market_id: "m1".to_string(),
            source_token_id: "t1".to_string(),
            target_market_id: "m2".to_string(),
            target_token_id: "t2".to_string(),
            relation: DependencyType::MutuallyExclusive,
            min_profit_bps: 10,
        };

        let config = CombinatorialArbConfig {
            dependencies: vec![dep.clone()],
            ..Default::default()
        };

        let strategy = CombinatorialArbStrategy::new(config);
        let actions = strategy.check_mutually_exclusive_arb(&dep, 0.55, 0.50);
        assert!(actions.is_some());
        let actions = actions.unwrap();
        assert_eq!(actions.len(), 2); // Sell both

        assert_eq!(actions[0].clone().into_place_order_asset_id().as_deref(), Some("m1"));
        assert_eq!(actions[1].clone().into_place_order_asset_id().as_deref(), Some("m2"));
    }

    #[test]
    fn test_identical_arb_detection() {
        let dep = Dependency {
            source_market_id: "m1".to_string(),
            source_token_id: "t1".to_string(),
            target_market_id: "m2".to_string(),
            target_token_id: "t2".to_string(),
            relation: DependencyType::Identical,
            min_profit_bps: 10,
        };

        let config = CombinatorialArbConfig {
            dependencies: vec![dep.clone()],
            ..Default::default()
        };

        let strategy = CombinatorialArbStrategy::new(config);
        let actions = strategy.check_identical_arb(&dep, 0.48, 0.52);
        assert!(actions.is_some());
        let actions = actions.unwrap();
        assert_eq!(actions.len(), 2); // Buy cheap, sell rich

        // source is cheaper (0.48), so we sell target (m2) and buy source (m1)
        assert_eq!(actions[0].clone().into_place_order_asset_id().as_deref(), Some("m2"));
        assert_eq!(actions[1].clone().into_place_order_asset_id().as_deref(), Some("m1"));
    }

    #[test]
    fn test_on_update_with_snapshots_generates_actions() {
        let dep = Dependency {
            source_market_id: "m1".to_string(),
            source_token_id: "yes".to_string(),
            target_market_id: "m2".to_string(),
            target_token_id: "yes".to_string(),
            relation: DependencyType::Implication,
            min_profit_bps: 5,
        };

        let config = CombinatorialArbConfig {
            dependencies: vec![dep],
            min_profit_threshold_bps: 5,
            ..Default::default()
        };

        let mut strategy = CombinatorialArbStrategy::new(config);
        strategy.activate();

        let mut market_snapshots = HashMap::new();
        market_snapshots.insert(
            MarketTokenKey {
                market_id: "m1".to_string(),
                token_id: "yes".to_string(),
            },
            MarketSnapshot {
                best_bid: Some(5600),
                best_ask: Some(5600),
                mid_tick: Some(5600),
            },
        );
        market_snapshots.insert(
            MarketTokenKey {
                market_id: "m2".to_string(),
                token_id: "yes".to_string(),
            },
            MarketSnapshot {
                best_bid: Some(5200),
                best_ask: Some(5200),
                mid_tick: Some(5200),
            },
        );

        let ctx = StrategyContext {
            now_ns: 1,
            asset_id: "m1".to_string(),
            position: Position::new(),
            pnl: PnLSnapshot {
                timestamp_ns: 0,
                realized_pnl: 0,
                unrealized_pnl: 0,
                total_pnl: 0,
                total_fees: 0,
                net_pnl: 0,
                high_water_mark: 0,
                drawdown: 0,
                drawdown_bps: 0,
            },
            best_bid: Some(5600),
            best_ask: Some(5600),
            best_bid_size: 0,
            best_ask_size: 0,
            mid_tick: Some(5600),
            spread_ticks: Some(0),
            our_bids: Vec::new(),
            our_asks: Vec::new(),
            market_snapshots,
        };

        let actions = strategy.on_update(&ctx);
        assert_eq!(actions.len(), 2);

        assert_eq!(actions[0].clone().into_place_order_asset_id().as_deref(), Some("m1"));
        assert_eq!(actions[1].clone().into_place_order_asset_id().as_deref(), Some("m2"));
    }

    trait PlaceOrderAssetId {
        fn into_place_order_asset_id(self) -> Option<String>;
    }

    impl PlaceOrderAssetId for crate::StrategyAction {
        fn into_place_order_asset_id(self) -> Option<String> {
            match self {
                crate::StrategyAction::PlaceOrder { asset_id, .. } => Some(asset_id),
                _ => None,
            }
        }
    }
}
