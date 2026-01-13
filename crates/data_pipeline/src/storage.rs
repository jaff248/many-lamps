//! Time-Series Storage with Parquet Partitioning
//!
//! Production-grade time-series storage for historical analysis using columnar Parquet format.

use arrow::array::{ArrayRef, Float64Array, Int64Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use chrono::{DateTime, TimeZone, Utc, Datelike, Timelike};
use many_lamps_core::{NormalizedMarketData, SignalData};
use mtrader_ml::ExtractedFeatureVector;
use parquet::basic::{Compression, GzipLevel, ZstdLevel};
use parquet::file::properties::WriterProperties;
use std::collections::hash_map::{Entry, HashMap};
use std::fs::{self, File};
use std::path::PathBuf;
use std::sync::Arc;

/// Compression algorithm for Parquet files
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CompressionCodec {
    None,
    Snappy,
    Gzip,
    Zstd,
}

impl Default for CompressionCodec {
    fn default() -> Self {
        CompressionCodec::Snappy
    }
}

impl From<CompressionCodec> for Compression {
    fn from(codec: CompressionCodec) -> Self {
        match codec {
            CompressionCodec::None => Compression::UNCOMPRESSED,
            CompressionCodec::Snappy => Compression::SNAPPY,
            CompressionCodec::Gzip => Compression::GZIP(GzipLevel::default()),
            CompressionCodec::Zstd => Compression::ZSTD(ZstdLevel::default()),
        }
    }
}

/// Time partitioning scheme for organizing data files
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PartitionScheme {
    Hourly,
    Daily,
    Weekly,
    Monthly,
}

impl Default for PartitionScheme {
    fn default() -> Self {
        PartitionScheme::Daily
    }
}

/// Partition key for time-based file organization
#[derive(Debug, Clone, Hash, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PartitionKey {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub week: u32,
}

impl PartitionKey {
    /// Create a new partition key from a Unix timestamp in microseconds
    pub fn from_timestamp_us(timestamp_us: i64) -> Self {
        let datetime = DateTime::from_timestamp(
            timestamp_us / 1_000_000,
            (timestamp_us % 1_000_000) as u32 * 1000,
        )
        .unwrap_or_else(|| Utc.timestamp_opt(0, 0).single().unwrap());

        let (year, month, day, hour, week) = (
            datetime.year(),
            datetime.month(),
            datetime.day(),
            datetime.hour(),
            datetime.iso_week().week(),
        );

        Self {
            year,
            month,
            day,
            hour,
            week,
        }
    }

    /// Generate the partition path relative to a base path
    pub fn to_path(&self, scheme: PartitionScheme) -> PathBuf {
        match scheme {
            PartitionScheme::Hourly => {
                PathBuf::from(format!("year={}/month={:02}/day={:02}/hour={:02}", self.year, self.month, self.day, self.hour))
            }
            PartitionScheme::Daily => {
                PathBuf::from(format!("year={}/month={:02}/day={:02}", self.year, self.month, self.day))
            }
            PartitionScheme::Weekly => {
                PathBuf::from(format!("year={}/week={:02}", self.year, self.week))
            }
            PartitionScheme::Monthly => {
                PathBuf::from(format!("year={}/month={:02}", self.year, self.month))
            }
        }
    }
}

/// Configuration for time-series storage
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TimeSeriesStorageConfig {
    pub base_path: PathBuf,
    pub partition_scheme: PartitionScheme,
    pub compression: CompressionCodec,
    pub max_rows_per_file: usize,
    pub enable_statistics: bool,
    pub enable_dictionary_encoding: bool,
}

impl Default for TimeSeriesStorageConfig {
    fn default() -> Self {
        Self {
            base_path: PathBuf::from("data/timeseries"),
            partition_scheme: PartitionScheme::Daily,
            compression: CompressionCodec::Snappy,
            max_rows_per_file: 1_000_000,
            enable_statistics: true,
            enable_dictionary_encoding: true,
        }
    }
}

impl TimeSeriesStorageConfig {
    pub fn new(base_path: PathBuf) -> Self {
        Self {
            base_path,
            ..Default::default()
        }
    }

    pub fn with_partition_scheme(mut self, scheme: PartitionScheme) -> Self {
        self.partition_scheme = scheme;
        self
    }

    pub fn with_compression(mut self, compression: CompressionCodec) -> Self {
        self.compression = compression;
        self
    }

    pub fn with_max_rows_per_file(mut self, max_rows: usize) -> Self {
        self.max_rows_per_file = max_rows;
        self
    }
}

/// Identifies the type of data being stored
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DataTypeIdentifier {
    MarketData,
    Features,
    Signals,
}

