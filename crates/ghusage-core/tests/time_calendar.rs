//! Calendar and time-zone behavior of periods and buckets: half-open
//! boundaries, Monday-start weeks, calendar months, month/year ends, leap
//! days and daylight-saving transitions.

mod common;

use common::*;
use ghusage_core::metrics::{self, summary};
use ghusage_core::model::Visibility;
use ghusage_core::time::{Granularity, Period, bucket_key, buckets, parse_bound, resolve_period};
use jiff::SignedDuration;

fn commits_dataset(times: &[&str]) -> metrics::Dataset {
    let events = times
        .iter()
        .enumerate()
        .map(|(i, t)| commit("R", &format!("sha{i}"), me(), t))
        .collect();
    dataset(
        vec![repo("R", "me", "tool", Visibility::Public)],
        vec![],
        vec![],
        events,
    )
}

fn per_bucket(ds: &metrics::Dataset, opts: &metrics::Options, g: Granularity) -> Vec<u64> {
    metrics::breakdown(ds, opts, g)
        .iter()
        .map(|b| b.summary.commits)
        .collect()
}

fn minutes(p: Period, z: &str, g: Granularity) -> Vec<i64> {
    let zone = zone(z);
    buckets(p, &zone, g)
        .iter()
        .map(|b| b.end.duration_since(b.start).as_secs() / 60)
        .collect()
}

// ---------------------------------------------------------------------------
// Half-open periods
// ---------------------------------------------------------------------------

#[test]
fn period_includes_its_start_and_excludes_its_end() {
    let ds = commits_dataset(&[
        "2026-11-19T23:59:59Z", // before
        "2026-11-20T00:00:00Z", // start: in
        "2026-11-20T23:59:59Z", // in
        "2026-11-21T00:00:00Z", // end: out
    ]);
    let opts = options("2026-11-20T00:00:00Z", "2026-11-21T00:00:00Z", zone("UTC"));
    assert_eq!(summary(&ds, &opts).commits, 2);
}

#[test]
fn adjacent_periods_count_a_boundary_event_exactly_once() {
    let ds = commits_dataset(&["2026-11-20T00:00:00Z"]);
    let left = options("2026-11-19T00:00:00Z", "2026-11-20T00:00:00Z", zone("UTC"));
    let right = options("2026-11-20T00:00:00Z", "2026-11-21T00:00:00Z", zone("UTC"));
    let whole = options("2026-11-19T00:00:00Z", "2026-11-21T00:00:00Z", zone("UTC"));
    assert_eq!(summary(&ds, &left).commits, 0);
    assert_eq!(summary(&ds, &right).commits, 1);
    assert_eq!(summary(&ds, &whole).commits, 1);
}

#[test]
fn empty_or_inverted_period_is_rejected() {
    let t = utc("2026-11-20T00:00:00Z");
    assert!(Period::new(t, t).is_err());
    assert!(Period::new(t, t - SignedDuration::from_secs(1)).is_err());
}

// ---------------------------------------------------------------------------
// Weeks
// ---------------------------------------------------------------------------

#[test]
fn weeks_start_on_monday_and_are_clipped_to_the_period() {
    // 2026-11-18 is a Wednesday; 2026-12-02 is a Wednesday.
    let p = Period::new(utc("2026-11-18T00:00:00Z"), utc("2026-12-02T00:00:00Z")).unwrap();
    let zone = zone("UTC");
    let b = buckets(p, &zone, Granularity::Week);
    let keys: Vec<_> = b.iter().map(|b| b.key).collect();
    assert_eq!(
        keys,
        vec![date("2026-11-16"), date("2026-11-23"), date("2026-11-30")]
    );
    let labels: Vec<_> = b.iter().map(|b| b.label.as_str()).collect();
    // 2026-01-01 is a Thursday, so ISO week 1 starts 2025-12-29 and
    // 2026-11-16 is 46 weeks later: week 47.
    assert_eq!(labels, vec!["2026-W47", "2026-W48", "2026-W49"]);
    assert_eq!(b[0].start, p.start);
    assert_eq!(b[0].end, utc("2026-11-23T00:00:00Z"));
    assert_eq!(b[2].start, utc("2026-11-30T00:00:00Z"));
    assert_eq!(b[2].end, p.end);
}

