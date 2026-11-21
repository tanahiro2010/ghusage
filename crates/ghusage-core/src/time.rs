//! Periods, time zones and calendar buckets.
//!
//! Timestamps are UTC internally. Every period is the half-open interval
//! `[start, end)`. Calendar boundaries (days, Monday-start weeks, calendar
//! months) are computed in the selected time zone.

use chrono::{
    DateTime, Datelike, Duration, LocalResult, Months, NaiveDate, NaiveTime, TimeZone, Utc,
};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Time zone used for calendar boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Zone(pub Tz);

impl Zone {
    pub fn utc() -> Self {
        Zone(Tz::UTC)
    }

    /// Parses an IANA zone name such as `Asia/Tokyo` or `UTC`.
    pub fn parse(name: &str) -> Result<Self, TimeError> {
        Tz::from_str(name.trim())
            .map(Zone)
            .map_err(|_| TimeError::UnknownZone(name.to_string()))
    }

    /// Resolves the zone: explicit value, then `TZ`, then the OS setting,
    /// then UTC.
    pub fn resolve(explicit: Option<&str>) -> Result<Self, TimeError> {
        if let Some(name) = explicit {
            return Self::parse(name);
        }
        if let Ok(tz) = std::env::var("TZ") {
            let tz = tz.trim_start_matches(':');
            if let Ok(z) = Self::parse(tz) {
                return Ok(z);
            }
        }
        if let Ok(name) = iana_time_zone::get_timezone()
            && let Ok(z) = Self::parse(&name)
        {
            return Ok(z);
        }
        Ok(Self::utc())
    }

    pub fn name(&self) -> &'static str {
        self.0.name()
    }

    /// Local calendar date of an instant.
    pub fn local_date(&self, t: DateTime<Utc>) -> NaiveDate {
        t.with_timezone(&self.0).date_naive()
    }

    /// The first instant of a local date. When midnight does not exist
    /// (a DST gap at midnight) the first valid instant of that date is used.
    pub fn start_of_day(&self, date: NaiveDate) -> DateTime<Utc> {
        let mut time = NaiveTime::MIN;
        for _ in 0..(24 * 4) {
            match self.0.from_local_datetime(&date.and_time(time)) {
                LocalResult::Single(t) => return t.with_timezone(&Utc),
                LocalResult::Ambiguous(a, b) => return a.min(b).with_timezone(&Utc),
                LocalResult::None => time += Duration::minutes(15),
            }
        }
        // Unreachable for real zones; fall back to UTC midnight.
        Utc.from_utc_datetime(&date.and_time(NaiveTime::MIN))
    }

    pub fn format(&self, t: DateTime<Utc>, fmt: &str) -> String {
        t.with_timezone(&self.0).format(fmt).to_string()
    }

    /// RFC 3339 timestamp in this zone.
    pub fn rfc3339(&self, t: DateTime<Utc>) -> String {
        t.with_timezone(&self.0).to_rfc3339()
    }
}

impl fmt::Display for Zone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TimeError {
    #[error("unknown time zone: {0}")]
    UnknownZone(String),
    #[error("invalid date or time: {0} (use YYYY-MM-DD, RFC 3339, today, yesterday, or Nd/Nw/Nm)")]
    InvalidBound(String),
    #[error("empty period: start {start} is not before end {end}")]
    EmptyPeriod { start: String, end: String },
}

/// Half-open period `[start, end)` in UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Period {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

impl Period {
    pub fn new(start: DateTime<Utc>, end: DateTime<Utc>) -> Result<Self, TimeError> {
        if start >= end {
            return Err(TimeError::EmptyPeriod {
                start: start.to_rfc3339(),
                end: end.to_rfc3339(),
            });
        }
        Ok(Self { start, end })
    }

    pub fn contains(&self, t: DateTime<Utc>) -> bool {
        self.start <= t && t < self.end
    }

    pub fn overlaps(&self, other: &Period) -> bool {
        self.start < other.end && other.start < self.end
    }

    pub fn duration(&self) -> Duration {
        self.end - self.start
    }

    /// Local dates touched by this period (inclusive of the last partial day).
    pub fn local_dates(&self, zone: Zone) -> (NaiveDate, NaiveDate) {
        let last = self.end - Duration::nanoseconds(1);
        (zone.local_date(self.start), zone.local_date(last))
    }
}

/// Calendar granularity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Granularity {
    Day,
    Week,
    Month,
}

impl Granularity {
    /// First local date of the bucket containing `date`. Weeks start on Monday.
    pub fn bucket_start(self, date: NaiveDate) -> NaiveDate {
        match self {
            Granularity::Day => date,
            Granularity::Week => {
                date - Duration::days(i64::from(date.weekday().num_days_from_monday()))
            }
            Granularity::Month => date.with_day(1).expect("day 1 exists"),
        }
    }

    /// First local date of the bucket after the one starting at `start`.
    pub fn next(self, start: NaiveDate) -> NaiveDate {
        match self {
            Granularity::Day => start + Duration::days(1),
            Granularity::Week => start + Duration::days(7),
            Granularity::Month => start
                .checked_add_months(Months::new(1))
                .expect("date in range"),
        }
    }