/// Schema versions for tracking schema evolution
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchemaVersion {
    pub major: u16,
    pub minor: u16,
}

impl SchemaVersion {
    pub fn new(major: u16, minor: u16) -> Self {
        Self { major, minor }
    }

    pub fn current() -> Self {
        Self::new(1, 0)
    }
}

/// Partition metadata for tracking file information
#[derive(Debug, Clone)]
pub struct PartitionMetadata {
    pub key: PartitionKey,
    pub row_count: usize,
    pub file_count: usize,
    pub schema_version: SchemaVersion,
    pub min_timestamp_us: i64,
    pub max_timestamp_us: i64,
    pub total_bytes: u64,
}

/// Active partition writer tracking
struct ActivePartitionWriter {
    key: PartitionKey,
    current_file: PathBuf,
    row_count: usize,
    writer: parquet::arrow::ArrowWriter<File>,
    file_count: usize,
}

/// Cached partition information
struct PartitionCache {
    metadata: HashMap<PartitionKey, PartitionMetadata>,
}

/// Statistics for storage operations
#[derive(Debug, Clone, Default)]
pub struct StorageStats {
    pub rows_written: usize,
    pub files_created: usize,
    pub bytes_written: u64,
    pub flush_count: usize,
    pub active_partitions: usize,
    pub total_partitions: usize,
}

