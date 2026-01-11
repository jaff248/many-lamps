//! Data Pipeline Crate
//!
//! High-performance data processing pipeline for trading systems.

pub mod feature_store;
pub mod quality;
pub mod storage;

pub use feature_store::{FeatureStore, FeatureStoreConfig, ThreadSafeFeatureStore, CacheStats, CachedFeature, FeatureKey, FeatureMetadata};
pub use quality::{DataQualityMonitor, DataQualityConfig, DataQualityMetrics, AnomalyType, FieldStats};

// Re-export storage types (StorageError is defined in storage module)
pub use storage::{
    TimeSeriesStorage,
    TimeSeriesStorageConfig,
    PartitionKey,
    PartitionScheme,
    CompressionCodec,
    DataTypeIdentifier,
    PartitionMetadata,
    SchemaVersion,
    StorageStats,
};
