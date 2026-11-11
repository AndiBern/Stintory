// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Telemetry channels: one named, typed column of samples with a unit.
//!
//! Game adapters map game data onto the canonical channels in [`names`]
//! (SI units, see docs/PLAN.md §4.2). Game-specific extras use a namespaced
//! name such as `iracing.LFshockDefl` or `f1.ers_store`.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Canonical channel names shared by all games.
pub mod names {
    /// Time since lap start, seconds.
    pub const TIME: &str = "t";
    /// Distance from the start/finish line, metres.
    pub const DIST: &str = "dist";
    /// Speed, m/s.
    pub const SPEED: &str = "speed";
    /// Throttle pedal, 0–1.
    pub const THROTTLE: &str = "throttle";
    /// Brake pedal, 0–1.
    pub const BRAKE: &str = "brake";
    /// Clutch pedal, 0–1 (1 = fully pressed).
    pub const CLUTCH: &str = "clutch";
    /// Steering wheel angle, radians, positive = right.
    pub const STEER: &str = "steer";
    /// Steering normalised to -1 (full left) … 1 (full right), when the angle is unknown.
    pub const STEER_NORM: &str = "steer_norm";
    /// Gear: -1 reverse, 0 neutral, 1..n.
    pub const GEAR: &str = "gear";
    /// Engine speed, rpm.
    pub const RPM: &str = "rpm";
    /// World position X, metres.
    pub const POS_X: &str = "pos_x";
    /// World position Y, metres.
    pub const POS_Y: &str = "pos_y";
    /// World position Z, metres.
    pub const POS_Z: &str = "pos_z";
    /// Lateral acceleration, m/s².
    pub const G_LAT: &str = "g_lat";
    /// Longitudinal acceleration, m/s².
    pub const G_LONG: &str = "g_long";
    /// Vertical acceleration, m/s².
    pub const G_VERT: &str = "g_vert";
    /// Yaw angle, radians.
    pub const YAW: &str = "yaw";
    /// Yaw rate, rad/s.
    pub const YAW_RATE: &str = "yaw_rate";
    /// Fuel in tank (litres, or kg where the game only reports mass).
    pub const FUEL: &str = "fuel";
    /// Brake bias, fraction to the front 0–1.
    pub const BRAKE_BIAS: &str = "brake_bias";
    /// ABS currently intervening.
    pub const ABS_ACTIVE: &str = "abs_active";
    /// Traction control currently intervening.
    pub const TC_ACTIVE: &str = "tc_active";
}

/// Physical unit of a channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    /// Dimensionless / enumeration / flag
    None,
    /// Fraction 0–1
    Ratio,
    /// Seconds
    Second,
    /// Metres
    Meter,
    /// Metres per second
    MeterPerSecond,
    /// Metres per second squared
    MeterPerSecondSquared,
    /// Radians
    Radian,
    /// Radians per second
    RadianPerSecond,
    /// Revolutions per minute
    Rpm,
    /// Degrees Celsius
    Celsius,
    /// Kilopascal
    Kilopascal,
    /// Litres
    Liter,
    /// Kilograms
    Kilogram,
    /// Newtons
    Newton,
    /// Newton metres
    NewtonMeter,
    /// Watts
    Watt,
    /// Joules
    Joule,
}

impl Unit {
    const ALL: [Unit; 17] = [
        Unit::None,
        Unit::Ratio,
        Unit::Second,
        Unit::Meter,
        Unit::MeterPerSecond,
        Unit::MeterPerSecondSquared,
        Unit::Radian,
        Unit::RadianPerSecond,
        Unit::Rpm,
        Unit::Celsius,
        Unit::Kilopascal,
        Unit::Liter,
        Unit::Kilogram,
        Unit::Newton,
        Unit::NewtonMeter,
        Unit::Watt,
        Unit::Joule,
    ];

    /// Stable symbol used in files.
    pub const fn as_str(self) -> &'static str {
        match self {
            Unit::None => "",
            Unit::Ratio => "ratio",
            Unit::Second => "s",
            Unit::Meter => "m",
            Unit::MeterPerSecond => "m/s",
            Unit::MeterPerSecondSquared => "m/s2",
            Unit::Radian => "rad",
            Unit::RadianPerSecond => "rad/s",
            Unit::Rpm => "rpm",
            Unit::Celsius => "C",
            Unit::Kilopascal => "kPa",
            Unit::Liter => "l",
            Unit::Kilogram => "kg",
            Unit::Newton => "N",
            Unit::NewtonMeter => "N*m",
            Unit::Watt => "W",
            Unit::Joule => "J",
        }
    }
}

impl fmt::Display for Unit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error for unknown unit symbols.
#[derive(Debug, thiserror::Error)]
#[error("unknown unit: {0:?}")]
pub struct UnknownUnit(String);

impl FromStr for Unit {
    type Err = UnknownUnit;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Unit::ALL
            .into_iter()
            .find(|u| u.as_str() == s)
            .ok_or_else(|| UnknownUnit(s.to_owned()))
    }
}

/// The samples of one channel.
#[derive(Debug, Clone, PartialEq)]
pub enum ChannelData {
    /// 32-bit floats (most channels)
    F32(Vec<f32>),
    /// 64-bit floats (where precision matters)
    F64(Vec<f64>),
    /// Integers (gear, enumerations)
    I32(Vec<i32>),
    /// Flags
    Bool(Vec<bool>),
}

impl ChannelData {
    /// Number of samples.
    pub fn len(&self) -> usize {
        match self {
            ChannelData::F32(v) => v.len(),
            ChannelData::F64(v) => v.len(),
            ChannelData::I32(v) => v.len(),
            ChannelData::Bool(v) => v.len(),
        }
    }

    /// True if there are no samples.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// One named telemetry channel.
#[derive(Debug, Clone, PartialEq)]
pub struct Channel {
    /// Canonical name (see [`names`]) or namespaced game-specific name.
    pub name: String,
    /// Unit of all samples.
    pub unit: Unit,
    /// The samples, one per tick.
    pub data: ChannelData,
}

impl Channel {
    /// Creates a channel.
    pub fn new(name: impl Into<String>, unit: Unit, data: ChannelData) -> Self {
        Self {
            name: name.into(),
            unit,
            data,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_symbols_roundtrip() {
        for unit in Unit::ALL {
            assert_eq!(unit.as_str().parse::<Unit>().unwrap(), unit);
        }
    }
}