/// Storage errors
#[derive(thiserror::Error, Debug)]
pub enum StorageError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Arrow error: {0}")]
    Arrow(#[from] arrow::error::ArrowError),

    #[error("Parquet error: {0}")]
    Parquet(#[from] parquet::errors::ParquetError),
}

/// Main time-series storage struct
pub struct TimeSeriesStorage {
    config: TimeSeriesStorageConfig,
    data_type: DataTypeIdentifier,
    active_writers: HashMap<PartitionKey, ActivePartitionWriter>,
    metadata_cache: PartitionCache,
    stats: StorageStats,
    schema: Arc<Schema>,
    schema_version: SchemaVersion,
}

impl TimeSeriesStorage {
    /// Create a new time-series storage instance
    pub fn new(config: TimeSeriesStorageConfig, data_type: DataTypeIdentifier) -> Result<Self, StorageError> {
        let base_path = &config.base_path;
        if !base_path.exists() {
            fs::create_dir_all(base_path).map_err(StorageError::Io)?;
        }

        let schema = match data_type {
            DataTypeIdentifier::MarketData => Self::market_data_schema(),
            DataTypeIdentifier::Features => Self::features_schema(),
            DataTypeIdentifier::Signals => Self::signals_schema(),
        };

        Ok(Self {
            config,
            data_type,
            active_writers: HashMap::new(),
            metadata_cache: PartitionCache {
                metadata: HashMap::new(),
            },
            stats: StorageStats::default(),
            schema,
            schema_version: SchemaVersion::current(),
        })
    }

    /// Get the market data schema
    fn market_data_schema() -> Arc<Schema> {
        Arc::new(Schema::new(vec![
            Field::new("timestamp_us", DataType::Int64, false),
            Field::new("token_id", DataType::Utf8, false),
            Field::new("mid_price", DataType::Float64, false),
            Field::new("spread", DataType::Float64, false),
            Field::new("bid_size", DataType::Float64, false),
            Field::new("ask_size", DataType::Float64, false),
            Field::new("imbalance", DataType::Float64, false),
            Field::new("volatility", DataType::Float64, false),
            Field::new("best_bid", DataType::Float64, true),
            Field::new("best_ask", DataType::Float64, true),
        ]))
    }

    /// Get the features schema
    fn features_schema() -> Arc<Schema> {
        Arc::new(Schema::new(vec![
            Field::new("timestamp_us", DataType::Int64, false),
            Field::new("token_id", DataType::Utf8, false),
            Field::new("f0_mid_price", DataType::Float64, false),
            Field::new("f1_price_ema", DataType::Float64, false),
            Field::new("f2_price_momentum_5", DataType::Float64, false),
            Field::new("f3_price_momentum_15", DataType::Float64, false),
            Field::new("f4_price_momentum_30", DataType::Float64, false),
            Field::new("f5_price_volatility", DataType::Float64, false),
            Field::new("f6_spread", DataType::Float64, false),
            Field::new("f7_spread_ema", DataType::Float64, false),
            Field::new("f8_spread_volatility", DataType::Float64, false),
            Field::new("f9_relative_spread", DataType::Float64, false),
            Field::new("f10_bid_size", DataType::Float64, false),
            Field::new("f11_ask_size", DataType::Float64, false),
            Field::new("f12_bid_ask_ratio", DataType::Float64, false),
            Field::new("f13_imbalance", DataType::Float64, false),
            Field::new("f14_imbalance_ema", DataType::Float64, false),
            Field::new("f15_volume_ema", DataType::Float64, false),
            Field::new("f16_volume_rate", DataType::Float64, false),
            Field::new("f17_trade_rate", DataType::Float64, false),
            Field::new("f18_price_return_1m", DataType::Float64, false),
            Field::new("f19_price_return_5m", DataType::Float64, false),
            Field::new("f20_spread_rank", DataType::Float64, false),
            Field::new("f21_volume_imbalance", DataType::Float64, false),
            Field::new("f22_bid_size_norm", DataType::Float64, false),
            Field::new("f23_ask_size_norm", DataType::Float64, false),
        ]))
    }

    /// Get the signals schema
    fn signals_schema() -> Arc<Schema> {
        Arc::new(Schema::new(vec![
            Field::new("timestamp_us", DataType::Int64, false),
            Field::new("token_id", DataType::Utf8, false),
            Field::new("model_id", DataType::Utf8, false),
            Field::new("value", DataType::Float64, false),
            Field::new("signal_type", DataType::Utf8, false),
            Field::new("confidence", DataType::Float64, false),
            Field::new("feature_importance", DataType::Utf8, true),
        ]))
    }

    /// Get the path for a data type
    fn data_type_path(&self) -> &'static str {
        match self.data_type {
            DataTypeIdentifier::MarketData => "market_data",
            DataTypeIdentifier::Features => "features",
            DataTypeIdentifier::Signals => "signals",
        }
    }

    /// Get or create writer for a partition
    fn get_or_create_writer(&mut self, timestamp_us: i64) -> Result<&mut ActivePartitionWriter, StorageError> {
        let key = PartitionKey::from_timestamp_us(timestamp_us);
        let _active_partitions_before = self.active_writers.len();
        let data_type_path = self.data_type_path();
        
        match self.active_writers.entry(key.clone()) {
            Entry::Occupied(existing) => Ok(existing.into_mut()),
            Entry::Vacant(vacant) => {
                let partition_path = key.to_path(self.config.partition_scheme);
                let file_path = self.config.base_path
                    .join(data_type_path)
                    .join(&partition_path);

                fs::create_dir_all(&file_path).map_err(StorageError::Io)?;

                let file_count = self.metadata_cache.metadata.get(&key)
                    .map(|m| m.file_count)
                    .unwrap_or(0);

                let file_name = format!("{}_{:04}.parquet", data_type_path, file_count);
                let full_path = file_path.join(&file_name);

                let props = WriterProperties::builder()
                    .set_compression(self.config.compression.into())
                    .set_max_row_group_size(100_000)
                    .build();

                let file = File::create(&full_path).map_err(StorageError::Io)?;
                let writer = parquet::arrow::ArrowWriter::try_new(file, self.schema.clone(), Some(props))
                    .map_err(StorageError::Parquet)?;

                let partition_writer = ActivePartitionWriter {
                    key: key.clone(),
                    current_file: full_path,
                    row_count: 0,
                    writer,
                    file_count,
                };

                self.stats.files_created += 1;

                Ok(vacant.insert(partition_writer))
            }
        }
    }

    /// Build market record batch
    fn build_market_record_batch(
        &self,
        timestamp_us: &[i64],
        token_id: &[String],
        mid_price: &[f64],
        spread: &[f64],
        bid_size: &[f64],
        ask_size: &[f64],
        imbalance: &[f64],
        volatility: &[f64],
        best_bid: &[Option<f64>],
        best_ask: &[Option<f64>],
    ) -> Result<RecordBatch, StorageError> {
        let timestamp_arr = Int64Array::from(timestamp_us.to_vec());
        let token_arr: Vec<&str> = token_id.iter().map(|s| s.as_str()).collect();
        let mid_arr = Float64Array::from(mid_price.to_vec());
        let spread_arr = Float64Array::from(spread.to_vec());
        let bid_arr = Float64Array::from(bid_size.to_vec());
        let ask_arr = Float64Array::from(ask_size.to_vec());
        let imb_arr = Float64Array::from(imbalance.to_vec());
        let vol_arr = Float64Array::from(volatility.to_vec());
        let best_bid_arr = Float64Array::from(best_bid.to_vec());
        let best_ask_arr = Float64Array::from(best_ask.to_vec());

        let arrays: Vec<ArrayRef> = vec![
            Arc::new(timestamp_arr),
            Arc::new(StringArray::from(token_arr)),
            Arc::new(mid_arr),
            Arc::new(spread_arr),
            Arc::new(bid_arr),
            Arc::new(ask_arr),
            Arc::new(imb_arr),
            Arc::new(vol_arr),
            Arc::new(best_bid_arr),
            Arc::new(best_ask_arr),
        ];
        RecordBatch::try_new(self.schema.clone(), arrays).map_err(StorageError::Arrow)
    }

    /// Build features record batch
    fn build_features_record_batch(
        &self,
        timestamp_us: &[i64],
        token_id: &[String],
        features: &[Vec<f64>],
    ) -> Result<RecordBatch, StorageError> {
        let token_arr: Vec<&str> = token_id.iter().map(|s| s.as_str()).collect();
        let mut arrays: Vec<ArrayRef> = vec![
            Arc::new(Int64Array::from(timestamp_us.to_vec())),
            Arc::new(StringArray::from(token_arr)),
        ];
        for feature_vec in features {
            arrays.push(Arc::new(Float64Array::from(feature_vec.clone())));
        }
        RecordBatch::try_new(self.schema.clone(), arrays).map_err(StorageError::Arrow)
    }

    /// Build signals record batch
    fn build_signals_record_batch(
        &self,
        timestamp_us: &[i64],
        token_id: &[String],
        model_id: &[String],
        value: &[f64],
        signal_type: &[String],
        confidence: &[f64],
        feature_importance: &[Option<String>],
    ) -> Result<RecordBatch, StorageError> {
        let token_arr: Vec<&str> = token_id.iter().map(|s| s.as_str()).collect();
        let model_arr: Vec<&str> = model_id.iter().map(|s| s.as_str()).collect();
        let signal_arr: Vec<&str> = signal_type.iter().map(|s| s.as_str()).collect();
        let importance_arr: Vec<&str> = feature_importance.iter().map(|o| o.as_ref().map(|s| s.as_str()).unwrap_or("")).collect();
        
        let arrays: Vec<ArrayRef> = vec![
            Arc::new(Int64Array::from(timestamp_us.to_vec())),
            Arc::new(StringArray::from(token_arr)),
            Arc::new(StringArray::from(model_arr)),
            Arc::new(Float64Array::from(value.to_vec())),
            Arc::new(StringArray::from(signal_arr)),
            Arc::new(Float64Array::from(confidence.to_vec())),
            Arc::new(StringArray::from(importance_arr)),
        ];
        RecordBatch::try_new(self.schema.clone(), arrays).map_err(StorageError::Arrow)
    }

    /// Write normalized market data to storage
    pub fn write_market_data(&mut self, data: &NormalizedMarketData) -> Result<(), StorageError> {
        let timestamp_us = vec![data.timestamp_us];
        let token_id = vec![data.token_id.0.clone()];
        let mid_price = vec![data.mid_price];
        let spread = vec![data.spread];
        let bid_size = vec![data.bid_size];
        let ask_size = vec![data.ask_size];
        let imbalance = vec![data.imbalance];
        let volatility = vec![data.volatility];
        let best_bid = vec![data.best_bid];
        let best_ask = vec![data.best_ask];

        let batch = self.build_market_record_batch(
            &timestamp_us, &token_id, &mid_price, &spread, &bid_size, &ask_size,
            &imbalance, &volatility, &best_bid, &best_ask,
        )?;

        let key = PartitionKey::from_timestamp_us(data.timestamp_us);
        let writer = self.get_or_create_writer(data.timestamp_us)?;
        writer.writer.write(&batch)?;
        writer.row_count += 1;
        self.stats.rows_written += 1;

        self.update_partition_metadata(&key, 1, data.timestamp_us, data.timestamp_us)
    }

    /// Write a batch of market data
    pub fn write_market_data_batch(&mut self, batch: &[NormalizedMarketData]) -> Result<(), StorageError> {
        if batch.is_empty() {
            return Ok(());
        }

        // Group by partition
        let mut partition_data: HashMap<PartitionKey, Vec<usize>> = HashMap::new();
        for (idx, item) in batch.iter().enumerate() {
            let key = PartitionKey::from_timestamp_us(item.timestamp_us);
            partition_data.entry(key).or_default().push(idx);
        }

        for (key, indices) in partition_data {
            let timestamp_us: Vec<i64> = indices.iter().map(|&i| batch[i].timestamp_us).collect();
            let token_id: Vec<String> = indices.iter().map(|&i| batch[i].token_id.0.clone()).collect();
            let mid_price: Vec<f64> = indices.iter().map(|&i| batch[i].mid_price).collect();
            let spread: Vec<f64> = indices.iter().map(|&i| batch[i].spread).collect();
            let bid_size: Vec<f64> = indices.iter().map(|&i| batch[i].bid_size).collect();
            let ask_size: Vec<f64> = indices.iter().map(|&i| batch[i].ask_size).collect();
            let imbalance: Vec<f64> = indices.iter().map(|&i| batch[i].imbalance).collect();
            let volatility: Vec<f64> = indices.iter().map(|&i| batch[i].volatility).collect();
            let best_bid: Vec<Option<f64>> = indices.iter().map(|&i| batch[i].best_bid).collect();
            let best_ask: Vec<Option<f64>> = indices.iter().map(|&i| batch[i].best_ask).collect();

            let batch_record = self.build_market_record_batch(
                &timestamp_us, &token_id, &mid_price, &spread, &bid_size, &ask_size,
                &imbalance, &volatility, &best_bid, &best_ask,
            )?;

            let writer = self.get_or_create_writer(batch[indices[0]].timestamp_us)?;
            writer.writer.write(&batch_record)?;
            writer.row_count += indices.len();
            self.stats.rows_written += indices.len();

            let min_ts = batch[indices[0]].timestamp_us;
            let max_ts = batch[indices[indices.len() - 1]].timestamp_us;
            self.update_partition_metadata(&key, indices.len(), min_ts, max_ts)?;
        }

        Ok(())
    }

    /// Write features to storage
    pub fn write_features(&mut self, features: &ExtractedFeatureVector) -> Result<(), StorageError> {
        let timestamp_us = vec![features.timestamp_us];
        let token_id = vec![features.token_id.0.clone()];
        let feature_arrays: Vec<Vec<f64>> = (0..24).map(|i| vec![features.features[i]]).collect();

        let batch = self.build_features_record_batch(&timestamp_us, &token_id, &feature_arrays)?;

        let key = PartitionKey::from_timestamp_us(features.timestamp_us);
        let writer = self.get_or_create_writer(features.timestamp_us)?;
        writer.writer.write(&batch)?;
        writer.row_count += 1;
        self.stats.rows_written += 1;

        self.update_partition_metadata(&key, 1, features.timestamp_us, features.timestamp_us)
    }

    /// Write a batch of features
    pub fn write_features_batch(&mut self, batch: &[ExtractedFeatureVector]) -> Result<(), StorageError> {
        if batch.is_empty() {
            return Ok(());
        }

        let mut partition_data: HashMap<PartitionKey, Vec<usize>> = HashMap::new();
        for (idx, item) in batch.iter().enumerate() {
            let key = PartitionKey::from_timestamp_us(item.timestamp_us);
            partition_data.entry(key).or_default().push(idx);
        }

        for (key, indices) in partition_data {
            let timestamp_us: Vec<i64> = indices.iter().map(|&i| batch[i].timestamp_us).collect();
            let token_id: Vec<String> = indices.iter().map(|&i| batch[i].token_id.0.clone()).collect();

            let mut feature_arrays: Vec<Vec<f64>> = (0..24).map(|_| Vec::with_capacity(indices.len())).collect();
            for &idx in &indices {
                for (i, &val) in batch[idx].features.iter().enumerate() {
                    feature_arrays[i].push(val);
                }
            }

            let batch_record = self.build_features_record_batch(&timestamp_us, &token_id, &feature_arrays)?;

            let writer = self.get_or_create_writer(batch[indices[0]].timestamp_us)?;
            writer.writer.write(&batch_record)?;
            writer.row_count += indices.len();
            self.stats.rows_written += indices.len();

            let min_ts = batch[indices[0]].timestamp_us;
            let max_ts = batch[indices[indices.len() - 1]].timestamp_us;
            self.update_partition_metadata(&key, indices.len(), min_ts, max_ts)?;
        }

        Ok(())
    }

    /// Write signals to storage
    pub fn write_signals(&mut self, signals: &SignalData) -> Result<(), StorageError> {
        let timestamp_us = vec![signals.timestamp_us];
        let token_id = vec![signals.token_id.0.clone()];
        let model_id = vec![signals.model_id.clone()];
        let value = vec![signals.value];
        let signal_type = vec![signals.signal_type.to_string()];
        let confidence = vec![signals.confidence];
        let feature_importance = vec![signals.feature_importance.as_ref().map(|fi| serde_json::to_string(fi).unwrap_or_default())];

        let batch = self.build_signals_record_batch(
            &timestamp_us, &token_id, &model_id, &value, &signal_type, &confidence, &feature_importance,
        )?;

        let key = PartitionKey::from_timestamp_us(signals.timestamp_us);
        let writer = self.get_or_create_writer(signals.timestamp_us)?;
        writer.writer.write(&batch)?;
        writer.row_count += 1;
        self.stats.rows_written += 1;

        self.update_partition_metadata(&key, 1, signals.timestamp_us, signals.timestamp_us)
    }

    /// Write a batch of signals
    pub fn write_signals_batch(&mut self, batch: &[SignalData]) -> Result<(), StorageError> {
        if batch.is_empty() {
            return Ok(());
        }

        let mut partition_data: HashMap<PartitionKey, Vec<usize>> = HashMap::new();
        for (idx, item) in batch.iter().enumerate() {
            let key = PartitionKey::from_timestamp_us(item.timestamp_us);
            partition_data.entry(key).or_default().push(idx);
        }

        for (key, indices) in partition_data {
            let timestamp_us: Vec<i64> = indices.iter().map(|&i| batch[i].timestamp_us).collect();
            let token_id: Vec<String> = indices.iter().map(|&i| batch[i].token_id.0.clone()).collect();
            let model_id: Vec<String> = indices.iter().map(|&i| batch[i].model_id.clone()).collect();
            let value: Vec<f64> = indices.iter().map(|&i| batch[i].value).collect();
            let signal_type: Vec<String> = indices.iter().map(|&i| batch[i].signal_type.to_string()).collect();
            let confidence: Vec<f64> = indices.iter().map(|&i| batch[i].confidence).collect();
            let feature_importance: Vec<Option<String>> = indices.iter()
                .map(|&i| batch[i].feature_importance.as_ref().map(|fi| serde_json::to_string(fi).unwrap_or_default()))
                .collect();

            let batch_record = self.build_signals_record_batch(
                &timestamp_us, &token_id, &model_id, &value, &signal_type, &confidence, &feature_importance,
            )?;

            let writer = self.get_or_create_writer(batch[indices[0]].timestamp_us)?;
            writer.writer.write(&batch_record)?;
            writer.row_count += indices.len();
            self.stats.rows_written += indices.len();

            let min_ts = batch[indices[0]].timestamp_us;
            let max_ts = batch[indices[indices.len() - 1]].timestamp_us;
            self.update_partition_metadata(&key, indices.len(), min_ts, max_ts)?;
        }

        Ok(())
    }

    /// Update partition metadata
    fn update_partition_metadata(
        &mut self,
        key: &PartitionKey,
        row_count: usize,
        min_ts: i64,
        max_ts: i64,
    ) -> Result<(), StorageError> {
        let metadata = self.metadata_cache.metadata.entry(key.clone())
            .or_insert(PartitionMetadata {
                key: key.clone(),
                row_count: 0,
                file_count: 0,
                schema_version: self.schema_version,
                min_timestamp_us: i64::MAX,
                max_timestamp_us: i64::MIN,
                total_bytes: 0,
            });

        metadata.row_count += row_count;
        metadata.min_timestamp_us = metadata.min_timestamp_us.min(min_ts);
        metadata.max_timestamp_us = metadata.max_timestamp_us.max(max_ts);

        if let Some(writer) = self.active_writers.get(key) {
            metadata.file_count = writer.file_count + 1;
            if let Ok(metadata_file) = writer.current_file.metadata() {
                metadata.total_bytes = metadata_file.len();
            }
        }

        self.stats.total_partitions = self.metadata_cache.metadata.len();
        Ok(())
    }

    /// Flush all active writers
    pub fn flush(&mut self) -> Result<(), StorageError> {
        let mut flushed_count = 0;
        for writer in self.active_writers.values_mut() {
            writer.writer.flush()?;
            flushed_count += 1;
        }
        self.stats.flush_count += flushed_count;
        Ok(())
    }

    /// Close all writers and finalize storage
    pub fn close(&mut self) -> Result<(), StorageError> {
        self.flush()?;

        let writers: Vec<_> = self.active_writers.drain().collect();
        for (_, writer) in writers {
            writer.writer.close()?;
        }

        self.stats.active_partitions = 0;
        Ok(())
    }

    /// Get storage statistics
    pub fn stats(&self) -> &StorageStats {
        &self.stats
    }

    /// Get current configuration
    pub fn config(&self) -> &TimeSeriesStorageConfig {
        &self.config
    }

    /// Get partition metadata
    pub fn partition_metadata(&self) -> Vec<&PartitionMetadata> {
        self.metadata_cache.metadata.values().collect()
    }
}

