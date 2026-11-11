// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Timestamp encoding for the database: fixed-width UTC with milliseconds,
//! e.g. `2026-11-10T18:00:00.000Z`. Fixed width makes text order equal
//! chronological order.

use time::format_description::FormatItem;
use time::macros::format_description;
use time::{OffsetDateTime, PrimitiveDateTime, UtcOffset};

use crate::StoreError;

const FORMAT: &[FormatItem<'static>] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z");

pub(crate) fn encode(t: OffsetDateTime) -> String {
    t.to_offset(UtcOffset::UTC)
        .format(FORMAT)
        .expect("formatting a valid timestamp with a static format cannot fail")
}

pub(crate) fn decode(column: &'static str, s: &str) -> Result<OffsetDateTime, StoreError> {
    PrimitiveDateTime::parse(s, FORMAT)
        .map(PrimitiveDateTime::assume_utc)
        .map_err(|e| StoreError::Corrupt {
            column,
            message: format!("{s:?}: {e}"),
        })
}

pub(crate) fn now() -> String {
    encode(OffsetDateTime::now_utc())
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn encodes_fixed_width_utc() {
        assert_eq!(
            encode(datetime!(2026-11-10 19:00:00.5 +01:00)),
            "2026-11-10T18:00:00.500Z"
        );
        assert_eq!(
            encode(datetime!(2026-11-10 18:00 UTC)),
            "2026-11-10T18:00:00.000Z"
        );
    }

    #[test]
    fn roundtrip_truncates_to_millis() {
        let t = datetime!(2026-11-10 18:00:00.123456 UTC);
        assert_eq!(
            decode("t", &encode(t)).unwrap(),
            datetime!(2026-11-10 18:00:00.123 UTC)
        );
    }

    #[test]
    fn text_order_is_chronological() {
        let a = encode(datetime!(2026-11-10 18:00:00 UTC));
        let b = encode(datetime!(2026-11-10 18:00:00.5 UTC));
        assert!(a < b);
    }
}