#[test]
fn sunday_night_and_monday_morning_fall_in_different_weeks() {
    let ds = commits_dataset(&["2026-11-22T23:59:59Z", "2026-11-23T00:00:00Z"]);
    let opts = local_days("2026-11-16", "2026-11-30", zone("UTC"));
    assert_eq!(per_bucket(&ds, &opts, Granularity::Week), vec![1, 1]);
}

#[test]
fn tokyo_week_boundary_differs_from_utc() {
    // 2026-11-22T15:00Z is Sunday in UTC but Monday 00:00 in Tokyo.
    let t = utc("2026-11-22T15:00:00Z");
    assert_eq!(
        bucket_key(t, &zone("UTC"), Granularity::Week),
        date("2026-11-16")
    );
    assert_eq!(
        bucket_key(t, &zone("Asia/Tokyo"), Granularity::Week),
        date("2026-11-23")
    );
    let ds = commits_dataset(&["2026-11-22T15:00:00Z"]);
    let tokyo = local_days("2026-11-16", "2026-11-30", zone("Asia/Tokyo"));
    assert_eq!(per_bucket(&ds, &tokyo, Granularity::Week), vec![0, 1]);
}

// ---------------------------------------------------------------------------
// Months, month end, year end, leap day
// ---------------------------------------------------------------------------

#[test]
fn months_are_calendar_months() {
    let p = Period::new(utc("2026-01-01T00:00:00Z"), utc("2026-04-01T00:00:00Z")).unwrap();
    // Jan 31 d, Feb 28 d (2026 is not a leap year), Mar 31 d
    assert_eq!(
        minutes(p, "UTC", Granularity::Month),
        vec![31 * 1440, 28 * 1440, 31 * 1440]
    );
    let ds = commits_dataset(&["2026-01-31T23:59:59Z", "2026-02-01T00:00:00Z"]);
    let opts = local_days("2026-01-01", "2026-04-01", zone("UTC"));
    assert_eq!(per_bucket(&ds, &opts, Granularity::Month), vec![1, 1, 0]);
}

#[test]
fn tokyo_month_starts_at_utc_15_00_of_the_previous_day() {
    let tokyo = zone("Asia/Tokyo");
    let opts = local_days("2026-01-01", "2026-03-01", tokyo.clone());
    let b = buckets(opts.period, &tokyo, Granularity::Month);
    assert_eq!(b.len(), 2);
    assert_eq!(b[1].key, date("2026-02-01"));
    assert_eq!(b[1].start, utc("2026-01-31T15:00:00Z"));
    // 2026-01-31T15:30Z is already February in Tokyo
    let ds = commits_dataset(&["2026-01-31T15:30:00Z"]);
    assert_eq!(per_bucket(&ds, &opts, Granularity::Month), vec![0, 1]);
}

#[test]
fn year_end_is_split_by_month_but_shares_an_iso_week() {
    let ds = commits_dataset(&["2026-12-31T23:59:59Z", "2027-01-01T00:00:00Z"]);
    let utc_zone = zone("UTC");
    let months = local_days("2026-12-01", "2027-02-01", utc_zone.clone());
    assert_eq!(per_bucket(&ds, &months, Granularity::Month), vec![1, 1]);
    let days = local_days("2026-12-31", "2027-01-02", utc_zone.clone());
    assert_eq!(per_bucket(&ds, &days, Granularity::Day), vec![1, 1]);
    // 2026-12-31 is a Thursday: its week starts Monday 2026-12-28, which is
    // ISO week 53 of 2026 (2026 starts on a Thursday). 2027-01-01 (Friday)
    // belongs to that same week.
    let weeks = local_days("2026-12-28", "2027-01-04", utc_zone);
    let b = metrics::breakdown(&ds, &weeks, Granularity::Week);
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].bucket.label, "2026-W53");
    assert_eq!(b[0].summary.commits, 2);
}

