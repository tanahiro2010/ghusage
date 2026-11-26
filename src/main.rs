//! Command-line entry point for ghusage.

mod cli;
mod progress;
mod render;

use std::fmt::Write as _;
use std::io::{IsTerminal, Write};
use std::process::ExitCode;

use clap::Parser;
use ghusage_app::report::{self, QueryContext, ReportKind};
use ghusage_app::session::{Session, SessionConfig};
use ghusage_app::sync::SyncReport;
use ghusage_app::{AppError, Result, commands, csv_out, exit};
use ghusage_core::coverage::CoverageStatus;
use ghusage_core::metrics::{
    CommitDateBasis, Filter, Ownership, Participation, PrSort, VisibilityFilter,
};
use ghusage_core::model::{EventType, PrState};
use ghusage_core::time::{Granularity, Period, Zone, resolve_period};
use jiff::{SignedDuration, Timestamp};

use cli::{AuthCommand, CacheCommand, Cli, Command, Format, GlobalArgs};
use render::Style;

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => {
            // Help and version go to stdout with success; usage errors exit 2.
            let code = if e.use_stderr() {
                exit::INVALID_ARGS
            } else {
                0
            };
            drop(e.print());
            return ExitCode::from(u8::try_from(code).unwrap_or(u8::MAX));
        }
    };
    init_logging();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: cannot start runtime: {e}");
            return ExitCode::from(u8::try_from(exit::GENERAL).unwrap_or(u8::MAX));
        }
    };
    match runtime.block_on(run(cli)) {
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(u8::MAX)),
        Err(e) => {
            eprintln!("error: {}", clean(&e));
            ExitCode::from(u8::try_from(e.exit_code()).unwrap_or(u8::MAX))
        }
    }
}

fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_env("GHUSAGE_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    drop(
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .with_ansi(false)
            .try_init(),
    );
}

fn style(g: &GlobalArgs, stream_is_tty: bool) -> Style {
    let no_color_env = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
    Style {
        color: stream_is_tty && !g.no_color && !no_color_env,
    }
}

fn session_config(g: &GlobalArgs) -> SessionConfig {
    SessionConfig {
        data_dir: g.data_dir.clone(),
        no_cache: g.no_cache,
        offline: g.offline,
        user: g.user.clone(),
        token_file: g.token_file.clone(),
        public_only: g.public_only,
        client: ghusage_app::session::client_config(g.jobs as usize),
    }
}

fn filter(g: &GlobalArgs) -> Filter {
    Filter {
        repositories: g.repos.clone(),
        owners: g.owners.clone(),
        visibility: g.visibility.map(|v| match v {
            cli::VisibilityArg::Public => VisibilityFilter::Public,
            cli::VisibilityArg::Private => VisibilityFilter::Private,
        }),
        ownership: g.ownership.map(|o| match o {
            cli::OwnershipArg::Owned => Ownership::Owned,
            cli::OwnershipArg::External => Ownership::External,
        }),
    }
}

const fn commit_basis(g: &GlobalArgs) -> CommitDateBasis {
    match g.commit_date {
        cli::CommitDateArg::Committer => CommitDateBasis::Committer,
        cli::CommitDateArg::Author => CommitDateBasis::Author,
    }
}

const fn default_since(cmd: &Command) -> &'static str {
    match cmd {
        Command::Weekly => "8w",
        Command::Monthly => "6m",
        Command::Yearly => "3y",
        Command::Sync(_) | Command::Tui => "30d",
        _ => "7d",
    }
}

fn period(g: &GlobalArgs, cmd: &Command, zone: &Zone) -> Result<Period> {
    Ok(resolve_period(
        g.since.as_deref(),
        g.until.as_deref(),
        default_since(cmd),
        zone,
        Timestamp::now(),
    )?)
}

/// Text from errors, which may quote GitHub responses, made safe for the
/// terminal.
fn clean(e: &dyn std::fmt::Display) -> String {
    ghusage_core::sanitize::sanitize_line(&e.to_string())
}

fn print_stdout(s: &str) -> Result<()> {
    let mut out = std::io::stdout().lock();
    match out.write_all(s.as_bytes()).and_then(|()| out.flush()) {
        Ok(()) => Ok(()),
        // A closed pipe (e.g. `| head`) is not an error.
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        Err(e) => Err(AppError::Other(format!("cannot write output: {e}"))),
    }
}

