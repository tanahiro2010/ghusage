//! Command-line interface definition.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "ghusage",
    version,
    about = "Know what you actually did on GitHub.",
    long_about = "Analyze your GitHub activity: commits, pull requests, reviews, comments, issues \
and everything that happens inside pull requests. Data is cached locally in SQLite.",
    after_help = "Exit codes: 0 success, 1 general error, 2 invalid arguments, 3 authentication \
error, 4 API error, 5 cache error, 6 incomplete data with --require-complete.\n\nGlobal options \
that a command does not use are ignored with a warning."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
    #[command(flatten)]
    pub global: GlobalArgs,
}

#[derive(Debug, Clone, Args)]
pub struct GlobalArgs {
    /// Start of the period: YYYY-MM-DD, RFC 3339, today, yesterday, or Nd/Nw/Nm/Ny (last N days/weeks/months/years).
    #[arg(long, global = true)]
    pub since: Option<String>,
    /// End of the period (exclusive). A date includes that whole day. Defaults to now.
    #[arg(long, global = true)]
    pub until: Option<String>,
    /// IANA time zone for day/week/month boundaries (default: TZ or system setting).
    #[arg(long, global = true)]
    pub timezone: Option<String>,
    /// Only this repository (owner/name). Repeatable.
    #[arg(long = "repo", global = true, value_name = "OWNER/NAME")]
    pub repos: Vec<String>,
    /// Only repositories owned by this user or organization. Repeatable.
    #[arg(long = "owner", global = true, value_name = "LOGIN")]
    pub owners: Vec<String>,
    /// Only public or only private repositories.
    #[arg(long, global = true, value_enum)]
    pub visibility: Option<VisibilityArg>,
    /// Only repositories you own, or only other repositories.
    #[arg(long, global = true, value_enum)]
    pub ownership: Option<OwnershipArg>,
    /// Analyze another user's activity (limited to what your token can see).
    #[arg(long, global = true)]
    pub user: Option<String>,
    /// Output format.
    #[arg(long, global = true, value_enum, default_value_t = Format::Table)]
    pub format: Format,
    /// Use only cached data; never contact GitHub.
    #[arg(long, global = true, conflicts_with = "no_cache")]
    pub offline: bool,
    /// Fetch fresh data into a temporary cache instead of the local cache.
    #[arg(long, global = true)]
    pub no_cache: bool,
    /// Exit with code 6 if any data for the period is incomplete.
    #[arg(long, global = true)]
    pub require_complete: bool,
    /// Disable colors (also honors NO_COLOR).
    #[arg(long, global = true)]
    pub no_color: bool,
    /// Include private repository names and titles in JSON/CSV output.
    #[arg(long, global = true)]
    pub include_private: bool,
    /// Only fetch and show public repositories.
    #[arg(long, global = true)]
    pub public_only: bool,
    /// Which commit date decides a commit's day and whether it is in the period.
    #[arg(long, global = true, value_enum, default_value_t = CommitDateArg::Committer)]
    pub commit_date: CommitDateArg,
    /// Read the GitHub token from this file (must not be readable by others).
    #[arg(long, global = true, value_name = "PATH")]
    pub token_file: Option<PathBuf>,
    /// Directory for the cache database (default: OS user data directory, or GHUSAGE_DATA_DIR).
    #[arg(long, global = true, value_name = "PATH")]
    pub data_dir: Option<PathBuf>,
    /// Reports skip syncing when the cache was synced this recently and show
    /// the data as of that sync, e.g. 30s, 5m, 1h; 0 always syncs.
    #[arg(long, global = true, env = "GHUSAGE_MAX_AGE", default_value = "5m", value_parser = parse_max_age)]
    pub max_age: std::time::Duration,
    /// Maximum concurrent GitHub requests (1-16).
    #[arg(long, global = true, default_value_t = 8, value_parser = clap::value_parser!(u8).range(1..=16))]
    pub concurrency: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    Table,
    Json,
    Csv,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum VisibilityArg {
    Public,
    Private,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OwnershipArg {
    Owned,
    External,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CommitDateArg {
    Committer,
    Author,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SortArg {
    Activity,
    Comments,
    Reviews,
    Created,
    Updated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum StateArg {
    Open,
    Closed,
    Merged,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Overview of the period (default command).
    Overview,
    /// Activity per day.
    Daily,
    /// Activity per week (weeks start on Monday).
    Weekly,
    /// Activity per calendar month.
    Monthly,
    /// Activity per calendar year.
    Yearly,
    /// Activity per repository.
    Repos,
    /// Pull requests with activity in the period.
    Prs(PrsArgs),
    /// Reviews you submitted and reviews you received.
    Reviews,
    /// Comments you wrote and comments on your pull requests.
    Comments,
    /// Issues you opened, closed or reopened.
    Issues,
    /// Detail and timeline of one pull request.
    Pr(PrArgs),
    /// Interactive terminal UI.
    Tui,
    /// Fetch data from GitHub into the local cache.
    Sync(SyncArgs),
    /// Check authentication, cache and connectivity.
    Doctor,
    /// Authentication commands.
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    /// Cache commands.
    Cache {
        #[command(subcommand)]
        command: CacheCommand,
    },
}

#[derive(Debug, Args)]
pub struct PrsArgs {
    /// Show activity statistics and lifecycle timings.
    #[arg(long)]
    pub details: bool,
    #[arg(long, value_enum, default_value_t = SortArg::Activity)]
    pub sort: SortArg,
    /// Only pull requests currently in this state.
    #[arg(long, value_enum)]
    pub state: Option<StateArg>,
    /// Include pull requests where only others were active.
    #[arg(long)]
    pub all_participants: bool,
}

#[derive(Debug, Args)]
pub struct PrArgs {
    /// Pull request URL, e.g. https://github.com/owner/repo/pull/123
    pub url: String,
    /// Show the full activity timeline.
    #[arg(long)]
    pub timeline: bool,
    /// Only these event types in the timeline (e.g. review_submitted,inline_comment). Repeatable.
    #[arg(long = "event-type", value_delimiter = ',')]
    pub event_types: Vec<String>,
}

#[derive(Debug, Args)]
pub struct SyncArgs {
    /// Refetch the whole period instead of only missing ranges.
    #[arg(long)]
    pub full: bool,
}

#[derive(Debug, Subcommand)]
pub enum AuthCommand {
    /// Show which credentials are used and whether they work.
    Status,
}

#[derive(Debug, Subcommand)]
pub enum CacheCommand {
    /// Show cache location, size, last sync and coverage.
    Status,
    /// Delete all cached GitHub data (credentials are not affected).
    Clear,
}

/// Parses `0`, `Ns`, `Nm` or `Nh`.
fn parse_max_age(s: &str) -> Result<std::time::Duration, String> {
    let s = s.trim();
    if s == "0" {
        return Ok(std::time::Duration::ZERO);
    }
    let (num, unit) = s.split_at(s.len().saturating_sub(1));
    let n: u64 = num
        .parse()
        .map_err(|_| format!("invalid duration {s:?}; use e.g. 30s, 5m, 1h or 0"))?;
    let secs = match unit {
        "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        _ => return Err(format!("invalid duration {s:?}; use e.g. 30s, 5m, 1h or 0")),
    };
    Ok(std::time::Duration::from_secs(secs))
}

#[cfg(test)]
mod tests {
    use super::parse_max_age;
    use std::time::Duration;

    #[test]
    fn max_age_accepts_seconds_minutes_hours_and_zero() {
        assert_eq!(parse_max_age("0"), Ok(Duration::ZERO));
        assert_eq!(parse_max_age("30s"), Ok(Duration::from_secs(30)));
        assert_eq!(parse_max_age("5m"), Ok(Duration::from_secs(300)));
        assert_eq!(parse_max_age("2h"), Ok(Duration::from_secs(7200)));
        assert!(parse_max_age("5").is_err());
        assert!(parse_max_age("m").is_err());
        assert!(parse_max_age("-1m").is_err());
    }
}