    pub fn label(self, start: NaiveDate) -> String {
        match self {
            Granularity::Day => start.format("%Y-%m-%d").to_string(),
            Granularity::Week => {
                let w = start.iso_week();
                format!("{}-W{:02}", w.year(), w.week())
            }
            Granularity::Month => start.format("%Y-%m").to_string(),
        }
    }
}

/// One calendar bucket, clipped to the requested period.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bucket {
    pub key: NaiveDate,
    pub label: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

/// Splits a period into calendar buckets in `zone`. Buckets are clipped to
/// the period, so they partition it exactly with no overlap or gap.
pub fn buckets(period: Period, zone: Zone, gran: Granularity) -> Vec<Bucket> {
    let (first, _) = period.local_dates(zone);
    let mut key = gran.bucket_start(first);
    let mut out = Vec::new();
    loop {
        let next = gran.next(key);
        let b_start = zone.start_of_day(key).max(period.start);
        let b_end = zone.start_of_day(next).min(period.end);
        if b_start >= period.end {
            break;
        }
        if b_start < b_end {
            out.push(Bucket {
                key,
                label: gran.label(key),
                start: b_start,
                end: b_end,
            });
        }
        key = next;
    }
    out
}

/// Key of the bucket an instant falls into.
pub fn bucket_key(t: DateTime<Utc>, zone: Zone, gran: Granularity) -> NaiveDate {
    gran.bucket_start(zone.local_date(t))
}

/// Parses a period bound.
///
/// Accepted forms: RFC 3339 timestamp, `YYYY-MM-DD` (a whole local day;
/// as an end bound the day is included, so the bound is the next midnight),
/// `today`, `yesterday`, and `Nd` / `Nw` / `Nm` meaning the last N days,
/// weeks or months including the current one (start bounds only).
pub fn parse_bound(
    s: &str,
    zone: Zone,
    is_end: bool,
    now: DateTime<Utc>,
) -> Result<DateTime<Utc>, TimeError> {
    let s = s.trim();
    let err = || TimeError::InvalidBound(s.to_string());
    if let Ok(t) = DateTime::parse_from_rfc3339(s) {
        return Ok(t.with_timezone(&Utc));
    }
    let today = zone.local_date(now);
    let date = if s.eq_ignore_ascii_case("today") {
        Some(today)
    } else if s.eq_ignore_ascii_case("yesterday") {
        Some(today - Duration::days(1))
    } else {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
    };
    if let Some(d) = date {
        let d = if is_end { d + Duration::days(1) } else { d };
        return Ok(zone.start_of_day(d));
    }
    if is_end || s.len() < 2 {
        return Err(err());
    }
    let (num, unit) = s.split_at(s.len() - 1);
    let n: u32 = num.parse().map_err(|_| err())?;
    if n == 0 {
        return Err(err());
    }
    let start = match unit {
        "d" => today - Duration::days(i64::from(n) - 1),
        "w" => Granularity::Week.bucket_start(today) - Duration::days(7 * (i64::from(n) - 1)),
        "m" => Granularity::Month
            .bucket_start(today)
            .checked_sub_months(Months::new(n - 1))
            .ok_or_else(err)?,
        _ => return Err(err()),
    };
    Ok(zone.start_of_day(start))
}