fn to_json<T: serde::Serialize>(v: &T) -> Result<String> {
    serde_json::to_string_pretty(v)
        .map(|s| s + "\n")
        .map_err(|e| AppError::Other(e.to_string()))
}

/// The command as typed, for messages.
const fn command_name(cmd: &Command) -> &'static str {
    match cmd {
        Command::Overview => "overview",
        Command::Daily => "daily",
        Command::Weekly => "weekly",
        Command::Monthly => "monthly",
        Command::Yearly => "yearly",
        Command::Repos => "repos",
        Command::Prs(_) => "prs",
        Command::Reviews => "reviews",
        Command::Comments => "comments",
        Command::Issues => "issues",
        Command::Pr(_) => "pr",
        Command::Commit(_) => "commit",
        Command::Tui => "tui",
        Command::Sync(_) => "sync",
        Command::Doctor => "doctor",
        Command::Auth {
            command: AuthCommand::Status,
        } => "auth status",
        Command::Cache {
            command: CacheCommand::Status,
        } => "cache status",
        Command::Cache {
            command: CacheCommand::Clear,
        } => "cache clear",
    }
}

/// Global options that were given but that `cmd` does not use.
fn unused_options(g: &GlobalArgs, cmd: &Command) -> Vec<&'static str> {
    // Reports and the TUI use everything; sync uses the period only.
    let (period, filters) = match cmd {
        Command::Sync(_) => (true, false),
        Command::Pr(_)
        | Command::Commit(_)
        | Command::Doctor
        | Command::Auth { .. }
        | Command::Cache { .. } => (false, false),
        _ => (true, true),
    };
    let mut unused = Vec::new();
    if !period {
        if g.since.is_some() {
            unused.push("--since");
        }
        if g.until.is_some() {
            unused.push("--until");
        }
    }
    if !filters {
        if !g.repos.is_empty() {
            unused.push("--repo");
        }
        if !g.owners.is_empty() {
            unused.push("--owner");
        }
        if g.visibility.is_some() {
            unused.push("--visibility");
        }
        if g.ownership.is_some() {
            unused.push("--ownership");
        }
        if g.commit_date != cli::CommitDateArg::Committer {
            unused.push("--commit-date");
        }
    }
    unused
}

async fn run(cli: Cli) -> Result<i32> {
    let g = cli.global.clone();
    let cmd = cli.command.unwrap_or(Command::Overview);
    for option in unused_options(&g, &cmd) {
        eprintln!(
            "warning: {option} has no effect on `{}`",
            command_name(&cmd)
        );
    }
    if g.format == Format::Csv
        && matches!(
            cmd,
            Command::Sync(_)
                | Command::Doctor
                | Command::Auth { .. }
                | Command::Cache { .. }
                | Command::Tui
                | Command::Commit(_)
        )
    {
        return Err(AppError::InvalidArgs(
            "--format csv is not available for this command; use table or json".into(),
        ));
    }
    if g.format == Format::Markdown
        && !matches!(
            cmd,
            Command::Overview
                | Command::Daily
                | Command::Weekly
                | Command::Monthly
                | Command::Yearly
                | Command::Repos
        )
    {
        return Err(AppError::InvalidArgs(
            "--format markdown is available for overview, daily, weekly, monthly, yearly and repos"
                .into(),
        ));
    }
    let zone = Zone::resolve(g.timezone.as_deref())?;
    match cmd {
        Command::Overview => report_cmd(&g, &cmd, zone, ReportKind::Overview, false).await,
        Command::Daily => {
            report_cmd(
                &g,
                &cmd,
                zone,
                ReportKind::Breakdown(Granularity::Day),
                false,
            )
            .await
        }
        Command::Weekly => {
            report_cmd(
                &g,
                &cmd,
                zone,
                ReportKind::Breakdown(Granularity::Week),
                false,
            )
            .await
        }
        Command::Monthly => {
            report_cmd(
                &g,
                &cmd,
                zone,
                ReportKind::Breakdown(Granularity::Month),
                false,
            )
            .await
        }
        Command::Yearly => {
            report_cmd(
                &g,
                &cmd,
                zone,
                ReportKind::Breakdown(Granularity::Year),
                false,
            )
            .await
        }
        Command::Repos => report_cmd(&g, &cmd, zone, ReportKind::Repositories, false).await,
        Command::Reviews => report_cmd(&g, &cmd, zone, ReportKind::Reviews, false).await,
        Command::Comments => report_cmd(&g, &cmd, zone, ReportKind::Comments, false).await,
        Command::Issues => report_cmd(&g, &cmd, zone, ReportKind::Issues, false).await,
        Command::Prs(ref a) => {
            let kind = ReportKind::PullRequests {
                participation: if a.all_participants {
                    Participation::All
                } else {
                    Participation::Subject
                },
                state: a.state.map(|s| match s {
                    cli::StateArg::Open => PrState::Open,
                    cli::StateArg::Closed => PrState::Closed,
                    cli::StateArg::Merged => PrState::Merged,
                }),
                sort: match a.sort {
                    cli::SortArg::Activity => PrSort::Activity,
                    cli::SortArg::Comments => PrSort::Comments,
                    cli::SortArg::Reviews => PrSort::Reviews,
                    cli::SortArg::Created => PrSort::Created,
                    cli::SortArg::Updated => PrSort::Updated,
                },
            };
            report_cmd(&g, &cmd, zone, kind, a.details).await
        }
        Command::Pr(ref a) => pr_cmd(&g, a, zone).await,
        Command::Commit(ref a) => commit_cmd(&g, a, zone).await,
        Command::Sync(ref a) => sync_cmd(&g, &cmd, zone, a.full, a.diff_stats).await,
        Command::Tui => tui_cmd(&g, &cmd, zone).await,
        Command::Doctor => doctor_cmd(&g, zone).await,
        Command::Auth {
            command: AuthCommand::Status,
        } => auth_status_cmd(&g).await,
        Command::Cache { command } => cache_cmd(&g, command, zone),
    }
}