impl Drop for TimeSeriesStorage {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::Rng;
    use tempfile::tempdir;
    use many_lamps_core::{TokenId, data_contracts::SignalType};

    fn create_test_market_data(timestamp_us: i64, token_id: &str) -> NormalizedMarketData {
        NormalizedMarketData {
            timestamp_us,
            token_id: TokenId(token_id.to_string()),
            mid_price: 0.55 + rand::thread_rng().rng::<f64>() * 0.01,
            spread: 0.001 + rand::thread_rng().rng::<f64>() * 0.0005,
            bid_size: 100.0 + rand::thread_rng().rng::<f64>() * 50.0,
            ask_size: 100.0 + rand::thread_rng().rng::<f64>() * 50.0,
            imbalance: rand::thread_rng().rng::<f64>() * 2.0 - 1.0,
            volatility: 0.001 + rand::thread_rng().rng::<f64>() * 0.01,
            best_bid: Some(0.55),
            best_ask: Some(0.551),
        }
    }

    fn create_test_features(timestamp_us: i64, token_id: &str) -> ExtractedFeatureVector {
        let mut features = ExtractedFeatureVector::new(timestamp_us, TokenId(token_id.to_string()));
        for i in 0..24 {
            features.features[i] = rand::thread_rng().rng();
        }
        features
    }

