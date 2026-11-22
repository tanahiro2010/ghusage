mod cli;
mod progress;
mod render;

use std::io::{IsTerminal, Write};
use std::process::ExitCode;

use chrono::Utc;
use clap::Parser;
use ghusage_app::report::{self, QueryContext, ReportKind};
use ghusage_app::session::{self, Session, SessionConfig};
use ghusage_app::{AppError, Result, csv_out, exit};
use ghusage_core::coverage::CoverageStatus;
use ghusage_core::metrics::{
    CommitDateBasis, Filter, Ownership, Participation, PrSort, VisibilityFilter,
};
use ghusage_core::model::{EventType, PrState};
use ghusage_core::time::{Granularity, Period, Zone, resolve_period};
use ghusage_github::ClientConfig;

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
            let _ = e.print();
            return ExitCode::from(code as u8);
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
            return ExitCode::from(exit::GENERAL as u8);
        }
    };
    match runtime.block_on(run(cli)) {
        Ok(code) => ExitCode::from(code as u8),
        Err(e) => {
            eprintln!("error: {}", clean(&e));
            ExitCode::from(e.exit_code() as u8)
        }
    }
}

fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_env("GHUSAGE_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .try_init();
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
        client: client_config(g),
    }
}

/// Client settings. `GHUSAGE_API_URL` and `GHUSAGE_GRAPHQL_URL` point the
/// client at another API endpoint (for example GitHub Enterprise Server).
fn client_config(g: &GlobalArgs) -> ClientConfig {
    let mut c = ClientConfig {
        concurrency: g.concurrency as usize,
        ..ClientConfig::default()
    };
    if let Some(url) = std::env::var("GHUSAGE_API_URL")
        .ok()
        .filter(|v| !v.is_empty())
    {
        c.api_base = url.trim_end_matches('/').to_string();
        // GitHub Enterprise Server serves REST at /api/v3 and GraphQL at
        // /api/graphql; github.com-style bases append /graphql.
        c.graphql_url = match c.api_base.strip_suffix("/api/v3") {
            Some(host) => format!("{host}/api/graphql"),
            None => format!("{}/graphql", c.api_base),
        };
    }
    if let Some(url) = std::env::var("GHUSAGE_GRAPHQL_URL")
        .ok()
        .filter(|v| !v.is_empty())
    {
        c.graphql_url = url;
    }
    if let Some(ms) = std::env::var("GHUSAGE_BACKOFF_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        c.backoff_base = std::time::Duration::from_millis(ms);
    }
    if let Some(ms) = std::env::var("GHUSAGE_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|ms| *ms > 0)
    {
        c.request_timeout = std::time::Duration::from_millis(ms);
    }
    c
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

fn commit_basis(g: &GlobalArgs) -> CommitDateBasis {
    match g.commit_date {
        cli::CommitDateArg::Committer => CommitDateBasis::Committer,
        cli::CommitDateArg::Author => CommitDateBasis::Author,
    }
}

fn default_since(cmd: &Command) -> &'static str {
    match cmd {
        Command::Weekly => "8w",
        Command::Monthly => "6m",
        Command::Yearly => "3y",
        Command::Sync(_) | Command::Tui => "30d",
        _ => "7d",
    }
}

fn period(g: &GlobalArgs, cmd: &Command, zone: Zone) -> Result<Period> {
    Ok(resolve_period(
        g.since.as_deref(),
        g.until.as_deref(),
        default_since(cmd),
        zone,
        Utc::now(),
    )?)
}

/// Text from errors, which may quote GitHub responses, made safe for the
/// terminal.
fn clean(e: &dyn std::fmt::Display) -> String {
    ghusage_core::sanitize::sanitize_line(&e.to_string())
}

fn print_stdout(s: &str) -> Result<()> {
    let mut out = std::io::stdout().lock();
    match out.write_all(s.as_bytes()).and_then(|_| out.flush()) {
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
fn command_name(cmd: &Command) -> &'static str {
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
        Command::Sync(ref a) => sync_cmd(&g, &cmd, zone, a.full).await,
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
    let mut s = Session::open(session_config(g)).await?;
    if let Some(reason) = &s.offline_reason {
        eprintln!(
            "warning: GitHub is not reachable ({}); showing cached data",
            clean(reason)
        );
    }
    if !s.is_offline() && s.needs_sync(period)? {
        let now = Utc::now();
        let max_age = chrono::Duration::from_std(g.max_age).unwrap_or(chrono::Duration::MAX);
        if let Some(until) = s.synced_until(period)?
            && until < period.end
            && now - until <= max_age
        {
            let mins = (now - until).num_minutes();
            eprintln!(
                "note: showing data synced {} ago (up to {}); use --max-age 0 to fetch the latest",
                if mins < 1 {
                    "less than a minute".to_string()
                } else {
                    format!("{mins} min")
                },
                zone.format(until, "%Y-%m-%d %H:%M")
            );
            return Ok((s, Period::new(period.start, until)?));
        }
        let progress = progress::Progress::start(s.client.as_ref().map(|c| c.activity()));
        let synced = s
            .sync(period, false, g.concurrency as usize, &|p| {
                progress.update(p)
            })
            .await;
        drop(progress);
        match synced {
            Ok(r) => {
                for f in r.failures.iter().take(5) {
                    eprintln!("warning: {}", ghusage_core::sanitize::sanitize_line(f));
                }
            }
            Err(e @ (AppError::AuthRejected(_) | AppError::Auth(_))) => return Err(e),
            Err(e) => {
                eprintln!("warning: sync failed, showing cached data: {}", clean(&e));
            }
        }
    }
    Ok((s, period))
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
    let (s, period) = prepared_session(g, period(g, cmd, zone)?, zone).await?;
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
        match EventType::parse(t.trim()) {
            Some(t) => types.push(t),
            None => {
                let valid: Vec<&str> = EventType::ALL.iter().map(|t| t.as_str()).collect();
                return Err(AppError::InvalidArgs(format!(
                    "unknown event type {t}; valid types: {}",
                    valid.join(", ")
                )));
            }
        }
    }
    let mut s = Session::open(session_config(g)).await?;
    if let Some(reason) = &s.offline_reason {
        eprintln!(
            "warning: GitHub is not reachable ({}); showing cached data",
            clean(reason)
        );
    }
    // A single pull request is always refreshed when online. GitHub resolves
    // old URLs of renamed repositories, so the fetched id is authoritative.
    let mut fetched_id = None;
    if let Some(client) = &s.client {
        let bundle = client
            .fetch_pull_request(&owner, &name, number)
            .await
            .map_err(AppError::from);
        match bundle {
            Ok(b) => {
                fetched_id = Some(b.pull_request.id.clone());
                let involved = ghusage_app::sync::involves(&s.subject, &b);
                let mut batch = ghusage_app::sync::pr_batch(&s.subject, b);
                if !involved {
                    // Viewing someone else's pull request must not add it
                    // to your own activity.
                    batch.account =
                        format!("{}{}", s.subject.login, ghusage_app::sync::VIEWED_SUFFIX);
                    batch
                        .events
                        .retain(|e| e.event_type != EventType::CommitAuthored);
                }
                s.store.apply(&batch)?;
            }
            Err(e @ AppError::AuthRejected(_)) => return Err(e),
            Err(e) => {
                if s.store.find_pr(&owner, &name, number)?.is_none() {
                    return Err(e);
                }
                eprintln!(
                    "warning: could not refresh pull request, showing cached data: {}",
                    clean(&e)
                );
            }
        }
    }
    let pr_id = match fetched_id {
        Some(id) => Some(id),
        None => s.store.find_pr(&owner, &name, number)?,
    };
    let Some(pr_id) = pr_id else {
        let hint = if s.client.is_none() {
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
    if let Some(reason) = &s.offline_reason {
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

async fn sync_cmd(g: &GlobalArgs, cmd: &Command, zone: Zone, full: bool) -> Result<i32> {
    if g.offline {
        return Err(AppError::InvalidArgs(
            "sync cannot run with --offline".into(),
        ));
    }
    let period = period(g, cmd, zone)?;
    let mut s = Session::open(session_config(g)).await?;
    if let Some(reason) = &s.offline_reason {
        return Err(AppError::Api(ghusage_github::ApiError::Network(
            reason.clone(),
        )));
    }
    let progress = progress::Progress::start(s.client.as_ref().map(|c| c.activity()));
    let synced = s
        .sync(period, full, g.concurrency as usize, &|p| {
            progress.update(p)
        })
        .await;
    drop(progress);
    let mut r = synced?;
    let out = match g.format {
        Format::Json => {
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
        }
        _ => {
            let mut o = format!(
                "Synced {} for {} → {}\n  commits found: {}\n  pull requests found: {} (fetched {}, unchanged {})\n  issues found: {} (fetched {}, unchanged {})\n  API requests: {} (retries {}, {} ms)\n",
                ghusage_core::sanitize::sanitize_line(&s.subject.login),
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
            o.push_str(&format!("  status: {}\n", r.status));
            for f in &r.failures {
                o.push_str(&format!(
                    "  incomplete: {}\n",
                    ghusage_core::sanitize::sanitize_line(f)
                ));
            }
            o
        }
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
    let period = period(g, cmd, zone)?;
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
        concurrency: g.concurrency as usize,
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
    let creds = session::credentials(g.token_file.as_deref())?;
    let source = creds.source.describe();
    let client = ghusage_github::GitHubClient::new(Some(creds), session_config(g).client)?;
    let viewer = client.viewer().await?;
    let rate = client.rate_limit().await.ok();
    match g.format {
        Format::Json => {
            #[derive(serde::Serialize)]
            struct Out {
                authenticated: bool,
                source: String,
                login: String,
                id: String,
                scopes: Option<Vec<String>>,
                rate_limit: Option<ghusage_github::RateLimitInfo>,
            }
            print_stdout(&to_json(&Out {
                authenticated: true,
                source,
                login: viewer.login,
                id: viewer.id,
                scopes: viewer.scopes,
                rate_limit: rate,
            })?)?;
        }
        _ => {
            let mut o = format!(
                "Logged in as {} (credentials from {})\n",
                ghusage_core::sanitize::sanitize_line(&viewer.login),
                source
            );
            if let Some(scopes) = viewer.scopes {
                o.push_str(&format!("  token scopes: {}\n", scopes.join(", ")));
            }
            if let Some(r) = rate {
                o.push_str(&format!(
                    "  rate limit: core {}/{} (resets {}), search {}/{}, graphql {}/{}\n",
                    r.core_remaining,
                    r.core_limit,
                    r.core_reset.to_rfc3339(),
                    r.search_remaining,
                    r.search_limit,
                    r.graphql_remaining,
                    r.graphql_limit
                ));
            }
            print_stdout(&o)?;
        }
    }
    Ok(exit::SUCCESS)
}

fn cache_cmd(g: &GlobalArgs, c: CacheCommand, zone: Zone) -> Result<i32> {
    let cfg = session_config(g);
    let (mut store, dir) = session::open_store(&cfg)?;
    match c {
        CacheCommand::Clear => {
            // Never clear underneath a running sync.
            let _lock = match &dir {
                Some(d) => Some(ghusage_storage::SyncLock::acquire(d)?),
                None => None,
            };
            store.clear()?;
            print_stdout("Cache cleared. Credentials were not changed.\n")?;
            Ok(exit::SUCCESS)
        }
        CacheCommand::Status => {
            let stats = store.stats()?;
            let login = store.meta(ghusage_app::sync::META_VIEWER_LOGIN)?;
            let account = g.user.clone().or(login);
            let last = match &account {
                Some(a) => store.last_sync_run(a)?,
                None => None,
            };
            let now = Utc::now();
            let p30 = Period::new(now - chrono::Duration::days(30), now)?;
            let coverage = match &account {
                Some(a) => {
                    let recs = store.coverage(a)?;
                    ghusage_core::coverage::DataKind::ALL
                        .iter()
                        .map(|k| ghusage_core::coverage::summarize(&recs, *k, p30.start, p30.end))
                        .collect()
                }
                None => Vec::new(),
            };
            match g.format {
                Format::Json => {
                    let v = serde_json::json!({
                        "path": dir.map(|d| ghusage_app::paths::db_path(&d)),
                        "file_size_bytes": stats.file_size_bytes,
                        "schema_version": stats.schema_version,
                        "account": account,
                        "repositories": stats.repositories,
                        "pull_requests": stats.pull_requests,
                        "issues": stats.issues,
                        "events": stats.events,
                        "last_sync": last.as_ref().map(|r| serde_json::json!({
                            "started_at": r.started_at,
                            "finished_at": r.finished_at,
                            "status": r.status,
                            "message": r.message,
                        })),
                        "coverage_last_30_days": coverage,
                    });
                    print_stdout(&to_json(&v)?)?;
                }
                _ => {
                    let style = style(g, std::io::stdout().is_terminal());
                    let mut o = format!(
                        "Cache: {}\n  size: {:.1} MiB, schema version {}\n  repositories {}, pull requests {}, issues {}, events {}\n",
                        dir.map(|d| ghusage_app::paths::db_path(&d).display().to_string())
                            .unwrap_or_else(|| "(in memory)".into()),
                        stats.file_size_bytes as f64 / (1024.0 * 1024.0),
                        stats.schema_version,
                        stats.repositories,
                        stats.pull_requests,
                        stats.issues,
                        stats.events
                    );
                    match &last {
                        Some(r) => o.push_str(&format!(
                            "  last sync: {} ({})\n",
                            r.finished_at
                                .map(|t| zone.format(t, "%Y-%m-%d %H:%M"))
                                .unwrap_or_else(|| "not finished".into()),
                            r.status
                        )),
                        None => o.push_str("  last sync: never\n"),
                    }
                    if !coverage.is_empty() {
                        o.push_str("Last 30 days:\n");
                        o.push_str(&render::coverage_lines(
                            &coverage,
                            last.and_then(|r| r.finished_at),
                            zone,
                            style,
                        ));
                    }
                    print_stdout(&o)?;
                }
            }
            Ok(exit::SUCCESS)
        }
    }
}

async fn doctor_cmd(g: &GlobalArgs, zone: Zone) -> Result<i32> {
    let mut lines: Vec<(bool, String)> = Vec::new();
    let cfg = session_config(g);
    match session::open_store(&cfg) {
        Ok((store, dir)) => {
            let path = dir
                .map(|d| ghusage_app::paths::db_path(&d).display().to_string())
                .unwrap_or_else(|| "(in memory)".into());
            lines.push((true, format!("cache opened: {path}")));
            match store.integrity_check() {
                Ok(p) if p.is_empty() => lines.push((true, "cache integrity: ok".into())),
                Ok(p) => lines.push((false, format!("cache integrity problems: {}", p.join("; ")))),
                Err(e) => lines.push((false, format!("cache integrity check failed: {e}"))),
            }
            if let Ok(Some(login)) = store.meta(ghusage_app::sync::META_VIEWER_LOGIN)
                && let Ok(Some(r)) = store.last_sync_run(&login)
            {
                lines.push((
                    r.status != "failed",
                    format!(
                        "last sync: {} ({})",
                        r.finished_at
                            .map(|t| zone.format(t, "%Y-%m-%d %H:%M"))
                            .unwrap_or_else(|| "not finished".into()),
                        r.status
                    ),
                ));
            }
        }
        Err(e) => lines.push((false, format!("cache: {e}"))),
    }
    if g.offline {
        lines.push((true, "offline: GitHub checks skipped".into()));
    } else {
        match session::credentials(g.token_file.as_deref()) {
            Ok(c) => {
                lines.push((true, format!("credentials found: {}", c.source.describe())));
                match ghusage_github::GitHubClient::new(Some(c), cfg.client.clone()) {
                    Ok(client) => {
                        match client.viewer().await {
                            Ok(v) => lines.push((
                                true,
                                format!(
                                    "authenticated as {}",
                                    ghusage_core::sanitize::sanitize_line(&v.login)
                                ),
                            )),
                            Err(e) => lines.push((false, format!("authentication: {e}"))),
                        }
                        match client.rate_limit().await {
                            Ok(r) => lines.push((
                                r.core_remaining > 0,
                                format!(
                                    "rate limit: core {}/{}, search {}/{}, graphql {}/{}",
                                    r.core_remaining,
                                    r.core_limit,
                                    r.search_remaining,
                                    r.search_limit,
                                    r.graphql_remaining,
                                    r.graphql_limit
                                ),
                            )),
                            Err(e) => lines.push((false, format!("rate limit: {e}"))),
                        }
                    }
                    Err(e) => lines.push((false, format!("client: {e}"))),
                }
            }
            Err(e) => lines.push((false, format!("credentials: {e}"))),
        }
    }
    let ok = lines.iter().all(|(ok, _)| *ok);
    match g.format {
        Format::Json => {
            let v = serde_json::json!({
                "ok": ok,
                "checks": lines.iter().map(|(ok, m)| serde_json::json!({"ok": ok, "message": m})).collect::<Vec<_>>(),
            });
            print_stdout(&to_json(&v)?)?;
        }
        _ => {
            let mut o = String::new();
            for (ok, m) in &lines {
                o.push_str(&format!("{} {m}\n", if *ok { "ok  " } else { "FAIL" }));
            }
            print_stdout(&o)?;
        }
    }
    Ok(if ok { exit::SUCCESS } else { exit::GENERAL })
}