/// Builds a period from optional bounds; a missing end is `now`.
pub fn resolve_period(
    since: Option<&str>,
    until: Option<&str>,
    default_since: &str,
    zone: Zone,
    now: DateTime<Utc>,
) -> Result<Period, TimeError> {
    let start = parse_bound(since.unwrap_or(default_since), zone, false, now)?;
    let end = match until {
        Some(u) => parse_bound(u, zone, true, now)?,
        None => now,
    };
    Period::new(start, end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }
    fn tokyo() -> Zone {
        Zone::parse("Asia/Tokyo").unwrap()
    }
    fn date(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn tokyo_day_boundary() {
        // 2026-11-20T15:00Z is 2026-11-21 00:00 in Tokyo.
        let z = tokyo();
        assert_eq!(
            z.local_date(utc("2026-11-20T14:59:59Z")),
            date("2026-11-20")
        );
        assert_eq!(
            z.local_date(utc("2026-11-20T15:00:00Z")),
            date("2026-11-21")
        );
        assert_eq!(
            z.start_of_day(date("2026-11-21")),
            utc("2026-11-20T15:00:00Z")
        );
    }

    #[test]
    fn changing_zone_moves_event_to_other_day() {
        let t = utc("2026-11-20T20:00:00Z");
        assert_eq!(Zone::utc().local_date(t), date("2026-11-20"));
        assert_eq!(tokyo().local_date(t), date("2026-11-21"));
    }

    #[test]
    fn weeks_start_monday() {
        // 2026-11-21 is a Saturday.
        assert_eq!(
            Granularity::Week.bucket_start(date("2026-11-21")),
            date("2026-11-16")
        );
        assert_eq!(
            Granularity::Week.bucket_start(date("2026-11-16")),
            date("2026-11-16")
        );
        assert_eq!(
            Granularity::Week.bucket_start(date("2026-11-22")),
            date("2026-11-16")
        );
    }

    #[test]
    fn months_and_leap_days() {
        assert_eq!(
            Granularity::Month.next(date("2024-01-01")),
            date("2024-02-01")
        );
        assert_eq!(
            Granularity::Month.next(date("2024-12-01")),
            date("2025-01-01")
        );
        let p = Period::new(utc("2024-02-01T00:00:00Z"), utc("2024-03-01T00:00:00Z")).unwrap();
        let b = buckets(p, Zone::utc(), Granularity::Day);
        assert_eq!(b.len(), 29);
        assert_eq!(b.last().unwrap().key, date("2024-02-29"));
    }

    #[test]
    fn dst_days_have_23_and_25_hours() {
        let ny = Zone::parse("America/New_York").unwrap();
        // DST starts 2026-03-08, ends 2026-11-01.
        let p = Period::new(
            ny.start_of_day(date("2026-03-07")),
            ny.start_of_day(date("2026-03-10")),
        )
        .unwrap();
        let b = buckets(p, ny, Granularity::Day);
        let hours: Vec<i64> = b.iter().map(|b| (b.end - b.start).num_hours()).collect();
        assert_eq!(hours, vec![24, 23, 24]);
        let p = Period::new(
            ny.start_of_day(date("2026-10-31")),
            ny.start_of_day(date("2026-11-03")),
        )
        .unwrap();
        let hours: Vec<i64> = buckets(p, ny, Granularity::Day)
            .iter()
            .map(|b| (b.end - b.start).num_hours())
            .collect();
        assert_eq!(hours, vec![24, 25, 24]);
    }

    #[test]
    fn midnight_gap_uses_first_valid_instant() {
        // Santiago (America/Santiago) skips 00:00 -> 01:00 on DST start
        // (2026-09-06). The day starts at 01:00 local = 04:00Z.
        let z = Zone::parse("America/Santiago").unwrap();
        let start = z.start_of_day(date("2026-09-06"));
        assert_eq!(z.local_date(start), date("2026-09-06"));
        assert_eq!(
            z.local_date(start - Duration::seconds(1)),
            date("2026-09-05")
        );
    }

    #[test]
    fn buckets_partition_period_exactly() {
        let p = Period::new(utc("2026-01-15T10:30:00Z"), utc("2026-04-02T03:00:00Z")).unwrap();
        for gran in [Granularity::Day, Granularity::Week, Granularity::Month] {
            let b = buckets(p, tokyo(), gran);
            assert_eq!(b.first().unwrap().start, p.start);
            assert_eq!(b.last().unwrap().end, p.end);
            for w in b.windows(2) {
                assert_eq!(w[0].end, w[1].start);
            }
        }
    }

    #[test]
    fn period_is_half_open() {
        let p = Period::new(utc("2026-01-01T00:00:00Z"), utc("2026-01-02T00:00:00Z")).unwrap();
        assert!(p.contains(utc("2026-01-01T00:00:00Z")));
        assert!(!p.contains(utc("2026-01-02T00:00:00Z")));
        assert!(Period::new(p.end, p.start).is_err());
    }

    #[test]
    fn parse_bounds() {
        let now = utc("2026-11-21T03:00:00Z"); // 12:00 in Tokyo, Saturday
        let z = tokyo();
        assert_eq!(
            parse_bound("2026-11-01", z, false, now).unwrap(),
            utc("2026-10-31T15:00:00Z")
        );
        // An end date includes the whole day.
        assert_eq!(
            parse_bound("2026-11-01", z, true, now).unwrap(),
            utc("2026-11-01T15:00:00Z")
        );
        assert_eq!(
            parse_bound("today", z, false, now).unwrap(),
            utc("2026-11-20T15:00:00Z")
        );
        assert_eq!(
            parse_bound("7d", z, false, now).unwrap(),
            utc("2026-11-14T15:00:00Z")
        );
        // Current week starts Monday 2026-11-16.
        assert_eq!(
            parse_bound("2w", z, false, now).unwrap(),
            utc("2026-11-08T15:00:00Z")
        );
        assert_eq!(
            parse_bound("3m", z, false, now).unwrap(),
            utc("2026-08-31T15:00:00Z")
        );
        assert_eq!(
            parse_bound("2026-11-01T00:00:00Z", z, false, now).unwrap(),
            utc("2026-11-01T00:00:00Z")
        );
        assert!(parse_bound("7d", z, true, now).is_err());
        assert!(parse_bound("0d", z, false, now).is_err());
        assert!(parse_bound("nonsense", z, false, now).is_err());
        assert!(Zone::parse("Mars/Base").is_err());
    }

    #[test]
    fn year_end_week_label() {
        // 2026-12-28 is Monday of ISO week 2026-W53.
        assert_eq!(Granularity::Week.label(date("2026-12-28")), "2026-W53");
        assert_eq!(Granularity::Month.label(date("2026-12-01")), "2026-12");
    }
}