    fn create_test_signals(timestamp_us: i64, token_id: &str) -> SignalData {
        SignalData {
            timestamp_us,
            token_id: TokenId(token_id.to_string()),
            model_id: "test_model_v1".to_string(),
            value: rand::thread_rng().rng(),
            signal_type: SignalType::Direction,
            confidence: rand::thread_rng().rng(),
            feature_importance: None,
        }
    }

    #[test]
    fn test_partition_key_hourly() {
        let key = PartitionKey::from_timestamp_us(1_706_412_800_000_000);
        assert_eq!(key.year, 2024);
        assert_eq!(key.month, 1);
        assert_eq!(key.day, 15);
        assert_eq!(key.hour, 12);
    }

    #[test]
    fn test_partition_key_daily() {
        let key = PartitionKey::from_timestamp_us(1_706_412_800_000_000);
        assert_eq!(key.year, 2024);
        assert_eq!(key.month, 1);
        assert_eq!(key.day, 15);
    }

    #[test]
    fn test_partition_path_hourly() {
        let key = PartitionKey { year: 2024, month: 1, day: 15, hour: 12, week: 3 };
        let path = key.to_path(PartitionScheme::Hourly);
        assert_eq!(path.to_string_lossy(), "year=2024/month=01/day=15/hour=12");
    }