/// Opens a session and fills gaps for `period` unless offline. Returns the
/// period to report: when the cache was synced within `--max-age`, it ends
/// at that sync instead of fetching the last few minutes again.
async fn prepared_session(g: &GlobalArgs, period: Period, zone: Zone) -> Result<(Session, Period)> {
    // A remembered viewer is checked at the same time as the sync starts;
    // when nothing is synced, it is checked before the cache is shown.
    let mut s = Session::open_deferred(session_config(g)).await?;
    let mut shown = period;
    loop {
        if s.is_offline() || !s.needs_sync(period)? {
            if s.is_unconfirmed() {
                s.confirm_subject().await?;
                continue;
            }
            break;
        }
        let now = Timestamp::now();
        let max_age =
            SignedDuration::from_secs(i64::try_from(g.max_age.as_secs()).unwrap_or(i64::MAX));
        if let Some(until) = s.synced_until(period)?
            && until < period.end
            && until.duration_until(now) <= max_age
        {
            if s.is_unconfirmed() {
                s.confirm_subject().await?;
                continue;
            }
            let mins = until.duration_until(now).as_secs() / 60;
            eprintln!(
                "note: showing data synced {} ago (up to {}); use --max-age 0 to fetch the latest",
                if mins < 1 {
                    "less than a minute".to_string()
                } else {
                    format!("{mins} min")
                },
                zone.format(until, "%Y-%m-%d %H:%M")
            );
            shown = Period::new(period.start, until)?;
            break;
        }
        let progress = progress::Progress::start(s.api_activity());
        let synced = s
            .sync(period, false, g.jobs as usize, &|p| {
                progress.update(p);
            })
            .await;
        let counted = match synced {
            Ok(_) if auto_diff_stats(g, &s)? => {
                Some(s.fetch_line_counts(period, &|p| progress.update(p)).await)
            }
            _ => None,
        };
        drop(progress);
        match synced {
            Ok(r) => {
                for f in r.failures.iter().take(5) {
                    eprintln!("warning: {}", ghusage_core::sanitize::sanitize_line(f));
                }
                match counted {
                    Some(Err(e @ (AppError::AuthRejected(_) | AppError::Auth(_)))) => {
                        return Err(e);
                    }
                    Some(Err(e)) => eprintln!("warning: line counts not fetched: {}", clean(&e)),
                    _ => {}
                }
            }
            Err(e @ (AppError::AuthRejected(_) | AppError::Auth(_))) => return Err(e),
            // The sync failed before the account was checked (for example
            // another sync holds the lock): check it now, then show the
            // cache as for any failed sync. A failed check fails the report.
            Err(e) if s.is_unconfirmed() => {
                s.confirm_subject().await?;
                if s.offline_state().reason.is_none() {
                    eprintln!("warning: sync failed, showing cached data: {}", clean(&e));
                }
            }
            // GitHub turned out to be unreachable; reported below.
            Err(_) if s.offline_state().reason.is_some() => {}
            Err(e) => {
                eprintln!("warning: sync failed, showing cached data: {}", clean(&e));
            }
        }
        break;
    }
    if let Some(reason) = &s.offline_state().reason {
        eprintln!(
            "warning: GitHub is not reachable ({}); showing cached data",
            clean(reason)
        );
    }
    Ok((s, shown))
}

