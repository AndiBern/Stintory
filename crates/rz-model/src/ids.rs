// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Identifiers. Sessions and laps get UUIDv7 ids generated on the client, so
//! data can be synced without id conflicts. Telemetry blobs are content
//! addressed by their SHA-256 hash.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

macro_rules! uuid_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            /// Creates a new, time-ordered (UUIDv7) id.
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }

            /// Wraps an existing UUID.
            pub const fn from_uuid(uuid: Uuid) -> Self {
                Self(uuid)
            }

            /// The underlying UUID.
            pub const fn as_uuid(&self) -> &Uuid {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(s).map(Self)
            }
        }
    };
}

uuid_id!(
    /// Id of a [`Session`](crate::Session).
    SessionId
);
uuid_id!(
    /// Id of a [`Lap`](crate::Lap).
    LapId
);

/// SHA-256 hash of a telemetry blob (lowercase hex). Identifies the blob's
/// content, so identical files are stored and synced only once.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct BlobHash(String);

impl BlobHash {
    /// Hashes the given bytes.
    pub fn of(bytes: &[u8]) -> Self {
        Self(hex::encode(Sha256::digest(bytes)))
    }

    /// The hash as 64 lowercase hex characters.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Error for strings that are not a valid [`BlobHash`].
#[derive(Debug, thiserror::Error)]
#[error("invalid blob hash: expected 64 lowercase hex characters")]
pub struct InvalidBlobHash;

impl FromStr for BlobHash {
    type Err = InvalidBlobHash;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let valid = s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        if valid {
            Ok(Self(s.to_owned()))
        } else {
            Err(InvalidBlobHash)
        }
    }
}

impl TryFrom<String> for BlobHash {
    type Error = InvalidBlobHash;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl From<BlobHash> for String {
    fn from(h: BlobHash) -> Self {
        h.0
    }
}

impl fmt::Display for BlobHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_time_ordered() {
        let a = LapId::new();
        let b = LapId::new();
        assert_ne!(a, b);
        assert!(a < b, "UUIDv7 ids created later must sort later");
    }

    #[test]
    fn id_roundtrips_through_string() {
        let id = SessionId::new();
        let parsed: SessionId = id.to_string().parse().unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn blob_hash_of_known_input() {
        // SHA-256 of the empty input
        assert_eq!(
            BlobHash::of(b"").as_str(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn blob_hash_rejects_invalid_strings() {
        assert!("abc".parse::<BlobHash>().is_err());
        assert!(
            "E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855"
                .parse::<BlobHash>()
                .is_err()
        );
        assert!(BlobHash::of(b"x").as_str().parse::<BlobHash>().is_ok());
    }
}