    #[test]
    fn test_partition_path_daily() {
        let key = PartitionKey { year: 2024, month: 1, day: 15, hour: 12, week: 3 };
        let path = key.to_path(PartitionScheme::Daily);
        assert_eq!(path.to_string_lossy(), "year=2024/month=01/day=15");
    }

    #[test]
    fn test_write_market_data() {
        let dir = tempdir().unwrap();
        let config = TimeSeriesStorageConfig::new(dir.path().to_path_buf())
            .with_partition_scheme(PartitionScheme::Hourly)
            .with_compression(CompressionCodec::Snappy);
        
        let mut storage = TimeSeriesStorage::new(config, DataTypeIdentifier::MarketData).unwrap();
        
        let base_ts = 1_706_412_800_000_000;
        for i in 0..100 {
            let data = create_test_market_data(base_ts + i as i64 * 1000, "0x123");
            storage.write_market_data(&data).unwrap();
        }
        
        storage.flush().unwrap();
        
        assert_eq!(storage.stats().rows_written, 100);
        assert!(storage.stats().files_created >= 1);
    }

    #[test]
    fn test_write_across_partitions() {
        let dir = tempdir().unwrap();
        let config = TimeSeriesStorageConfig::new(dir.path().to_path_buf())
            .with_partition_scheme(PartitionScheme::Daily)
            .with_max_rows_per_file(10);
        
        let mut storage = TimeSeriesStorage::new(config, DataTypeIdentifier::MarketData).unwrap();
        
        let day1_ts = 1_706_412_800_000_000;
        let day2_ts = day1_ts + 86400 * 1_000_000;
        
        for i in 0..15 {
            let ts = if i < 8 { day1_ts + i as i64 * 1000 } else { day2_ts + (i - 8) as i64 * 1000 };
            let data = create_test_market_data(ts, "0x123");
            storage.write_market_data(&data).unwrap();
        }
        
        storage.flush().unwrap();
        
        let partitions = storage.partition_metadata();
        assert!(partitions.len() >= 1);
    }