/// Whether a sync also fetches line counts without `--diff-stats`: on the
/// first sync of an account (the sync just run is its only finished one)
/// and in CI, unless `--no-diff-stats` is given.
fn auto_diff_stats(g: &GlobalArgs, s: &Session) -> Result<bool> {
    if g.no_diff_stats {
        return Ok(false);
    }
    Ok(in_ci() || s.completed_syncs()? <= 1)
}

/// Whether ghusage runs in CI: `CI` is set to anything but empty, `false`
/// or `0` (GitHub Actions and most CI services set `CI=true`).
fn in_ci() -> bool {
    std::env::var("CI")
        .is_ok_and(|v| !(v.is_empty() || v == "0" || v.eq_ignore_ascii_case("false")))
}

/// Copies the request totals of the whole session into a sync report.
fn update_api_stats(r: &mut SyncReport, s: &Session) {
    let stats = s.api_stats();
    r.api.requests = stats.requests;
    r.api.retries = stats.retries;
    r.api.total_api_time_ms = stats.total_api_time_ms;
}

fn require_complete(g: &GlobalArgs, status: CoverageStatus) -> i32 {
    if g.require_complete && !status.is_complete() {
        eprintln!(
            "error: data for the requested period is {} (--require-complete)",
            status.as_str()
        );
        exit::INCOMPLETE
    } else {
        exit::SUCCESS
    }
}

async fn report_cmd(
    g: &GlobalArgs,
    cmd: &Command,
    zone: Zone,
    kind: ReportKind,
    details: bool,
) -> Result<i32> {
    let (s, period) = prepared_session(g, period(g, cmd, &zone)?, zone.clone()).await?;
    let ds = s.dataset(Some(period))?;
    let ctx = QueryContext {
        period,
        zone,
        filter: filter(g),
        commit_basis: commit_basis(g),
        redact_private: g.format != Format::Table && !g.include_private,
        synced_at: s.last_synced_at()?,
    };
    let r = report::build(&ds, &ctx, kind);
    let out = match g.format {
        Format::Table => {
            render::report(&r, kind, details, style(g, std::io::stdout().is_terminal()))
        }
        Format::Json => to_json(&r)?,
        Format::Csv => csv_out::report_csv(&r, kind),
        Format::Markdown => render::report_markdown(&r, kind).ok_or_else(|| {
            AppError::InvalidArgs(
                "--format markdown is available for overview, daily, weekly, monthly, yearly and repos".into(),
            )
        })?,
    };
    print_stdout(&out)?;
    if s.is_offline() && !r.overall_coverage().is_complete() && g.format == Format::Table {
        eprintln!("note: offline mode; some of the period has not been synced");
    }
    Ok(require_complete(g, r.overall_coverage()))
}

