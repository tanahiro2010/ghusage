//! Report computation off the UI thread. A worker owns its own read-only
//! [`DataSource`] and answers requests with reports built by the shared
//! report functions (`ghusage_app::report`), the same ones the CLI uses.

use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, Sender};

use ghusage_app::report::{self, IssueReport, PrReport, QueryContext, Report, ReportKind};
use ghusage_core::metrics::Dataset;
use ghusage_core::time::Period;

use crate::app::{CacheKey, IssueKey};
use crate::source::{DataSource, SystemInfo};

/// What to compute.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Job {
    Report(CacheKey),
    Pr(String),
    Issue(IssueKey),
    System,
}

/// Which shared report builder produces a report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Build {
    Kind(ReportKind),
    /// Overview plus the most recently active pull requests.
    Dashboard,
    /// Every listing, for one day or one repository.
    Detail,
}

#[derive(Debug, Clone)]
pub(crate) struct Request {
    pub job: Job,
    /// Bumped by the UI whenever cached data must be re-read; results of
    /// older generations are stale.
    pub generation: u64,
    /// Requested for the view on screen (as opposed to warm-up).
    pub foreground: bool,
    /// The period selected in the UI; the dataset read covers it. A day
    /// detail narrows `ctx.period` to one day inside it.
    pub view: Period,
    pub ctx: QueryContext,
    pub build: Build,
}

#[derive(Debug)]
pub(crate) enum Outcome {
    Report(Result<Box<Report>, String>),
    Pr(Result<Option<Box<PrReport>>, String>),
    Issue(Result<Option<Box<IssueReport>>, String>),
    System(Box<SystemInfo>),
    /// Skipped because a newer request superseded it.
    Dropped,
}

#[derive(Debug)]
pub(crate) struct Response {
    pub job: Job,
    pub generation: u64,
    pub outcome: Outcome,
    pub loaded_dataset: bool,
}

/// Computes reports from a data source, keeping the last dataset read.
pub(crate) struct Worker {
    source: Box<dyn DataSource + Send>,
    dataset: Option<(Period, u64, Dataset)>,
}

impl Worker {
    pub fn new(source: Box<dyn DataSource + Send>) -> Self {
        Self {
            source,
            dataset: None,
        }
    }

    /// The dataset of `period`, read from the source unless already read
    /// for this period and generation.
    fn load(&mut self, period: Period, generation: u64) -> (Result<&Dataset, String>, bool) {
        let fresh = self
            .dataset
            .as_ref()
            .is_some_and(|(p, g, _)| *p == period && *g == generation);
        if fresh {
            return (Ok(&self.dataset.as_ref().expect("checked above").2), false);
        }
        self.dataset = None;
        match self.source.dataset(period) {
            Ok(ds) => {
                let (_, _, ds) = self.dataset.insert((period, generation, ds));
                (Ok(ds), true)
            }
            Err(e) => (Err(e), true),
        }
    }

    pub fn handle(&mut self, req: Request) -> Response {
        let mut loaded_dataset = false;
        let outcome = match &req.job {
            Job::Report(_) => {
                let (ds, loaded) = self.load(req.view, req.generation);
                loaded_dataset = loaded;
                Outcome::Report(ds.map(|ds| {
                    Box::new(match req.build {
                        Build::Kind(kind) => report::build(ds, &req.ctx, kind),
                        Build::Dashboard => report::build_dashboard(ds, &req.ctx),
                        Build::Detail => report::build_detail(ds, &req.ctx),
                    })
                }))
            }
            Job::Issue(k) => {
                let (ds, loaded) = self.load(req.view, req.generation);
                loaded_dataset = loaded;
                Outcome::Issue(ds.map(|ds| report::issue_report(ds, &req.ctx, &k.id).map(Box::new)))
            }
            Job::Pr(id) => {
                loaded_dataset = true;
                Outcome::Pr(
                    self.source.pr_dataset(id).map(|ds| {
                        report::pr_report(&ds, id, &[], req.ctx.zone, false).map(Box::new)
                    }),
                )
            }
            Job::System => Outcome::System(Box::new(self.source.system_info())),
        };
        Response {
            job: req.job,
            generation: req.generation,
            outcome,
            loaded_dataset,
        }
    }
}

fn view_of(r: &Request) -> Option<(Period, crate::app::FilterPreset)> {
    match &r.job {
        Job::Report(k) => Some((r.view, k.filter)),
        _ => None,
    }
}

/// Removes requests that a newer one made pointless and returns them.
fn prune(queue: &mut VecDeque<Request>) -> Vec<Request> {
    let Some(latest_gen) = queue.iter().map(|r| r.generation).max() else {
        return Vec::new();
    };
    // The view the user is looking at now: the newest report request.
    let view = queue
        .iter()
        .rev()
        .filter(|r| r.foreground)
        .find_map(view_of)
        .or_else(|| queue.iter().rev().find_map(view_of));
    let mut dropped = Vec::new();
    let mut keep = VecDeque::with_capacity(queue.len());
    for r in queue.drain(..) {
        let stale = r.generation < latest_gen;
        let other_view = matches!(r.job, Job::Report(_)) && view.is_some() && view_of(&r) != view;
        if stale || other_view {
            dropped.push(r);
        } else {
            keep.push_back(r);
        }
    }
    *queue = keep;
    dropped
}

/// Picks the next request: the newest foreground one, else the oldest
/// warm-up one.
fn next(queue: &mut VecDeque<Request>) -> Option<Request> {
    match queue.iter().rposition(|r| r.foreground) {
        Some(i) => queue.remove(i),
        None => queue.pop_front(),
    }
}

/// Starts a worker thread. It stops when the request sender is dropped.
pub(crate) fn spawn(worker: Worker) -> std::io::Result<(Sender<Request>, Receiver<Response>)> {
    let (req_tx, req_rx) = mpsc::channel::<Request>();
    let (resp_tx, resp_rx) = mpsc::channel::<Response>();
    std::thread::Builder::new()
        .name("ghusage-compute".into())
        .spawn(move || {
            crate::term::set_quiet_panics(true);
            let mut worker = worker;
            let mut queue = VecDeque::new();
            loop {
                if queue.is_empty() {
                    match req_rx.recv() {
                        Ok(r) => queue.push_back(r),
                        Err(_) => return,
                    }
                }
                queue.extend(req_rx.try_iter());
                for r in prune(&mut queue) {
                    let resp = Response {
                        job: r.job,
                        generation: r.generation,
                        outcome: Outcome::Dropped,
                        loaded_dataset: false,
                    };
                    if resp_tx.send(resp).is_err() {
                        return;
                    }
                }
                let Some(req) = next(&mut queue) else {
                    continue;
                };
                let (job, generation) = (req.job.clone(), req.generation);
                let resp =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| worker.handle(req)))
                        .unwrap_or_else(|_| {
                            // Start from a clean state after an internal error.
                            worker.dataset = None;
                            let msg = "internal error while computing this view".to_string();
                            Response {
                                outcome: match &job {
                                    Job::Pr(_) => Outcome::Pr(Err(msg)),
                                    Job::Issue(_) => Outcome::Issue(Err(msg)),
                                    Job::Report(_) => Outcome::Report(Err(msg)),
                                    Job::System => Outcome::System(Box::new(SystemInfo {
                                        error: Some(msg),
                                        ..SystemInfo::default()
                                    })),
                                },
                                job,
                                generation,
                                loaded_dataset: false,
                            }
                        });
                if resp_tx.send(resp).is_err() {
                    return;
                }
            }
        })?;
    Ok((req_tx, resp_rx))
}
