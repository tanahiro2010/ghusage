# Periods and time zones

## Half-open periods

Every period is half-open: it includes its start and excludes its end
(`start ≤ t < end`). Adjacent periods therefore never overlap and never leave
a gap: an event at exactly midnight belongs to the day that starts at that
midnight.

Internally all times are UTC. Calendar boundaries (days, weeks, months) are
computed in the selected time zone.

## `--since` and `--until`

```sh
ghusage overview --since 2026-11-01 --until 2026-11-30
ghusage daily --since 14d
ghusage overview --since 2026-11-20T09:00:00+09:00 --until 2026-11-20T18:00:00+09:00
```

Accepted forms:

| Form | As `--since` (start) | As `--until` (end) |
|---|---|---|
| `YYYY-MM-DD` | Local midnight at the start of that date | Local midnight at the **end** of that date, so the whole day is included |
| `today` | Today's local midnight | The end of today |
| `yesterday` | Yesterday's local midnight | The end of yesterday (today's midnight) |
| RFC 3339 timestamp, e.g. `2026-11-20T09:00:00Z` or `2026-11-20T09:00:00+09:00` | That instant | That instant (exclusive) |
| `Nd` | The start of the day N−1 days ago: the last N days including today | not accepted |
| `Nw` | The Monday that starts the week N−1 weeks ago: the last N weeks including the current week | not accepted |
| `Nm` | The first day of the month N−1 months ago: the last N months including the current month | not accepted |

- `N` must be a positive integer (`0d` is rejected).
- When `--until` is omitted, the period ends now.
- When `--since` is omitted, the command's default applies: `7d` for most
  reports, `8w` for `weekly`, `6m` for `monthly`, `30d` for `sync` and `tui`.
- The start must be before the end; otherwise the command fails with exit
  code 2 (`error: empty period: ...`).

Examples, assuming today is Saturday 2026-11-21 in the selected time zone:

| Options | Period |
|---|---|
| `--since 7d` | 2026-11-15 00:00 → now |
| `--since 2w` | Monday 2026-11-09 00:00 → now |
| `--since 3m` | 2026-09-01 00:00 → now |
| `--since today` | 2026-11-21 00:00 → now |
| `--since yesterday --until yesterday` | 2026-11-20 00:00 → 2026-11-21 00:00 |
| `--since 2026-11-01 --until 2026-11-30` | 2026-11-01 00:00 → 2026-12-01 00:00 |

The header of each report shows the resolved period, and JSON contains it as
`period.start` / `period.end`.

## Time zone

The time zone decides where days, weeks and months begin, and which date an
event belongs to. It is chosen in this order:

1. `--timezone <IANA name>`, e.g. `--timezone Asia/Tokyo` or `--timezone UTC`;
2. the `TZ` environment variable, if it holds a valid IANA name (a leading
   `:` is ignored);
3. the operating system's time zone setting;
4. UTC.

An invalid `--timezone` fails with exit code 2. An invalid `TZ` value is
ignored and the next source is used.

The same event can fall on different dates in different time zones. For
example, a commit at 2026-11-20 16:00 UTC is on 2026-11-20 in UTC but on
2026-11-21 in `Asia/Tokyo`.

## Weeks and months

- Weeks start on **Monday** at local midnight. `weekly` labels weeks with
  their ISO week number, e.g. `2026-W47` (a year can have week 53).
- Months are **calendar months** starting on the 1st at local midnight,
  labeled `2026-11`.
- The first and last bucket are clipped to the period. With
  `--since 2026-11-04`, the first week bucket starts on Wednesday 2026-11-04,
  not on Monday. JSON gives each bucket's exact `start` and `end`.

## Daylight saving time

Days are calendar days, not 24-hour blocks. On the day a DST change happens,
the local day is 23 or 25 hours long (or differs by 30 minutes in zones with
such shifts), and its bucket has that length. If local midnight does not
exist on a date because of a DST change, the day starts at the first valid
local time of that date. Events are never lost or counted twice around a
transition.

## Commit dates

Git commits carry two dates: the author date (when the change was first
written) and the committer date (when the commit was last created, for
example by a rebase or amend). `ghusage` uses the committer date by default;
`--commit-date author` uses the author date to decide which period and day a
commit belongs to. Neither date is the push time.

GitHub's commit search finds commits by committer date. With
`--commit-date author`, a commit whose author date is in the period but whose
committer date is outside every synced range is not in the cache and is not
counted.

## Timestamps in output

- Table output shows times in the selected time zone (`YYYY-MM-DD HH:MM`).
- JSON `period.start` / `period.end` are RFC 3339 in the selected time zone
  (e.g. `2026-11-02T00:00:00+09:00`). All other JSON timestamps are RFC 3339
  in UTC (e.g. `2026-11-01T15:00:00Z`).
- CSV timestamps are RFC 3339 in UTC with a `+00:00` offset.