async fn pr_cmd(g: &GlobalArgs, a: &cli::PrArgs, zone: Zone) -> Result<i32> {
    let Some((owner, name, number)) = ghusage_github::parse_pr_url(&a.url) else {
        return Err(AppError::InvalidArgs(format!(
            "not a pull request URL: {} (expected https://github.com/OWNER/REPO/pull/NUMBER)",
            ghusage_core::sanitize::sanitize_line(&a.url)
        )));
    };
    let mut types = Vec::new();
    for t in &a.event_types {
        if let Some(t) = EventType::parse(t.trim()) {
            types.push(t);
        } else {
            let valid: Vec<&str> = EventType::ALL.iter().map(|t| t.as_str()).collect();
            return Err(AppError::InvalidArgs(format!(
                "unknown event type {t}; valid types: {}",
                valid.join(", ")
            )));
        }
    }
    let mut s = Session::open(session_config(g)).await?;
    if let Some(reason) = &s.offline_state().reason {
        eprintln!(
            "warning: GitHub is not reachable ({}); showing cached data",
            clean(reason)
        );
    }
    let loaded = s.load_pr(&owner, &name, number).await;
    let loaded = match loaded {
        Ok(result) => {
            if let Some(error) = result.refresh_error.as_deref() {
                eprintln!(
                    "warning: could not refresh pull request, showing cached data: {}",
                    clean(&error)
                );
            }
            result
        }
        Err(e) => return Err(e),
    };
    let pr_id = loaded.pr_id;
    let Some(pr_id) = pr_id else {
        let hint = if s.is_offline() {
            "run without --offline to fetch it; if the repository was renamed, use its current URL"
        } else {
            "it could not be fetched from GitHub"
        };
        return Err(AppError::Other(format!(
            "pull request {owner}/{name}#{number} is not in the cache; {hint}"
        )));
    };
    let ds = s.pr_dataset(&pr_id)?;
    let redact = g.format != Format::Table && !g.include_private;
    let r = report::pr_report(&ds, &pr_id, &types, zone, redact)
        .ok_or_else(|| AppError::Other("pull request not found in cache".into()))?;
    let out = match g.format {
        Format::Table => render::pr(&r, a.timeline, style(g, std::io::stdout().is_terminal())),
        Format::Json => to_json(&r)?,
        Format::Csv => csv_out::pr_csv(&r),
        Format::Markdown => unreachable!("rejected before the command runs"),
    };
    print_stdout(&out)?;
    Ok(require_complete(g, r.coverage))
}

async fn commit_cmd(g: &GlobalArgs, a: &cli::CommitArgs, zone: Zone) -> Result<i32> {
    let Some(target) = ghusage_app::commit::CommitTarget::parse(&a.reference) else {
        return Err(AppError::InvalidArgs(format!(
            "not a commit: {} (expected OWNER/REPO@SHA or https://github.com/OWNER/REPO/commit/SHA)",
            ghusage_core::sanitize::sanitize_line(&a.reference)
        )));
    };
    let mut s = Session::open(session_config(g)).await?;
    if let Some(reason) = &s.offline_state().reason {
        eprintln!(
            "warning: GitHub is not reachable ({}); showing cached data",
            clean(reason)
        );
    }
    let loaded = ghusage_app::commit::load(&mut s, &target).await?;
    let redact = g.format != Format::Table && !g.include_private;
    let r = ghusage_app::commit::report(&loaded, a.patch, redact);
    let out = match g.format {
        Format::Json => to_json(&r)?,
        _ => render::commit(&r, zone, style(g, std::io::stdout().is_terminal())),
    };
    print_stdout(&out)?;
    Ok(exit::SUCCESS)
}

