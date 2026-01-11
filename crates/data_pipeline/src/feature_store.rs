//! Feature Store with LRU Caching
//!
//! High-performance cache for computed features supporting sub-microsecond
//! cache hit latency. Designed for both backtest and live trading scenarios.

use many_lamps_core::TokenId;
use mtrader_ml::ExtractedFeatureVector;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::sync::{Arc, Mutex};
use thiserror::Error;

/// Feature store configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureStoreConfig {
    pub capacity: usize,
    pub ttl_seconds: u64,
    pub enable_stats: bool,
}

impl Default for FeatureStoreConfig {
    fn default() -> Self {
        Self { capacity: 10_000, ttl_seconds: 300, enable_stats: true }
    }
}

impl FeatureStoreConfig {
    pub fn with_capacity(capacity: usize) -> Self {
        Self { capacity, ..Default::default() }
    }
    pub fn with_ttl(ttl_seconds: u64) -> Self {
        Self { ttl_seconds, ..Default::default() }
    }
    pub fn hft() -> Self {
        Self { capacity: 1_000, ttl_seconds: 0, enable_stats: true }
    }
    pub fn backtest() -> Self {
        Self { capacity: 100_000, ttl_seconds: 0, enable_stats: true }
    }
}

/// Wrapper struct for cached features with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedFeature {
    pub feature: ExtractedFeatureVector,
    pub cached_at: i64,
    pub expires_at: i64,
    pub access_count: usize,
    pub metadata: Option<FeatureMetadata>,
}

impl CachedFeature {
    pub fn new(feature: ExtractedFeatureVector, ttl_seconds: u64, current_time_us: i64) -> Self {
        let expires_at = if ttl_seconds > 0 { current_time_us + (ttl_seconds as i64) * 1_000_000 } else { 0 };
        Self { feature, cached_at: current_time_us, expires_at, access_count: 1, metadata: None }
    }
    
    #[inline]
    pub fn is_expired(&self, current_time_us: i64) -> bool {
        self.expires_at > 0 && current_time_us >= self.expires_at
    }
    
    #[inline]
    pub fn mark_accessed(&mut self) {
        self.access_count += 1;
    }
    
    #[inline]
    pub fn feature(&self) -> &ExtractedFeatureVector {
        &self.feature
    }
    
    #[inline]
    pub fn feature_mut(&mut self) -> &mut ExtractedFeatureVector {
        &mut self.feature
    }
}

/// Optional metadata for cached features
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureMetadata {
    pub source: String,
    pub compute_time_us: f64,
    pub context: Option<String>,
}

/// Cache statistics
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub insertions: u64,
    pub current_size: usize,
    pub capacity: usize,
    pub hit_rate: f64,
}

impl CacheStats {
    #[inline]
    pub fn record_hit(&mut self) {
        self.hits += 1;
        self.update_hit_rate();
    }
    
    #[inline]
    pub fn record_miss(&mut self) {
        self.misses += 1;
        self.update_hit_rate();
    }
    
    #[inline]
    pub fn record_eviction(&mut self) {
        self.evictions += 1;
    }
    
    #[inline]
    pub fn record_insertion(&mut self) {
        self.insertions += 1;
    }
    
    #[inline]
    pub fn set_size(&mut self, size: usize) {
        self.current_size = size;
    }
    
    #[inline]
    pub fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity;
    }
    
    #[inline]
    fn update_hit_rate(&mut self) {
        let total = self.hits + self.misses;
        self.hit_rate = if total > 0 { self.hits as f64 / total as f64 } else { 0.0 };
    }
    
    #[inline]
    pub fn reset(&mut self) {
        self.hits = 0;
        self.misses = 0;
        self.evictions = 0;
        self.insertions = 0;
        self.hit_rate = 0.0;
    }
}

#[derive(Debug, Error)]
pub enum FeatureStoreError {
    #[error("Cache capacity exceeded")] CapacityExceeded,
    #[error("Feature not found")] NotFound,
    #[error("Feature has expired")] Expired,
}

