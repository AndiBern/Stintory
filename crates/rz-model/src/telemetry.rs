// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Lap telemetry and its file format.
//!
//! A lap's telemetry is stored as **one Parquet file**: one column per
//! channel, one row per sample at the game's native rate, zstd compressed.
//! The file can be opened with any Parquet tool (pandas, DuckDB, …).
//!
//! File metadata (Arrow schema metadata):
//! - `rz.format_version`: format version, currently [`FORMAT_VERSION`]
//! - `rz.sample_rate_hz`: nominal sample rate
//!
//! Column metadata:
//! - `rz.unit`: unit symbol (see [`Unit::as_str`])

#[cfg(feature = "parquet")]
use std::collections::HashMap;
#[cfg(feature = "parquet")]
use std::sync::Arc;

#[cfg(feature = "parquet")]
use arrow_array::{
    Array, ArrayRef, BooleanArray, Float32Array, Float64Array, Int32Array, RecordBatch,
};
#[cfg(feature = "parquet")]
use arrow_schema::{ArrowError, DataType, Field, Schema};
#[cfg(feature = "parquet")]
use bytes::Bytes;
#[cfg(feature = "parquet")]
use parquet::arrow::ArrowWriter;
#[cfg(feature = "parquet")]
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
#[cfg(feature = "parquet")]
use parquet::basic::{Compression, ZstdLevel};
#[cfg(feature = "parquet")]
use parquet::errors::ParquetError;
#[cfg(feature = "parquet")]
use parquet::file::properties::WriterProperties;

use crate::channel::Channel;
#[cfg(feature = "parquet")]
use crate::channel::{ChannelData, Unit};
#[cfg(feature = "parquet")]
use crate::ids::BlobHash;

/// Current major version of the lap telemetry file format.
pub const FORMAT_VERSION: u32 = 1;

#[cfg(feature = "parquet")]
const META_FORMAT_VERSION: &str = "rz.format_version";
#[cfg(feature = "parquet")]
const META_SAMPLE_RATE: &str = "rz.sample_rate_hz";
#[cfg(feature = "parquet")]
const META_UNIT: &str = "rz.unit";
#[cfg(feature = "parquet")]
const ZSTD_LEVEL: i32 = 3;

