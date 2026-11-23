//! Jiff timezone behavior used as the migration prototype for #15.
//!
//! The production model still uses Chrono while the workspace migration is
//! staged. Keeping this adapter small gives us a focused seam for comparing
//! calendar and DST behavior before changing public data types.

use jiff::{Timestamp, civil::Date, tz::TimeZone};

#[derive(Clone, Debug, Eq, PartialEq)]
struct JiffZone(TimeZone);

impl JiffZone {
    fn parse(name: &str) -> Result<Self, jiff::Error> {
        TimeZone::get(name.trim()).map(Self)
    }

    fn utc() -> Self {
        Self(TimeZone::UTC)
    }

    fn name(&self) -> &str {
        self.0.iana_name().unwrap_or("UTC")
    }

    fn local_date(&self, instant: Timestamp) -> Date {
        instant.to_zoned_datetime(self.0.clone()).date()
    }

    fn start_of_day(&self, date: Date) -> Result<Timestamp, jiff::Error> {
        // Jiff's default compatible disambiguation advances through a gap and
        // selects the earlier occurrence in a fold, matching the existing
        // start_of_day contract.
        date.to_zoned_datetime(self.0.clone())
            .map(|zdt| zdt.timestamp())
    }

    fn format(&self, instant: Timestamp, format: &str) -> String {
        instant
            .to_zoned_datetime(self.0.clone())
            .strftime(format)
            .to_string()
    }

    fn rfc3339(&self, instant: Timestamp) -> String {
        let zdt = instant.to_zoned_datetime(self.0.clone());
        zdt.strftime("%Y-%m-%dT%H:%M:%S%:z").to_string()
    }
}

#[test]
fn jiff_preserves_zone_and_dst_boundaries() {
    let tokyo = JiffZone::parse("Asia/Tokyo").unwrap();
    let instant: Timestamp = "2026-11-20T15:00:00Z".parse().unwrap();
    assert_eq!(tokyo.name(), "Asia/Tokyo");
    assert_eq!(tokyo.local_date(instant).to_string(), "2026-11-21");
    assert_eq!(tokyo.rfc3339(instant), "2026-11-21T00:00:00+09:00");

    let new_york = JiffZone::parse("America/New_York").unwrap();
    let spring: Date = "2026-03-08".parse().unwrap();
    let fall: Date = "2026-11-01".parse().unwrap();
    assert_eq!(
        new_york
            .start_of_day(spring)
            .unwrap()
            .to_zoned_datetime(new_york.0.clone())
            .strftime("%Y-%m-%dT%H:%M:%S%:z")
            .to_string(),
        "2026-03-08T00:00:00-05:00"
    );
    assert_eq!(
        new_york
            .start_of_day(fall)
            .unwrap()
            .to_zoned_datetime(new_york.0.clone())
            .strftime("%Y-%m-%dT%H:%M:%S%:z")
            .to_string(),
        "2026-11-01T00:00:00-04:00"
    );
}

#[test]
fn jiff_handles_a_midnight_dst_gap() {
    let santiago = JiffZone::parse("America/Santiago").unwrap();
    let date: Date = "2026-09-06".parse().unwrap();
    let start = santiago.start_of_day(date).unwrap();
    assert_eq!(santiago.local_date(start).to_string(), "2026-09-06");
    assert_eq!(
        start
            .checked_sub(jiff::SignedDuration::from_secs(1))
            .unwrap()
            .to_zoned_datetime(santiago.0)
            .date()
            .to_string(),
        "2026-09-05"
    );
}

#[test]
fn jiff_formats_with_strftime_compatible_patterns() {
    let utc = JiffZone::utc();
    let instant: Timestamp = "2026-01-02T03:04:05Z".parse().unwrap();
    assert_eq!(utc.format(instant, "%Y-%m-%d"), "2026-01-02");
}