/// Cache key combining token ID and version for cache isolation
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct FeatureKey {
    pub token_id: String,
    pub version: u32,
}

impl FeatureKey {
    pub fn new(token_id: TokenId, version: u32) -> Self {
        Self { token_id: token_id.0, version }
    }
}

/// Main Feature Store with LRU caching
#[derive(Debug)]
pub struct FeatureStore {
    cache: HashMap<FeatureKey, CachedFeature>,
    lru_order: VecDeque<FeatureKey>,
    config: FeatureStoreConfig,
    stats: Option<CacheStats>,
}

impl FeatureStore {
    pub fn new() -> Self {
        Self::with_config(FeatureStoreConfig::default())
    }

    pub fn with_config(config: FeatureStoreConfig) -> Self {
        let stats = if config.enable_stats { Some(CacheStats::default()) } else { None };
        let mut store = Self { cache: HashMap::new(), lru_order: VecDeque::new(), config, stats };
        if let Some(ref mut s) = store.stats {
            s.set_capacity(store.config.capacity);
        }
        store
    }

    #[inline]
    fn current_time_us(&self) -> i64 {
        std::time::SystemTime::UNIX_EPOCH
            .elapsed()
            .map(|d| d.as_micros() as i64)
            .unwrap_or(0)
    }

    fn shrink_to_fit(&mut self) {
        while self.lru_order.len() > self.config.capacity {
            if let Some(key) = self.lru_order.pop_front() {
                self.cache.remove(&key);
                if let Some(ref mut s) = self.stats {
                    s.record_eviction();
                }
            }
        }
    }

    pub fn get(&mut self, key: &FeatureKey) -> Option<&CachedFeature> {
        let now = self.current_time_us();
        
        // Only check expiration for non-0 TTL
        if self.config.ttl_seconds > 0 {
            // Check if entry exists and is expired - if so, don't return it
            if let Some(cached) = self.cache.get(key) {
                if cached.is_expired(now) {
                    // Expired - don't return it, let it be cleaned up later
                    if let Some(ref mut s) = self.stats {
                        s.record_miss();
                    }
                    return None;
                }
            }
        }
        
        // Entry exists and is valid
        if let Some(cached) = self.cache.get(key) {
            // Move to end (most recently used)
            self.lru_order.retain(|k| k != key);
            self.lru_order.push_back(key.clone());
            if let Some(ref mut s) = self.stats {
                s.record_hit();
            }
            return Some(cached);
        }
        
        if let Some(ref mut s) = self.stats {
            s.record_miss();
        }
        None
    }

    pub fn get_mut(&mut self, key: &FeatureKey) -> Option<&mut CachedFeature> {
        let now = self.current_time_us();
        
        // Check expiration first
        if self.config.ttl_seconds > 0 {
            if let Some(cached) = self.cache.get(key) {
                if cached.is_expired(now) {
                    if let Some(ref mut s) = self.stats {
                        s.record_miss();
                    }
                    return None;
                }
            }
        }
        
        // Get mutable reference
        if let Some(cached) = self.cache.get_mut(key) {
            self.lru_order.retain(|k| k != key);
            self.lru_order.push_back(key.clone());
            if let Some(ref mut s) = self.stats {
                s.record_hit();
            }
            return Some(cached);
        }
        
        if let Some(ref mut s) = self.stats {
            s.record_miss();
        }
        None
    }

    pub fn insert(&mut self, key: FeatureKey, feature: ExtractedFeatureVector) {
        let now = self.current_time_us();
        let cached = CachedFeature::new(feature, self.config.ttl_seconds, now);
        self.cache.insert(key.clone(), cached);
        self.lru_order.retain(|k| k != &key);
        self.lru_order.push_back(key);
        
        if let Some(ref mut s) = self.stats {
            s.record_insertion();
        }
        
        self.shrink_to_fit();
        
        if let Some(ref mut s) = self.stats {
            s.set_size(self.cache.len());
        }
    }