#[test]
fn tokyo_new_year_starts_nine_hours_earlier_than_utc() {
    // 2026-12-31T15:00Z is 2027-01-01 00:00 in Tokyo.
    let ds = commits_dataset(&["2026-12-31T14:59:59Z", "2026-12-31T15:00:00Z"]);
    let tokyo = local_days("2026-12-01", "2027-02-01", zone("Asia/Tokyo"));
    let b = metrics::breakdown(&ds, &tokyo, Granularity::Month);
    assert_eq!(b[0].bucket.label, "2026-12");
    assert_eq!(b[1].bucket.label, "2027-01");
    assert_eq!(per_bucket(&ds, &tokyo, Granularity::Month), vec![1, 1]);
    let utc_months = local_days("2026-12-01", "2027-02-01", zone("UTC"));
    assert_eq!(per_bucket(&ds, &utc_months, Granularity::Month), vec![2, 0]);
}

#[test]
fn leap_day_2028_is_its_own_day() {
    let feb = local_days("2028-02-01", "2028-03-01", zone("UTC"));
    let b = buckets(feb.period, &feb.zone, Granularity::Day);
    assert_eq!(b.len(), 29);
    assert_eq!(b[28].key, date("2028-02-29"));
    assert_eq!(b[28].label, "2028-02-29");

    let ds = commits_dataset(&[
        "2028-02-28T23:59:59Z",
        "2028-02-29T12:00:00Z",
        "2028-03-01T00:00:00Z",
    ]);
    let days = local_days("2028-02-28", "2028-03-02", zone("UTC"));
    // 02-28, 02-29, 03-01
    assert_eq!(per_bucket(&ds, &days, Granularity::Day), vec![1, 1, 1]);

    let q = Period::new(utc("2028-01-01T00:00:00Z"), utc("2028-04-01T00:00:00Z")).unwrap();
    assert_eq!(
        minutes(q, "UTC", Granularity::Month),
        vec![31 * 1440, 29 * 1440, 31 * 1440]
    );
    // the following year has no leap day
    let feb27 = local_days("2027-02-01", "2027-03-01", zone("UTC"));
    assert_eq!(
        buckets(feb27.period, &feb27.zone, Granularity::Day).len(),
        28
    );
}

#[test]
fn leap_day_in_tokyo_starts_at_utc_15_00_on_the_28th() {
    let ds = commits_dataset(&["2028-02-28T15:00:00Z"]);
    let tokyo = local_days("2028-02-28", "2028-03-01", zone("Asia/Tokyo"));
    // 02-28 Tokyo: 0, 02-29 Tokyo: 1
    assert_eq!(per_bucket(&ds, &tokyo, Granularity::Day), vec![0, 1]);
}

// ---------------------------------------------------------------------------
// Daylight saving time
// ---------------------------------------------------------------------------

#[test]
fn new_york_dst_days_are_23_and_25_hours() {
    let ny = zone("America/New_York");
    let spring = local_days("2026-03-07", "2026-03-10", ny.clone());
    assert_eq!(
        minutes(spring.period, "America/New_York", Granularity::Day),
        vec![1440, 23 * 60, 1440]
    );
    let fall = local_days("2026-10-31", "2026-11-03", ny);
    assert_eq!(
        minutes(fall.period, "America/New_York", Granularity::Day),
        vec![1440, 25 * 60, 1440]
    );
}

