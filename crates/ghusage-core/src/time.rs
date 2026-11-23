//! Periods, time zones and calendar buckets backed by Jiff.

use jiff::{SignedDuration, Timestamp, ToSpan, civil::Date, tz::TimeZone};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Zone(pub TimeZone);

impl Zone {
    pub fn utc() -> Self {
        Self(TimeZone::UTC)
    }

    pub fn parse(name: &str) -> Result<Self, TimeError> {
        TimeZone::get(name.trim())
            .map(Self)
            .map_err(|_| TimeError::UnknownZone(name.to_string()))
    }

    pub fn resolve(explicit: Option<&str>) -> Result<Self, TimeError> {
        if let Some(name) = explicit {
            return Self::parse(name);
        }
        if let Ok(tz) = std::env::var("TZ")
            && let Ok(z) = Self::parse(tz.trim_start_matches(':'))
        {
            return Ok(z);
        }
        if let Ok(name) = iana_time_zone::get_timezone()
            && let Ok(z) = Self::parse(&name)
        {
            return Ok(z);
        }
        Ok(Self::utc())
    }

    pub fn name(&self) -> &str {
        self.0.iana_name().unwrap_or("UTC")
    }

    pub fn local_date(&self, t: Timestamp) -> Date {
        t.to_zoned_datetime(self.0.clone()).date()
    }

    /// Jiff's compatible disambiguation advances through a midnight gap and
    /// selects the earlier occurrence in a fold.
    pub fn start_of_day(&self, date: Date) -> Timestamp {
        date.to_zoned_datetime(self.0.clone())
            .expect("valid calendar date")
            .timestamp()
    }

    pub fn format(&self, t: Timestamp, fmt: &str) -> String {
        t.to_zoned_datetime(self.0.clone())
            .strftime(fmt)
            .to_string()
    }