    pub fn get_or_compute<F>(&mut self, key: &FeatureKey, compute: F) -> CachedFeature
    where
        F: FnOnce() -> ExtractedFeatureVector,
    {
        let now = self.current_time_us();
        
        // Check cache
        match self.cache.get(key) {
            Some(cached) if self.config.ttl_seconds == 0 || !cached.is_expired(now) => {
                // Cache hit
                self.lru_order.retain(|k| k != key);
                self.lru_order.push_back(key.clone());
                if let Some(ref mut s) = self.stats {
                    s.record_hit();
                }
                return self.cache.get(key).unwrap().clone();
            }
            Some(_) | None => {
                // Cache miss or expired - compute and insert
                let feature = compute();
                let cached = CachedFeature::new(feature, self.config.ttl_seconds, now);
                self.cache.insert(key.clone(), cached);
                self.lru_order.retain(|k| k != key);
                self.lru_order.push_back(key.clone());
                
                if let Some(ref mut s) = self.stats {
                    s.record_miss();
                    s.record_insertion();
                }
                
                self.shrink_to_fit();
                
                if let Some(ref mut s) = self.stats {
                    s.set_size(self.cache.len());
                }
                
                return self.cache.get(key).unwrap().clone();
            }
        }
    }

    pub fn contains(&self, key: &FeatureKey) -> bool {
        self.cache.contains_key(key)
    }

    pub fn remove(&mut self, key: &FeatureKey) -> Option<CachedFeature> {
        self.lru_order.retain(|k| k != key);
        let result = self.cache.remove(key);
        if let Some(ref mut s) = self.stats {
            s.set_size(self.cache.len());
        }
        result
    }

    pub fn clear(&mut self) {
        self.cache.clear();
        self.lru_order.clear();
        if let Some(ref mut s) = self.stats {
            s.set_size(0);
        }
    }

    pub fn reset_stats(&mut self) {
        if let Some(ref mut s) = self.stats {
            s.reset();
            s.set_capacity(self.config.capacity);
        }
    }

    pub fn stats(&self) -> Option<&CacheStats> {
        self.stats.as_ref()
    }

    pub fn len(&self) -> usize {
        self.cache.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }

    pub fn capacity(&self) -> usize {
        self.config.capacity
    }

    pub fn utilization(&self) -> f64 {
        self.cache.len() as f64 / self.config.capacity as f64
    }

    pub fn peek_lru(&mut self) -> Option<(&FeatureKey, &CachedFeature)> {
        self.lru_order.front().and_then(|key| self.cache.get(key).map(|c| (key, c)))
    }
    
    /// Remove expired entries from the cache (for cleanup)
    pub fn remove_expired(&mut self) -> usize {
        if self.config.ttl_seconds == 0 {
            return 0;
        }
        
        let now = self.current_time_us();
        let mut removed = 0;
        let expired_keys: Vec<FeatureKey> = self.cache
            .iter()
            .filter(|(_, cached)| cached.is_expired(now))
            .map(|(k, _)| k.clone())
            .collect();
        
        for key in expired_keys {
            self.cache.remove(&key);
            self.lru_order.retain(|k| k != &key);
            removed += 1;
        }
        
        if let Some(ref mut s) = self.stats {
            s.set_size(self.cache.len());
        }
        
        removed
    }
}

impl Default for FeatureStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Thread-safe wrapper for FeatureStore using Arc<Mutex<>>
#[derive(Debug, Clone)]
pub struct ThreadSafeFeatureStore {
    inner: Arc<Mutex<FeatureStore>>,
}

impl ThreadSafeFeatureStore {
    pub fn new(store: FeatureStore) -> Self {
        Self {
            inner: Arc::new(Mutex::new(store)),
        }
    }

    pub fn with_config(config: FeatureStoreConfig) -> Self {
        Self::new(FeatureStore::with_config(config))
    }

    pub fn get(&self, key: &FeatureKey) -> Option<CachedFeature> {
        let mut store = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        store.get(key).cloned()
    }

    pub fn insert(&self, key: FeatureKey, feature: ExtractedFeatureVector) {
        let mut store = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        store.insert(key, feature);
    }