#[test]
fn london_dst_days_are_23_and_25_hours() {
    let london = zone("Europe/London");
    // BST starts Sunday 2026-03-29 and ends Sunday 2026-10-25.
    let spring = local_days("2026-03-29", "2026-03-30", london.clone());
    assert_eq!(spring.period.start, utc("2026-03-29T00:00:00Z"));
    assert_eq!(spring.period.end, utc("2026-03-29T23:00:00Z"));
    let fall = local_days("2026-10-25", "2026-10-26", london);
    assert_eq!(fall.period.start, utc("2026-10-24T23:00:00Z"));
    assert_eq!(fall.period.end, utc("2026-10-26T00:00:00Z"));
    assert_eq!(
        minutes(fall.period, "Europe/London", Granularity::Day),
        vec![25 * 60]
    );
}

#[test]
fn lord_howe_half_hour_dst_shift() {
    // Lord Howe moves by 30 minutes: 2026-04-05 (end) is 24.5 h and
    // 2026-10-04 (start) is 23.5 h.
    let lh = "Australia/Lord_Howe";
    let april = local_days("2026-04-04", "2026-04-07", zone(lh));
    assert_eq!(
        minutes(april.period, lh, Granularity::Day),
        vec![1440, 1470, 1440]
    );
    let october = local_days("2026-10-03", "2026-10-06", zone(lh));
    assert_eq!(
        minutes(october.period, lh, Granularity::Day),
        vec![1440, 1410, 1440]
    );
}

#[test]
fn dst_month_buckets_still_cover_every_event() {
    let ny = zone("America/New_York");
    // March 2026 in New York has 31 days minus one hour.
    let opts = local_days("2026-03-01", "2026-04-01", ny);
    assert_eq!(
        minutes(opts.period, "America/New_York", Granularity::Month),
        vec![31 * 1440 - 60]
    );
    let ds = commits_dataset(&[
        "2026-03-01T05:00:00Z", // 03-01 00:00 EST: first instant
        "2026-03-08T06:59:59Z", // 01:59:59 EST just before the jump
        "2026-03-08T07:00:00Z", // 03:00 EDT just after the jump
        "2026-04-01T03:59:59Z", // 03-31 23:59:59 EDT: last instant
        "2026-04-01T04:00:00Z", // 04-01 00:00 EDT: outside
    ]);
    assert_eq!(per_bucket(&ds, &opts, Granularity::Month), vec![4]);
    assert_eq!(summary(&ds, &opts).commits, 4);
}

// ---------------------------------------------------------------------------
// Date assignment and bounds
// ---------------------------------------------------------------------------

#[test]
fn tokyo_and_utc_assign_dates_nine_hours_apart() {
    let tokyo = zone("Asia/Tokyo");
    let u = zone("UTC");
    let cases = [
        // (instant, UTC date, Tokyo date)
        ("2026-11-20T00:00:00Z", "2026-11-20", "2026-11-20"),
        ("2026-11-20T14:59:59Z", "2026-11-20", "2026-11-20"),
        ("2026-11-20T15:00:00Z", "2026-11-20", "2026-11-21"),
        ("2026-11-20T23:59:59Z", "2026-11-20", "2026-11-21"),
    ];
    for (t, d_utc, d_tokyo) in cases {
        assert_eq!(u.local_date(utc(t)), date(d_utc), "{t} in UTC");
        assert_eq!(tokyo.local_date(utc(t)), date(d_tokyo), "{t} in Tokyo");
    }
}

#[test]
fn date_bounds_cover_whole_local_days_across_dst() {
    let ny = zone("America/New_York");
    let now = utc("2026-11-21T03:00:00Z");
    // start of 03-08 is midnight EST; the inclusive end is midnight EDT of 03-09
    assert_eq!(
        parse_bound("2026-03-08", &ny, false, now).unwrap(),
        utc("2026-03-08T05:00:00Z")
    );
    assert_eq!(
        parse_bound("2026-03-08", &ny, true, now).unwrap(),
        utc("2026-03-09T04:00:00Z")
    );
    let p = resolve_period(Some("2026-03-08"), Some("2026-03-08"), "7d", &ny, now).unwrap();
    assert_eq!((p.end - p.start).as_hours(), 23);
}