async fn sync_cmd(
    g: &GlobalArgs,
    cmd: &Command,
    zone: Zone,
    full: bool,
    diff_stats: bool,
) -> Result<i32> {
    if g.offline {
        return Err(AppError::InvalidArgs(
            "sync cannot run with --offline".into(),
        ));
    }
    let period = period(g, cmd, &zone)?;
    let mut s = Session::open_deferred(session_config(g)).await?;
    if let Some(error) = s.offline_error() {
        return Err(error);
    }
    let progress = progress::Progress::start(s.api_activity());
    let synced = s
        .sync(period, full, g.jobs as usize, &|p| {
            progress.update(p);
        })
        .await;
    let synced = match synced {
        Ok(mut r) if diff_stats => {
            let counts = s.fetch_line_counts(period, &|p| progress.update(p)).await;
            counts.map(|c| {
                r.diff_stats = Some(c);
                update_api_stats(&mut r, &s);
                r
            })
        }
        // Fetched by default; a failure only warns, the sync itself worked.
        Ok(mut r) if auto_diff_stats(g, &s)? => {
            match s.fetch_line_counts(period, &|p| progress.update(p)).await {
                Ok(c) => {
                    r.diff_stats = Some(c);
                    update_api_stats(&mut r, &s);
                    Ok(r)
                }
                Err(e @ (AppError::AuthRejected(_) | AppError::Auth(_))) => Err(e),
                Err(e) => {
                    eprintln!("warning: line counts not fetched: {}", clean(&e));
                    Ok(r)
                }
            }
        }
        other => other,
    };
    drop(progress);
    let mut r = synced?;
    let out = if g.format == Format::Json {
        if !g.include_private {
            let public = report::public_names(&s.dataset(Some(period))?);
            for f in &mut r.failures {
                *f = report::scrub_names(f, &public);
            }
            for c in &mut r.coverage {
                if let Some(reason) = &c.reason {
                    c.reason = Some(report::scrub_names(reason, &public));
                }
            }
        }
        to_json(&r)?
    } else {
        let mut o = format!(
            "Synced {} for {} → {}\n  commits found: {}\n  pull requests found: {} (fetched {}, unchanged {})\n  issues found: {} (fetched {}, unchanged {})\n  API requests: {} (retries {}, {} ms)\n",
            ghusage_core::sanitize::sanitize_line(&s.identity().login),
            zone.format(period.start, "%Y-%m-%d %H:%M"),
            zone.format(period.end, "%Y-%m-%d %H:%M"),
            r.commits_found,
            r.pull_requests_found,
            r.pull_requests_fetched,
            r.pull_requests_unchanged,
            r.issues_found,
            r.issues_fetched,
            r.issues_unchanged,
            r.api.requests,
            r.api.retries,
            r.api.total_api_time_ms,
        );
        let _ = writeln!(o, "  status: {}", r.status);
        if let Some(d) = &r.diff_stats {
            let _ = writeln!(
                o,
                "  line counts: {} commits (already known {}, fetched {}, unavailable {})",
                d.commits, d.already_known, d.fetched, d.unavailable
            );
        }
        for f in &r.failures {
            let _ = writeln!(
                o,
                "  incomplete: {}",
                ghusage_core::sanitize::sanitize_line(f)
            );
        }
        o
    };
    print_stdout(&out)?;
    let status = if r.is_complete() {
        CoverageStatus::Complete
    } else {
        CoverageStatus::Partial
    };
    if !r.is_complete() && !g.require_complete {
        // A partially failed sync is an API error unless the caller asked
        // for the stricter incompleteness code.
        return Ok(exit::API);
    }
    Ok(require_complete(g, status))
}

async fn tui_cmd(g: &GlobalArgs, cmd: &Command, zone: Zone) -> Result<i32> {
    let period = period(g, cmd, &zone)?;
    let mut cfg = session_config(g);
    let online = !g.offline;
    // The TUI reads from the cache and syncs in the background.
    cfg.offline = true;
    let s = match Session::open(cfg.clone()).await {
        Ok(s) => s,
        Err(_) if online => {
            // First run: resolve the subject online once.
            let mut online_cfg = cfg.clone();
            online_cfg.offline = false;
            Session::open(online_cfg).await?
        }
        Err(e) => return Err(e),
    };
    let opts = ghusage_tui::TuiOptions {
        period,
        zone,
        filter: filter(g),
        session: cfg,
        background_sync: online,
        concurrency: g.jobs as usize,
        commit_basis: commit_basis(g),
    };
    drop(s);
    match ghusage_tui::run(opts) {
        Ok(()) => Ok(exit::SUCCESS),
        // Cache and session errors keep their own message and exit code.
        Err(ghusage_tui::TuiError::App(e)) => Err(e),
        Err(e @ ghusage_tui::TuiError::Terminal(_)) => Err(AppError::Other(e.to_string())),
    }
}