    pub fn get_or_compute<F>(&self, key: FeatureKey, compute: F) -> CachedFeature
    where
        F: FnOnce() -> ExtractedFeatureVector,
    {
        let mut store = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        store.get_or_compute(&key, compute)
    }

    pub fn contains(&self, key: &FeatureKey) -> bool {
        let store = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        store.contains(key)
    }

    pub fn remove(&self, key: &FeatureKey) -> Option<CachedFeature> {
        let mut store = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        store.remove(key)
    }

    pub fn clear(&self) {
        let mut store = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        store.clear();
    }

    pub fn stats(&self) -> Option<CacheStats> {
        let store = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        store.stats().cloned()
    }

    pub fn len(&self) -> usize {
        let store = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        store.len()
    }

    pub fn is_empty(&self) -> bool {
        let store = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        store.is_empty()
    }

    pub fn capacity(&self) -> usize {
        let store = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        store.capacity()
    }

    pub fn into_inner(self) -> FeatureStore {
        Arc::try_unwrap(self.inner)
            .expect("Cannot unwrap Arc")
            .into_inner()
            .expect("Mutex poisoned")
    }
}

impl From<FeatureStore> for ThreadSafeFeatureStore {
    fn from(store: FeatureStore) -> Self {
        Self::new(store)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mtrader_ml::ExtractedFeatureVector;

    fn create_test_feature(timestamp_us: i64, token_id: &str) -> ExtractedFeatureVector {
        let mut feature = ExtractedFeatureVector::new(timestamp_us, TokenId(token_id.to_string()));
        feature.features[0] = 0.55;
        feature.features[1] = 0.001;
        feature
    }

    fn create_test_key(token_id: &str, version: u32) -> FeatureKey {
        FeatureKey::new(TokenId(token_id.to_string()), version)
    }

    #[test]
    fn test_cache_hit_miss() {
        let mut store = FeatureStore::new();
        let key = create_test_key("token1", 1);
        
        // Initially miss
        assert!(store.get(&key).is_none());
        
        // Insert and hit
        store.insert(key.clone(), create_test_feature(1000, "token1"));
        assert!(store.get(&key).is_some());
        
        let stats = store.stats().unwrap();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
    }

    #[test]
    fn test_lru_eviction() {
        let mut store = FeatureStore::with_config(FeatureStoreConfig::with_capacity(3));
        
        for i in 0..3 {
            let token = format!("token{}", i);
            store.insert(
                create_test_key(&token, 1),
                create_test_feature(1000 + i, &token),
            );
        }
        
        assert_eq!(store.len(), 3);
        
        // Access token0 to make it recently used
        let key0 = create_test_key("token0", 1);
        store.get(&key0);
        
        // Insert new entry - should evict token1 (LRU)
        let key3 = create_test_key("token3", 1);
        store.insert(key3.clone(), create_test_feature(2000, "token3"));
        
        assert_eq!(store.len(), 3);
        assert!(store.contains(&key0));
        assert!(!store.contains(&create_test_key("token1", 1)));
    }

    #[test]
    fn test_ttl_expiration() {
        let mut store = FeatureStore::with_config(FeatureStoreConfig::with_ttl(1));
        let key = create_test_key("token1", 1);
        
        store.insert(key.clone(), create_test_feature(1000, "token1"));
        assert!(store.get(&key).is_some());
        
        // Wait for expiration
        std::thread::sleep(std::time::Duration::from_millis(1100));
        
        // After expiration, get should return None
        assert!(store.get(&key).is_none());
        
        // remove_expired should clean up
        let removed = store.remove_expired();
        assert_eq!(removed, 1);
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn test_get_or_compute() {
        let mut store = FeatureStore::new();
        let key = create_test_key("token1", 1);
        let mut compute_count = 0;
        
        let result = store.get_or_compute(&key, || {
            compute_count += 1;
            create_test_feature(1000, "token1")
        });
        
        assert_eq!(compute_count, 1);
        assert_eq!(result.feature().timestamp_us, 1000);
        
        // Second call should use cached value
        let result2 = store.get_or_compute(&key, || {
            compute_count += 1;
            create_test_feature(2000, "token1")
        });
        
        assert_eq!(compute_count, 1);
        assert_eq!(result2.feature().timestamp_us, 1000);
    }

    #[test]
    fn test_statistics_tracking() {
        let mut store = FeatureStore::with_config(FeatureStoreConfig {
            capacity: 10,
            ttl_seconds: 0,
            enable_stats: true,
        });
        
        store.insert(create_test_key("token1", 1), create_test_feature(1000, "token1"));
        store.get(&create_test_key("token1", 1));
        store.get(&create_test_key("token2", 1));
        
        let stats = store.stats().unwrap();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.insertions, 1);
    }

    #[test]
    fn test_clear_and_reset() {
        let mut store = FeatureStore::new();
        
        for i in 0..5 {
            let token = format!("token{}", i);
            store.insert(
                create_test_key(&token, 1),
                create_test_feature((1000 + i) as i64, &token),
            );
        }
        
        assert_eq!(store.len(), 5);
        store.clear();
        assert_eq!(store.len(), 0);
        assert!(store.is_empty());
        
        let stats = store.stats().unwrap();
        assert_eq!(stats.insertions, 5);
        
        store.reset_stats();
        let stats = store.stats().unwrap();
        assert_eq!(stats.hits, 0);
    }

    #[test]
    fn test_thread_safe_store() {
        use std::thread;
        let store = ThreadSafeFeatureStore::with_config(FeatureStoreConfig::with_capacity(1000));
        
        let handles: Vec<_> = (0..4)
            .map(|i| {
                let store = store.clone();
                thread::spawn(move || {
                    for j in 0..100 {
                        let token = format!("token{}", i * 100 + j);
                        store.insert(
                            create_test_key(&token, 1),
                            create_test_feature((i * 100 + j) as i64, &token),
                        );
                    }
                })
            })
            .collect();
        
        for handle in handles {
            handle.join().unwrap();
        }
        
        assert_eq!(store.len(), 400);
    }

    #[test]
    fn test_cached_feature_metadata() {
        let now = 1768157345931993i64;
        let mut cached = CachedFeature::new(create_test_feature(1000, "token1"), 0, now);
        
        assert_eq!(cached.cached_at, now);
        cached.mark_accessed();
        assert_eq!(cached.access_count, 2);
        
        cached.metadata = Some(FeatureMetadata {
            source: "test".to_string(),
            compute_time_us: 5.5,
            context: None,
        });
        
        assert!(cached.metadata.is_some());
    }

    #[test]
    fn test_config_presets() {
        let hft = FeatureStoreConfig::hft();
        assert_eq!(hft.capacity, 1_000);
        assert_eq!(hft.ttl_seconds, 0);
        
        let backtest = FeatureStoreConfig::backtest();
        assert_eq!(backtest.capacity, 100_000);
        assert_eq!(backtest.ttl_seconds, 0);
    }

    #[test]
    fn test_utilization() {
        let mut store = FeatureStore::with_config(FeatureStoreConfig::with_capacity(100));
        
        assert_eq!(store.utilization(), 0.0);
        
        for i in 0..50 {
            let token = format!("token{}", i);
            store.insert(
                create_test_key(&token, 1),
                create_test_feature(i as i64, &token),
            );
        }
        
        let util = store.utilization();
        assert!((util - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_peek_lru() {
        let mut store = FeatureStore::new();
        store.insert(create_test_key("token1", 1), create_test_feature(1000, "token1"));
        store.insert(create_test_key("token2", 1), create_test_feature(1001, "token2"));
        
        let (lru_key, _) = store.peek_lru().unwrap();
        assert_eq!(lru_key.token_id, "token1");
    }

    #[test]
    fn test_debug_hit() {
        use std::collections::HashMap;
        
        let mut store = FeatureStore::new();
        let key = create_test_key("token1", 1);
        
        println!("=== Debug Test ===");
        println!("Default TTL: {}", store.config.ttl_seconds);
        println!("Initial key: {:?}", key);
        
        // Direct hashmap test
        let mut map: HashMap<FeatureKey, i32> = HashMap::new();
        map.insert(key.clone(), 42);
        println!("Direct hashmap get: {:?}", map.get(&key));
        
        println!("Initial contains: {}", store.contains(&key));
        
        let feature = create_test_feature(1000, "token1");
        println!("Inserting feature with timestamp: {}", feature.timestamp_us);
        println!("Current time us: {}", store.current_time_us());
        
        store.insert(key.clone(), feature);
        
        println!("After insert, contains: {}", store.contains(&key));
        println!("Store len: {}", store.len());
        println!("LRU order len: {}", store.lru_order.len());
        
        // Direct cache test
        println!("Direct cache get: {:?}", store.cache.get(&key));
        
        let result = store.get(&key);
        println!("Get result: {:?}", result);
        
        assert!(result.is_some(), "Should find inserted feature");
    }
    
    #[test]
    fn test_debug_hash() {
        use std::collections::HashMap;
        
        // Test FeatureKey hash/eq directly
        let key1 = create_test_key("token1", 1);
        let key2 = create_test_key("token1", 1);
        
        // Test equality
        assert_eq!(key1, key2, "Keys should be equal");
        
        // Test hash map
        let mut map: HashMap<FeatureKey, i32> = HashMap::new();
        map.insert(key1.clone(), 42);
        
        // Check if key2 can find it
        let found = map.get(&key2);
        println!("key1 == key2: {}", key1 == key2);
        println!("found value: {:?}", found);
        
        assert!(found.is_some(), "Should find key2 after inserting key1");
        assert_eq!(found.unwrap(), &42);
    }
    
    #[test]
    fn test_multiple_versions() {
        let mut store = FeatureStore::new();
        let key_v1 = create_test_key("token1", 1);
        let key_v2 = create_test_key("token1", 2);
        
        store.insert(key_v1.clone(), create_test_feature(1000, "token1"));
        store.insert(key_v2.clone(), create_test_feature(1001, "token1"));
        
        assert_eq!(store.len(), 2);
        
        let cached_v1 = store.get(&key_v1).unwrap().clone();
        let cached_v2 = store.get(&key_v2).unwrap().clone();
        
        assert_eq!(cached_v1.feature().timestamp_us, 1000);
        assert_eq!(cached_v2.feature().timestamp_us, 1001);
    }

    #[test]
    fn test_remove_entry() {
        let mut store = FeatureStore::new();
        let key = create_test_key("token1", 1);
        
        store.insert(key.clone(), create_test_feature(1000, "token1"));
        assert_eq!(store.len(), 1);
        
        let removed = store.remove(&key);
        assert!(removed.is_some());
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn test_get_mut() {
        let mut store = FeatureStore::new();
        let key = create_test_key("token1", 1);
        
        store.insert(key.clone(), create_test_feature(1000, "token1"));
        let cached = store.get_mut(&key);
        assert!(cached.is_some());
        
        cached.unwrap().feature_mut().features[0] = 0.99;
        
        let cached2 = store.get(&key).unwrap().clone();
        assert_eq!(cached2.feature().features[0], 0.99);
    }

    #[test]
    fn test_disabled_stats() {
        let mut store = FeatureStore::with_config(FeatureStoreConfig {
            capacity: 10,
            ttl_seconds: 0,
            enable_stats: false,
        });
        
        let key = create_test_key("token1", 1);
        store.insert(key.clone(), create_test_feature(1000, "token1"));
        store.get(&key);
        
        assert!(store.stats().is_none());
    }
    
    #[test]
    fn test_remove_expired() {
        let mut store = FeatureStore::with_config(FeatureStoreConfig::with_ttl(1));
        
        for i in 0..5 {
            let token = format!("token{}", i);
            store.insert(
                create_test_key(&token, 1),
                create_test_feature(1000 + i, &token),
            );
        }
        
        assert_eq!(store.len(), 5);
        
        // Wait for expiration
        std::thread::sleep(std::time::Duration::from_millis(1100));
        
        let removed = store.remove_expired();
        assert_eq!(removed, 5);
        assert_eq!(store.len(), 0);
    }
}