    pub fn rfc3339(&self, t: Timestamp) -> String {
        self.format(t, "%Y-%m-%dT%H:%M:%S%.f%:z")
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
    #[error(
        "invalid date or time: {0} (use YYYY-MM-DD, RFC 3339, today, yesterday, or Nd/Nw/Nm/Ny)"
    )]
    InvalidBound(String),
    #[error("empty period: start {start} is not before end {end}")]
    EmptyPeriod { start: String, end: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Period {
    pub start: Timestamp,
    pub end: Timestamp,
}

impl Period {
    pub fn new(start: Timestamp, end: Timestamp) -> Result<Self, TimeError> {
        if start >= end {
            return Err(TimeError::EmptyPeriod {
                start: start.to_string(),
                end: end.to_string(),
            });
        }
        Ok(Self { start, end })
    }
    pub fn contains(&self, t: Timestamp) -> bool {
        self.start <= t && t < self.end
    }
    pub fn overlaps(&self, other: &Self) -> bool {
        self.start < other.end && other.start < self.end
    }
    pub fn duration(&self) -> SignedDuration {
        self.end.duration_since(self.start)
    }
    pub fn local_dates(&self, zone: &Zone) -> (Date, Date) {
        let last = self
            .end
            .checked_sub(SignedDuration::from_nanos(1))
            .expect("period end is representable");
        (zone.local_date(self.start), zone.local_date(last))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Granularity {
    Day,
    Week,
    Month,
    Year,
}

impl Granularity {
    pub fn bucket_start(self, date: Date) -> Date {
        match self {
            Self::Day => date,
            Self::Week => date
                .checked_sub(i64::from(date.weekday().to_monday_zero_offset()).days())
                .expect("date in range"),
            Self::Month => Date::new(date.year(), date.month(), 1).expect("day 1 exists"),
            Self::Year => Date::new(date.year(), 1, 1).expect("January 1 exists"),
        }
    }
    pub fn next(self, start: Date) -> Date {
        match self {
            Self::Day => start.checked_add(1.days()).expect("date in range"),
            Self::Week => start.checked_add(7.days()).expect("date in range"),
            Self::Month => start.checked_add(1.months()).expect("date in range"),
            Self::Year => start.checked_add(1.years()).expect("date in range"),
        }
    }
    pub fn label(self, start: Date) -> String {
        match self {
            Self::Day => start.strftime("%Y-%m-%d").to_string(),
            Self::Week => {
                let w = start.iso_week_date();
                format!("{}-W{:02}", w.year(), w.week())
            }
            Self::Month => start.strftime("%Y-%m").to_string(),
            Self::Year => start.strftime("%Y").to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bucket {
    pub key: Date,
    pub label: String,
    pub start: Timestamp,
    pub end: Timestamp,
}

pub fn buckets(period: Period, zone: &Zone, gran: Granularity) -> Vec<Bucket> {
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

pub fn bucket_key(t: Timestamp, zone: &Zone, gran: Granularity) -> Date {
    gran.bucket_start(zone.local_date(t))
}

pub fn parse_bound(
    s: &str,
    zone: &Zone,
    is_end: bool,
    now: Timestamp,
) -> Result<Timestamp, TimeError> {
    let s = s.trim();
    let err = || TimeError::InvalidBound(s.to_string());
    if let Ok(t) = s.parse::<Timestamp>() {
        return Ok(t);
    }
    let today = zone.local_date(now);
    let date = if s.eq_ignore_ascii_case("today") {
        Some(today)
    } else if s.eq_ignore_ascii_case("yesterday") {
        Some(today.checked_sub(1.days()).map_err(|_| err())?)
    } else {
        s.parse::<Date>().ok()
    };
    if let Some(d) = date {
        let d = if is_end {
            d.checked_add(1.days()).map_err(|_| err())?
        } else {
            d
        };
        return Ok(zone.start_of_day(d));
    }
    if is_end || s.len() < 2 {
        return Err(err());
    }
    let (num, unit) = s.split_at(s.len() - 1);
    let n: i64 = num.parse().map_err(|_| err())?;
    if n <= 0 {
        return Err(err());
    }
    let start = match unit {
        "d" => today.checked_sub((n - 1).days()),
        "w" => Granularity::Week
            .bucket_start(today)
            .checked_sub((7 * (n - 1)).days()),
        "m" => Granularity::Month
            .bucket_start(today)
            .checked_sub((n - 1).months()),
        "y" => Granularity::Year
            .bucket_start(today)
            .checked_sub((n - 1).years()),
        _ => return Err(err()),
    }
    .map_err(|_| err())?;
    Ok(zone.start_of_day(start))
}

pub fn resolve_period(
    since: Option<&str>,
    until: Option<&str>,
    default_since: &str,
    zone: &Zone,
    now: Timestamp,
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
    fn utc(s: &str) -> Timestamp {
        s.parse().unwrap()
    }
    fn date(s: &str) -> Date {
        s.parse().unwrap()
    }
    #[test]
    fn tokyo_day_boundary() {
        let z = Zone::parse("Asia/Tokyo").unwrap();
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
    fn dst_days_have_23_and_25_hours() {
        let z = Zone::parse("America/New_York").unwrap();
        let p = Period::new(
            z.start_of_day(date("2026-03-07")),
            z.start_of_day(date("2026-03-10")),
        )
        .unwrap();
        let hours: Vec<i64> = buckets(p, &z, Granularity::Day)
            .iter()
            .map(|b| b.end.duration_since(b.start).as_secs_f64() as i64 / 3600)
            .collect();
        assert_eq!(hours, vec![24, 23, 24]);
        let p = Period::new(
            z.start_of_day(date("2026-10-31")),
            z.start_of_day(date("2026-11-03")),
        )
        .unwrap();
        let hours: Vec<i64> = buckets(p, &z, Granularity::Day)
            .iter()
            .map(|b| b.end.duration_since(b.start).as_secs_f64() as i64 / 3600)
            .collect();
        assert_eq!(hours, vec![24, 25, 24]);
    }
    #[test]
    fn parse_bounds() {
        let now = utc("2026-11-21T03:00:00Z");
        let z = Zone::parse("Asia/Tokyo").unwrap();
        assert_eq!(
            parse_bound("2026-11-01", &z, false, now).unwrap(),
            utc("2026-10-31T15:00:00Z")
        );
        assert_eq!(
            parse_bound("7d", &z, false, now).unwrap(),
            utc("2026-11-14T15:00:00Z")
        );
        assert!(parse_bound("7d", &z, true, now).is_err());
    }
}
