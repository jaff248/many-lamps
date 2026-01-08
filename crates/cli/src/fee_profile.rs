//! Fee profile classification utilities.

use mtrader_core::fees::{FeeSchedule, MarketFeeProfile};
use mtrader_gateway::RestMarketInfo;

pub fn classify_fee_profile(info: &RestMarketInfo, default_fee_bps: u16) -> MarketFeeProfile {
    if let Some(fee_rate_str) = info.fee_rate_bps.as_deref() {
        if let Ok(fee_rate_bps) = fee_rate_str.parse::<u16>() {
            if fee_rate_bps == 0 {
                return MarketFeeProfile::zero("fee_rate_bps=0");
            }
            return MarketFeeProfile {
                label: "fee_rate_bps".to_string(),
                schedule: FeeSchedule::Parabolic { fee_rate_bps },
            };
        }
    }

    let mut fields = Vec::new();
    if let Some(market_type) = info.market_type.as_deref() {
        fields.push(market_type);
    }
    if let Some(category) = info.category.as_deref() {
        fields.push(category);
    }
    if let Some(question) = info.question.as_deref() {
        fields.push(question);
    }
    if let Some(tags) = info.tags.as_ref() {
        for tag in tags {
            fields.push(tag);
        }
    }

    let haystack = fields.join(" ").to_lowercase();

    if haystack.contains("15m")
        || haystack.contains("15-min")
        || haystack.contains("15 minute")
    {
        return MarketFeeProfile::crypto_15m();
    }

    if haystack.contains("1h")
        || haystack.contains("1 hour")
        || haystack.contains("60m")
        || haystack.contains("60 min")
    {
        return MarketFeeProfile::zero("crypto_1h");
    }

    MarketFeeProfile {
        label: "default_fee_profile".to_string(),
        schedule: FeeSchedule::Parabolic {
            fee_rate_bps: default_fee_bps,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_info() -> RestMarketInfo {
        RestMarketInfo {
            condition_id: "cond".to_string(),
            tokens: vec![],
            minimum_order_size: None,
            minimum_tick_size: None,
            active: true,
            closed: false,
            rewards: None,
            fee_rate_bps: None,
            market_type: None,
            category: None,
            question: None,
            tags: None,
        }
    }

    #[test]
    fn test_classify_15m_market() {
        let mut info = base_info();
        info.market_type = Some("crypto-15m".to_string());

        let profile = classify_fee_profile(&info, 1000);
        assert_eq!(profile.schedule, FeeSchedule::Parabolic { fee_rate_bps: 1000 });
    }

    #[test]
    fn test_classify_1h_market() {
        let mut info = base_info();
        info.category = Some("1h".to_string());

        let profile = classify_fee_profile(&info, 1000);
        assert_eq!(profile.schedule, FeeSchedule::Zero);
    }
}