/// Errors when building, writing or reading lap telemetry.
#[derive(Debug, thiserror::Error)]
pub enum TelemetryError {
    /// A channel with this name already exists.
    #[error("duplicate channel: {0}")]
    DuplicateChannel(String),
    /// A channel's sample count differs from the other channels.
    #[error("channel {name} has {actual} samples, expected {expected}")]
    LengthMismatch {
        /// Channel name
        name: String,
        /// Sample count of the other channels
        expected: usize,
        /// Sample count of this channel
        actual: usize,
    },
    /// The file is not valid Rundenzeit lap telemetry.
    #[error("invalid telemetry file: {0}")]
    InvalidFile(String),
    /// The file was written by a newer, incompatible format version.
    #[error("unsupported telemetry format version {0} (supported: {FORMAT_VERSION})")]
    UnsupportedVersion(u32),
    /// Parquet encoding/decoding failed.
    #[cfg(feature = "parquet")]
    #[error(transparent)]
    Parquet(#[from] ParquetError),
    /// Arrow conversion failed.
    #[cfg(feature = "parquet")]
    #[error(transparent)]
    Arrow(#[from] ArrowError),
}

/// All telemetry channels of one lap. All channels have the same number of
/// samples; sample `i` of every channel belongs to the same tick.
#[derive(Debug, Clone, PartialEq)]
pub struct LapTelemetry {
    sample_rate_hz: f32,
    channels: Vec<Channel>,
}

impl LapTelemetry {
    /// Creates empty telemetry with the given nominal sample rate.
    pub fn new(sample_rate_hz: f32) -> Self {
        Self {
            sample_rate_hz,
            channels: Vec::new(),
        }
    }

    /// Nominal sample rate in Hz.
    pub fn sample_rate_hz(&self) -> f32 {
        self.sample_rate_hz
    }

    /// Number of samples per channel (0 if there are no channels).
    pub fn len(&self) -> usize {
        self.channels.first().map_or(0, |c| c.data.len())
    }

    /// True if there are no samples.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// All channels in insertion order.
    pub fn channels(&self) -> &[Channel] {
        &self.channels
    }

    /// Looks up a channel by name.
    pub fn channel(&self, name: &str) -> Option<&Channel> {
        self.channels.iter().find(|c| c.name == name)
    }

    /// Adds a channel. Fails if the name exists or the sample count differs
    /// from the existing channels.
    pub fn add_channel(&mut self, channel: Channel) -> Result<(), TelemetryError> {
        if self.channel(&channel.name).is_some() {
            return Err(TelemetryError::DuplicateChannel(channel.name));
        }
        if let Some(first) = self.channels.first() {
            let expected = first.data.len();
            let actual = channel.data.len();
            if expected != actual {
                return Err(TelemetryError::LengthMismatch {
                    name: channel.name,
                    expected,
                    actual,
                });
            }
        }
        self.channels.push(channel);
        Ok(())
    }

    /// Builder-style [`add_channel`](Self::add_channel).
    pub fn with_channel(mut self, channel: Channel) -> Result<Self, TelemetryError> {
        self.add_channel(channel)?;
        Ok(self)
    }

    /// Encodes the telemetry as a Parquet file.
    #[cfg(feature = "parquet")]
    pub fn to_parquet(&self) -> Result<Vec<u8>, TelemetryError> {
        let fields: Vec<Field> = self
            .channels
            .iter()
            .map(|c| {
                Field::new(&c.name, data_type(&c.data), false).with_metadata(HashMap::from([(
                    META_UNIT.to_owned(),
                    c.unit.as_str().to_owned(),
                )]))
            })
            .collect();
        let schema = Arc::new(Schema::new_with_metadata(
            fields,
            HashMap::from([
                (META_FORMAT_VERSION.to_owned(), FORMAT_VERSION.to_string()),
                (META_SAMPLE_RATE.to_owned(), self.sample_rate_hz.to_string()),
            ]),
        ));
        let columns: Vec<ArrayRef> = self.channels.iter().map(|c| to_array(&c.data)).collect();

        let props = WriterProperties::builder()
            .set_compression(Compression::ZSTD(ZstdLevel::try_new(ZSTD_LEVEL)?))
            .build();
        let mut out = Vec::new();
        let mut writer = ArrowWriter::try_new(&mut out, schema.clone(), Some(props))?;
        if !self.channels.is_empty() {
            writer.write(&RecordBatch::try_new(schema, columns)?)?;
        }
        writer.close()?;
        Ok(out)
    }

    /// Encodes the telemetry and returns the bytes with their content hash.
    #[cfg(feature = "parquet")]
    pub fn to_blob(&self) -> Result<(Vec<u8>, BlobHash), TelemetryError> {
        let bytes = self.to_parquet()?;
        let hash = BlobHash::of(&bytes);
        Ok((bytes, hash))
    }

    /// Decodes a Parquet file written by [`to_parquet`](Self::to_parquet).
    #[cfg(feature = "parquet")]
    pub fn from_parquet(bytes: impl Into<Bytes>) -> Result<Self, TelemetryError> {
        let builder = ParquetRecordBatchReaderBuilder::try_new(bytes.into())?;
        let schema = builder.schema().clone();
        let meta = schema.metadata();

        let version: u32 = meta
            .get(META_FORMAT_VERSION)
            .ok_or_else(|| invalid(format!("missing {META_FORMAT_VERSION}")))?
            .parse()
            .map_err(|_| invalid(format!("bad {META_FORMAT_VERSION}")))?;
        if version != FORMAT_VERSION {
            return Err(TelemetryError::UnsupportedVersion(version));
        }
        let sample_rate_hz: f32 = meta
            .get(META_SAMPLE_RATE)
            .ok_or_else(|| invalid(format!("missing {META_SAMPLE_RATE}")))?
            .parse()
            .map_err(|_| invalid(format!("bad {META_SAMPLE_RATE}")))?;

        let mut data: Vec<ChannelData> = schema
            .fields()
            .iter()
            .map(|f| empty_data(f.data_type()).ok_or_else(|| unsupported_type(f)))
            .collect::<Result<_, _>>()?;
        for batch in builder.build()? {
            let batch = batch?;
            for (target, column) in data.iter_mut().zip(batch.columns()) {
                append(target, column.as_ref())?;
            }
        }

        let mut telemetry = LapTelemetry::new(sample_rate_hz);
        for (field, data) in schema.fields().iter().zip(data) {
            let unit = field
                .metadata()
                .get(META_UNIT)
                .map_or(Ok(Unit::None), |u| u.parse())
                .map_err(|e| invalid(format!("column {}: {e}", field.name())))?;
            telemetry.add_channel(Channel::new(field.name().clone(), unit, data))?;
        }
        Ok(telemetry)
    }
}

#[cfg(feature = "parquet")]
fn invalid(msg: String) -> TelemetryError {
    TelemetryError::InvalidFile(msg)
}

#[cfg(feature = "parquet")]
fn unsupported_type(field: &Field) -> TelemetryError {
    invalid(format!(
        "column {} has unsupported type {}",
        field.name(),
        field.data_type()
    ))
}

#[cfg(feature = "parquet")]
fn data_type(data: &ChannelData) -> DataType {
    match data {
        ChannelData::F32(_) => DataType::Float32,
        ChannelData::F64(_) => DataType::Float64,
        ChannelData::I32(_) => DataType::Int32,
        ChannelData::Bool(_) => DataType::Boolean,
    }
}

#[cfg(feature = "parquet")]
fn to_array(data: &ChannelData) -> ArrayRef {
    match data {
        ChannelData::F32(v) => Arc::new(Float32Array::from(v.clone())),
        ChannelData::F64(v) => Arc::new(Float64Array::from(v.clone())),
        ChannelData::I32(v) => Arc::new(Int32Array::from(v.clone())),
        ChannelData::Bool(v) => Arc::new(BooleanArray::from(v.clone())),
    }
}

#[cfg(feature = "parquet")]
fn empty_data(data_type: &DataType) -> Option<ChannelData> {
    match data_type {
        DataType::Float32 => Some(ChannelData::F32(Vec::new())),
        DataType::Float64 => Some(ChannelData::F64(Vec::new())),
        DataType::Int32 => Some(ChannelData::I32(Vec::new())),
        DataType::Boolean => Some(ChannelData::Bool(Vec::new())),
        _ => None,
    }
}

#[cfg(feature = "parquet")]
fn append(target: &mut ChannelData, column: &dyn Array) -> Result<(), TelemetryError> {
    if column.null_count() > 0 {
        return Err(invalid("null values are not allowed".to_owned()));
    }
    let any = column.as_any();
    let mismatch = || invalid("column type changed between row groups".to_owned());
    match target {
        ChannelData::F32(v) => v.extend_from_slice(
            any.downcast_ref::<Float32Array>()
                .ok_or_else(mismatch)?
                .values(),
        ),
        ChannelData::F64(v) => v.extend_from_slice(
            any.downcast_ref::<Float64Array>()
                .ok_or_else(mismatch)?
                .values(),
        ),
        ChannelData::I32(v) => v.extend_from_slice(
            any.downcast_ref::<Int32Array>()
                .ok_or_else(mismatch)?
                .values(),
        ),
        ChannelData::Bool(v) => {
            let arr = any.downcast_ref::<BooleanArray>().ok_or_else(mismatch)?;
            v.extend((0..arr.len()).map(|i| arr.value(i)));
        }
    }
    Ok(())
}

#[cfg(all(test, feature = "parquet"))]
mod tests {
    use super::*;
    use crate::channel::names;

    fn sample_lap(samples: usize) -> LapTelemetry {
        let t: Vec<f64> = (0..samples).map(|i| i as f64 / 60.0).collect();
        let speed: Vec<f32> = (0..samples)
            .map(|i| 50.0 + (i % 100) as f32 * 0.1)
            .collect();
        let gear: Vec<i32> = (0..samples).map(|i| 3 + (i / 1000) as i32 % 3).collect();
        let abs: Vec<bool> = (0..samples).map(|i| i % 97 == 0).collect();
        LapTelemetry::new(60.0)
            .with_channel(Channel::new(names::TIME, Unit::Second, ChannelData::F64(t)))
            .unwrap()
            .with_channel(Channel::new(
                names::SPEED,
                Unit::MeterPerSecond,
                ChannelData::F32(speed),
            ))
            .unwrap()
            .with_channel(Channel::new(
                names::GEAR,
                Unit::None,
                ChannelData::I32(gear),
            ))
            .unwrap()
            .with_channel(Channel::new(
                names::ABS_ACTIVE,
                Unit::None,
                ChannelData::Bool(abs),
            ))
            .unwrap()
    }

    #[test]
    fn parquet_roundtrip_preserves_everything() {
        // 90 s lap at 60 Hz
        let lap = sample_lap(5400);
        let bytes = lap.to_parquet().unwrap();
        let back = LapTelemetry::from_parquet(bytes).unwrap();
        assert_eq!(back, lap);
        assert_eq!(back.sample_rate_hz(), 60.0);
        assert_eq!(
            back.channel(names::SPEED).unwrap().unit,
            Unit::MeterPerSecond
        );
    }

    #[test]
    fn empty_telemetry_roundtrips() {
        let lap = LapTelemetry::new(100.0);
        let back = LapTelemetry::from_parquet(lap.to_parquet().unwrap()).unwrap();
        assert!(back.is_empty());
        assert_eq!(back.sample_rate_hz(), 100.0);
    }

    #[test]
    fn encoding_is_deterministic() {
        let lap = sample_lap(1000);
        let (_, a) = lap.to_blob().unwrap();
        let (_, b) = lap.to_blob().unwrap();
        assert_eq!(a, b, "same telemetry must produce the same blob hash");
    }

    #[test]
    fn compression_is_effective() {
        let lap = sample_lap(5400);
        let bytes = lap.to_parquet().unwrap();
        // raw: 5400 * (8 + 4 + 4 + 1) bytes = ~92 KB
        assert!(bytes.len() < 40_000, "file is {} bytes", bytes.len());
    }

    #[test]
    fn rejects_duplicate_and_mismatched_channels() {
        let mut lap = sample_lap(10);
        let dup = Channel::new(
            names::SPEED,
            Unit::MeterPerSecond,
            ChannelData::F32(vec![0.0; 10]),
        );
        assert!(matches!(
            lap.add_channel(dup),
            Err(TelemetryError::DuplicateChannel(_))
        ));
        let short = Channel::new(names::RPM, Unit::Rpm, ChannelData::F32(vec![0.0; 9]));
        assert!(matches!(
            lap.add_channel(short),
            Err(TelemetryError::LengthMismatch {
                expected: 10,
                actual: 9,
                ..
            })
        ));
    }

    #[test]
    fn rejects_foreign_parquet_files() {
        // A Parquet file without Rundenzeit metadata
        let schema = Arc::new(Schema::new(vec![Field::new("x", DataType::Int32, false)]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(Int32Array::from(vec![1, 2, 3])) as ArrayRef],
        )
        .unwrap();
        let mut out = Vec::new();
        let mut w = ArrowWriter::try_new(&mut out, schema, None).unwrap();
        w.write(&batch).unwrap();
        w.close().unwrap();
        assert!(matches!(
            LapTelemetry::from_parquet(out),
            Err(TelemetryError::InvalidFile(_))
        ));
    }
}