async fn auth_status_cmd(g: &GlobalArgs) -> Result<i32> {
    let status = commands::auth_status(&session_config(g)).await?;
    if g.format == Format::Json {
        #[derive(serde::Serialize)]
        struct Out {
            authenticated: bool,
            source: String,
            login: String,
            id: String,
            scopes: Option<Vec<String>>,
            rate_limit: Option<commands::RateLimit>,
        }
        print_stdout(&to_json(&Out {
            authenticated: true,
            source: status.source,
            login: status.login,
            id: status.id,
            scopes: status.scopes,
            rate_limit: status.rate_limit,
        })?)?;
    } else {
        let mut o = format!(
            "Logged in as {} (credentials from {})\n",
            ghusage_core::sanitize::sanitize_line(&status.login),
            status.source
        );
        if let Some(scopes) = status.scopes {
            let _ = writeln!(o, "  token scopes: {}", scopes.join(", "));
        }
        if let Some(r) = status.rate_limit {
            let _ = writeln!(
                o,
                "  rate limit: core {}/{} (resets {}), search {}/{}, graphql {}/{}",
                r.core_remaining,
                r.core_limit,
                r.core_reset.strftime("%Y-%m-%dT%H:%M:%S%.f%:z"),
                r.search_remaining,
                r.search_limit,
                r.graphql_remaining,
                r.graphql_limit
            );
        }
        print_stdout(&o)?;
    }
    Ok(exit::SUCCESS)
}

fn cache_cmd(g: &GlobalArgs, c: CacheCommand, zone: Zone) -> Result<i32> {
    let cfg = session_config(g);
    match c {
        CacheCommand::Clear => {
            commands::clear_cache(&cfg)?;
            print_stdout("Cache cleared. Credentials were not changed.\n")?;
        }
        CacheCommand::Status => {
            let status = commands::cache_status(&cfg, g.user.as_deref(), Timestamp::now())?;
            if g.format == Format::Json {
                let v = serde_json::json!({
                    "path": status.path,
                    "file_size_bytes": status.file_size_bytes,
                    "schema_version": status.schema_version,
                    "account": status.account,
                    "repositories": status.repositories,
                    "pull_requests": status.pull_requests,
                    "issues": status.issues,
                    "events": status.events,
                    "last_sync": status.last_sync.as_ref().map(|r| serde_json::json!({
                        "started_at": r.started_at,
                        "finished_at": r.finished_at,
                        "status": r.status,
                        "message": r.message,
                    })),
                    "coverage_last_30_days": status.coverage_last_30_days,
                });
                print_stdout(&to_json(&v)?)?;
            } else {
                let style = style(g, std::io::stdout().is_terminal());
                let mut o = format!(
                    "Cache: {}\n  size: {:.1} MiB, schema version {}\n  repositories {}, pull requests {}, issues {}, events {}\n",
                    status
                        .path
                        .as_ref()
                        .map_or_else(|| "(in memory)".into(), |path| path.display().to_string()),
                    status.file_size_bytes as f64 / (1024.0 * 1024.0),
                    status.schema_version,
                    status.repositories,
                    status.pull_requests,
                    status.issues,
                    status.events
                );
                match &status.last_sync {
                    Some(r) => {
                        let _ = writeln!(
                            o,
                            "  last sync: {} ({})",
                            r.finished_at.map_or_else(
                                || "not finished".into(),
                                |t| zone.format(t, "%Y-%m-%d %H:%M")
                            ),
                            r.status
                        );
                    }
                    None => o.push_str("  last sync: never\n"),
                }
                if !status.coverage_last_30_days.is_empty() {
                    o.push_str("Last 30 days:\n");
                    o.push_str(&render::coverage_lines(
                        &status.coverage_last_30_days,
                        status.last_sync.as_ref().and_then(|r| r.finished_at),
                        &zone,
                        style,
                    ));
                }
                print_stdout(&o)?;
            }
        }
    }
    Ok(exit::SUCCESS)
}

async fn doctor_cmd(g: &GlobalArgs, zone: Zone) -> Result<i32> {
    let report = commands::doctor(&session_config(g), zone).await?;
    if g.format == Format::Json {
        let v = serde_json::json!({
            "ok": report.ok,
            "checks": report.checks.iter().map(|check| serde_json::json!({
                "ok": check.state != commands::DoctorState::Failed,
                "message": check.message,
            })).collect::<Vec<_>>(),
        });
        print_stdout(&to_json(&v)?)?;
    } else {
        let mut o = String::new();
        for check in &report.checks {
            let label = match check.state {
                commands::DoctorState::Passed | commands::DoctorState::Skipped => "ok  ",
                commands::DoctorState::Failed => "FAIL",
            };
            let _ = writeln!(o, "{label} {}", check.message);
        }
        print_stdout(&o)?;
    }
    Ok(report.exit_code)
}