    #[test]
    fn test_write_features() {
        let dir = tempdir().unwrap();
        let config = TimeSeriesStorageConfig::new(dir.path().to_path_buf());
        
        let mut storage = TimeSeriesStorage::new(config, DataTypeIdentifier::Features).unwrap();
        
        let base_ts = 1_706_412_800_000_000;
        for i in 0..50 {
            let features = create_test_features(base_ts + i as i64 * 1000, "0x456");
            storage.write_features(&features).unwrap();
        }
        
        storage.flush().unwrap();
        assert_eq!(storage.stats().rows_written, 50);
    }

    #[test]
    fn test_write_signals() {
        let dir = tempdir().unwrap();
        let config = TimeSeriesStorageConfig::new(dir.path().to_path_buf());
        
        let mut storage = TimeSeriesStorage::new(config, DataTypeIdentifier::Signals).unwrap();
        
        let base_ts = 1_706_412_800_000_000;
        for i in 0..30 {
            let signal = create_test_signals(base_ts + i as i64 * 1000, "0x789");
            storage.write_signals(&signal).unwrap();
        }
        
        storage.flush().unwrap();
        assert_eq!(storage.stats().rows_written, 30);
    }

    #[test]
    fn test_multiple_tokens() {
        let dir = tempdir().unwrap();
        let config = TimeSeriesStorageConfig::new(dir.path().to_path_buf());
        
        let mut storage = TimeSeriesStorage::new(config, DataTypeIdentifier::MarketData).unwrap();
        
        let base_ts = 1_706_412_800_000_000;
        
        for token in &["0x111", "0x222", "0x333"] {
            for i in 0..10 {
                let data = create_test_market_data(base_ts + i as i64 * 1000, token);
                storage.write_market_data(&data).unwrap();
            }
        }
        
        storage.close().unwrap();
        
        assert_eq!(storage.stats().rows_written, 30);
    }

