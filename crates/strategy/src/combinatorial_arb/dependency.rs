/// Types of dependency between two outcomes
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DependencyType {
    /// A implies B (if A happens, B MUST happen).
    /// Arb: Price(A) cannot be > Price(B).
    /// If Price(A) > Price(B), buy B, sell A.
    Implication,

    /// A and B are mutually exclusive (cannot both happen).
    /// Arb: Price(A) + Price(B) cannot be > 1.
    /// If Price(A) + Price(B) > 1, sell A, sell B.
    MutuallyExclusive,

    /// A and B are identical (should have same price).
    /// Arb: Price(A) != Price(B).
    /// If Price(A) < Price(B), buy A, sell B.
    Identical,
}

/// A directed edge in the dependency graph
#[derive(Debug, Clone)]
pub struct Dependency {
    pub source_market_id: String,
    pub source_token_id: String,
    pub target_market_id: String,
    pub target_token_id: String,
    pub relation: DependencyType,
    pub min_profit_bps: u32,
}