#[test]
fn utc_and_tokyo_date_bounds() {
    let now = utc("2026-11-21T03:00:00Z");
    let p = resolve_period(
        Some("2026-11-01"),
        Some("2026-11-30"),
        "7d",
        &zone("UTC"),
        now,
    )
    .unwrap();
    assert_eq!(p.start, utc("2026-11-01T00:00:00Z"));
    assert_eq!(p.end, utc("2026-12-01T00:00:00Z"));
    let p = resolve_period(
        Some("2026-11-01"),
        Some("2026-11-30"),
        "7d",
        &zone("Asia/Tokyo"),
        now,
    )
    .unwrap();
    assert_eq!(p.start, utc("2026-10-31T15:00:00Z"));
    assert_eq!(p.end, utc("2026-11-30T15:00:00Z"));
}

#[test]
fn buckets_partition_partial_day_periods() {
    // Starts and ends mid-day in Tokyo; the first and last buckets are clipped.
    let p = Period::new(utc("2026-11-20T10:00:00Z"), utc("2026-11-23T02:00:00Z")).unwrap();
    for g in [Granularity::Day, Granularity::Week, Granularity::Month] {
        let b = buckets(p, &zone("Asia/Tokyo"), g);
        assert_eq!(b.first().unwrap().start, p.start);
        assert_eq!(b.last().unwrap().end, p.end);
        for w in b.windows(2) {
            assert_eq!(w[0].end, w[1].start);
        }
    }
    // Tokyo days touched: 11-20 (19:00..), 11-21, 11-22, 11-23 (..11:00)
    assert_eq!(buckets(p, &zone("Asia/Tokyo"), Granularity::Day).len(), 4);
}

#[test]
fn years_are_calendar_years_in_the_report_zone() {
    // 2026-12-31T15:00Z is already 2027 in Tokyo, still 2026 in UTC.
    let ds = commits_dataset(&[
        "2025-06-01T00:00:00Z",
        "2026-12-31T14:59:59Z",
        "2026-12-31T15:00:00Z",
    ]);
    let tokyo = local_days("2025-01-01", "2028-01-01", zone("Asia/Tokyo"));
    let b = metrics::breakdown(&ds, &tokyo, Granularity::Year);
    let labels: Vec<&str> = b.iter().map(|x| x.bucket.label.as_str()).collect();
    assert_eq!(labels, vec!["2025", "2026", "2027"]);
    assert_eq!(per_bucket(&ds, &tokyo, Granularity::Year), vec![1, 1, 1]);
    let utc_years = local_days("2025-01-01", "2028-01-01", zone("UTC"));
    assert_eq!(
        per_bucket(&ds, &utc_years, Granularity::Year),
        vec![1, 2, 0]
    );
    // The Tokyo year 2027 starts at 2026-12-31T15:00Z.
    assert_eq!(b[2].bucket.start, utc("2026-12-31T15:00:00Z"));
}

#[test]
fn relative_years_start_on_january_first() {
    let now = utc("2026-11-21T03:00:00Z");
    let utc_zone = zone("UTC");
    // 1y: this calendar year; 3y: this year and the two before.
    let p = resolve_period(Some("1y"), None, "7d", &utc_zone, now).unwrap();
    assert_eq!(p.start, utc("2026-01-01T00:00:00Z"));
    let p = resolve_period(Some("3y"), None, "7d", &utc_zone, now).unwrap();
    assert_eq!(p.start, utc("2024-01-01T00:00:00Z"));
    assert_eq!(p.end, now);
    assert!(resolve_period(Some("0y"), None, "7d", &utc_zone, now).is_err());
}