    #[test]
    fn test_batch_writes() {
        let dir = tempdir().unwrap();
        let config = TimeSeriesStorageConfig::new(dir.path().to_path_buf());
        
        let mut storage = TimeSeriesStorage::new(config, DataTypeIdentifier::MarketData).unwrap();
        
        let base_ts = 1_706_412_800_000_000;
        
        let batch: Vec<NormalizedMarketData> = (0..100)
            .map(|i| create_test_market_data(base_ts + i as i64 * 1000, "0xbatch"))
            .collect();
        
        storage.write_market_data_batch(&batch).unwrap();
        storage.close().unwrap();
        
        assert_eq!(storage.stats().rows_written, 100);
    }

    #[test]
    fn test_schema_versioning() {
        let dir = tempdir().unwrap();
        let config = TimeSeriesStorageConfig::new(dir.path().to_path_buf());
        
        let storage = TimeSeriesStorage::new(config, DataTypeIdentifier::MarketData).unwrap();
        
        assert_eq!(storage.schema_version.major, 1);
        assert_eq!(storage.schema_version.minor, 0);
        
        let fields = storage.schema.fields();
        assert!(fields.iter().any(|f| f.name() == "timestamp_us"));
        assert!(fields.iter().any(|f| f.name() == "mid_price"));
        assert!(fields.iter().any(|f| f.name() == "spread"));
    }

    #[test]
    fn test_compression_options() {
        let dir = tempdir().unwrap();
        
        for codec in [CompressionCodec::None, CompressionCodec::Snappy, CompressionCodec::Gzip] {
            let config = TimeSeriesStorageConfig::new(dir.path().join(format!("{:?}", codec)).to_path_buf())
                .with_compression(codec);
            let mut storage = TimeSeriesStorage::new(config, DataTypeIdentifier::MarketData).unwrap();
            
            let base_ts = 1_706_412_800_000_000;
            for i in 0..100 {
                let data = create_test_market_data(base_ts + i as i64 * 1000, "0xtest");
                storage.write_market_data(&data).unwrap();
            }
            
            storage.close().unwrap();
        }
    }

    #[test]
    fn test_partition_scheme_options() {
        let dir = tempdir().unwrap();
        let base_ts = 1_706_412_800_000_000;
        
        for scheme in [PartitionScheme::Hourly, PartitionScheme::Daily, PartitionScheme::Weekly, PartitionScheme::Monthly] {
            let config = TimeSeriesStorageConfig::new(dir.path().join(format!("{:?}", scheme)).to_path_buf())
                .with_partition_scheme(scheme);
            let mut storage = TimeSeriesStorage::new(config, DataTypeIdentifier::MarketData).unwrap();
            
            let data = create_test_market_data(base_ts, "0xtest");
            storage.write_market_data(&data).unwrap();
            storage.close().unwrap();
        }
    }
}
