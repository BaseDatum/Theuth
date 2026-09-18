use crate::Timestamp;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;
use serde::de::Visitor;

impl TryFrom<Timestamp> for DateTime<Utc> {
    type Error = &'static str;
    fn try_from(value: Timestamp) -> Result<Self, Self::Error> {
        let Timestamp { seconds, nanos } = value;

        Self::from_timestamp(
            seconds,
            nanos
                .try_into()
                .map_err(|_| "out of range integral type conversion attempted")?,
        )
        .ok_or("invalid or out-of-range datetime")
    }
}

impl From<DateTime<Utc>> for Timestamp {
    fn from(value: DateTime<Utc>) -> Self {
        Self {
            seconds: value.timestamp(),
            nanos: value.timestamp_subsec_nanos() as i32,
        }
    }
}

impl Serialize for Timestamp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // Protobuf JSON mapping: Timestamp output is always Z-normalized RFC 3339 with
        // 0, 3, 6 or 9 fractional digits. `to_rfc3339()` emits `+00:00`, which is valid
        // RFC 3339 but not the canonical form conformance kits check for.
        let t: DateTime<Utc> = (*self).try_into().map_err(serde::ser::Error::custom)?;
        serializer.serialize_str(t.to_rfc3339_opts(SecondsFormat::AutoSi, true).as_str())
    }
}

struct TimestampVisitor;

impl<'de> Visitor<'de> for TimestampVisitor {
    type Value = Timestamp;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a date string")
    }

    fn visit_str<E>(self, s: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        let d = DateTime::parse_from_rfc3339(s).map_err(serde::de::Error::custom)?;
        let d: DateTime<Utc> = d.into();
        Ok(d.into())
    }
}

impl<'de> serde::Deserialize<'de> for Timestamp {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_str(TimestampVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, TimeZone};
    use serde::Deserialize;
    use serde::de::value::{BorrowedStrDeserializer, Error};

    #[test]
    fn test_date() {
        let datetime = FixedOffset::east_opt(5 * 3600)
            .expect("time zone offset should be valid")
            .with_ymd_and_hms(2016, 11, 8, 21, 7, 9)
            .unwrap();
        let encoded = datetime.to_rfc3339();
        assert_eq!(&encoded, "2016-11-08T21:07:09+05:00");

        let utc: DateTime<Utc> = datetime.into();

        // Offsets are accepted on input...
        let deserializer = BorrowedStrDeserializer::<'_, Error>::new(&encoded);
        let a: Timestamp = Timestamp::deserialize(deserializer).unwrap();
        assert_eq!(a.seconds, utc.timestamp());
        assert_eq!(a.nanos, utc.timestamp_subsec_nanos() as i32);

        // ...but output is Z-normalized, never `+00:00`.
        let encoded = serde_json::to_string(&a).unwrap();
        assert_eq!(encoded, "\"2016-11-08T16:07:09Z\"");
    }

    #[test]
    fn test_fractional_digits() {
        let json = |seconds: i64, nanos: i32| serde_json::to_string(&Timestamp { seconds, nanos }).unwrap();
        // 0, 3, 6 or 9 fractional digits, as the protobuf JSON mapping requires.
        assert_eq!(json(1_478_621_229, 0), "\"2016-11-08T16:07:09Z\"");
        assert_eq!(json(1_478_621_229, 123_000_000), "\"2016-11-08T16:07:09.123Z\"");
        assert_eq!(json(1_478_621_229, 123_456_000), "\"2016-11-08T16:07:09.123456Z\"");
        assert_eq!(json(1_478_621_229, 123_456_789), "\"2016-11-08T16:07:09.123456789Z\"");
        // Pre-epoch: negative seconds with positive nanos (the protobuf normal form).
        assert_eq!(json(-1, 500_000_000), "\"1969-12-31T23:59:59.500Z\"");

        // Round trip preserves the exact instant.
        let round = |s: &str| -> Timestamp {
            Timestamp::deserialize(BorrowedStrDeserializer::<'_, Error>::new(s)).unwrap()
        };
        assert_eq!(round("2016-11-08T16:07:09.123456789Z"), Timestamp { seconds: 1_478_621_229, nanos: 123_456_789 });
        assert_eq!(round("1969-12-31T23:59:59.500Z"), Timestamp { seconds: -1, nanos: 500_000_000 });
    }
}
