//! What agents ask of the project open in Kineto, and what waits for the
//! director's word (ADR-0011).
//!
//! An agent's command runs on the open session's worker, as the
//! director's buttons do. One that pays per call waits until the director
//! allows it in Kineto's window; so does an agent that asks to connect from
//! the internet. Each command is remembered for a while under a number,
//! which `get_request` answers about.

use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{Condvar, Mutex, MutexGuard},
    time::{Duration, Instant},
};

use kineto_core::{
    generation::ImageProviderKind,
    session::{AgentCommand, AgentTicket, SessionRemote, SubmitError},
    video::VideoProviderKind,
};
use serde::Serialize;
use serde_json::{Value, json};

/// How long a tool call waits for the director's word, or for its command
/// to end, before it answers anyway.
pub const ANSWER_WAIT: Duration = Duration::from_secs(45);
/// How long a request waits for the director before it lapses.
const ASK_LIFETIME: Duration = Duration::from_secs(30 * 60);
/// How many requests are remembered.
const KEPT: usize = 200;

/// What a command costs, as far as Kineto knows before it is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Charge {
    /// Nothing is paid per call: an edit, the free demo, a local model, or
    /// an agent the director's subscription covers.
    Free,
    /// Paid per call, with what is known of the price.
    Paid(String),
}

/// A command an agent asks for.
#[derive(Debug, Clone, PartialEq)]
pub struct Work {
    pub command: AgentCommand,
    /// What it does, for the director: "make keyframes of s01/sh02".
    pub what: String,
    pub charge: Charge,
}

/// What waits for the director, as Kineto's window shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Waiting {
    pub id: u64,
    /// `run` for a command, `connect` for an agent asking to connect.
    pub kind: &'static str,
    pub agent: String,
    pub what: String,
    /// What it costs, for a paid command.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<String>,
}

/// Where an agent's request to connect stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Waiting,
    Allowed,
    Declined,
    /// Lapsed, or never asked.
    Gone,
}

#[derive(Debug)]
enum State {
    Waiting,
    Declined,
    Allowed,
    Running(AgentTicket),
    Ended(Result<(), String>),
    Lapsed,
}

#[derive(Debug)]
enum Asked {
    /// A command, until it is sent.
    Run(Option<AgentCommand>),
    Connect,
}

#[derive(Debug)]
struct Entry {
    id: u64,
    agent: String,
    what: String,
    cost: Option<String>,
    asked: Asked,
    state: State,
    at: Instant,
}

impl Entry {
    /// Lapse when it has waited too long; take in how it ended when it has.
    fn tend(&mut self, now: Instant) {
        match &mut self.state {
            State::Waiting if now.duration_since(self.at) > ASK_LIFETIME => {
                self.state = State::Lapsed;
            }
            State::Running(ticket) => {
                if let Some(ended) = ticket.outcome(Duration::ZERO) {
                    self.state = State::Ended(ended);
                }
            }
            _ => {}
        }
    }
}

#[derive(Debug, Default)]
struct Book {
    next: u64,
    entries: VecDeque<Entry>,
}

impl Book {
    fn find(&mut self, id: u64) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|entry| entry.id == id)
    }

    fn keep(&mut self, mut entry: Entry) -> u64 {
        self.next += 1;
        entry.id = self.next;
        self.entries.push_back(entry);
        // The oldest that no longer waits go first.
        while self.entries.len() > KEPT {
            match self
                .entries
                .iter()
                .position(|entry| !matches!(entry.state, State::Waiting))
            {
                Some(index) => {
                    self.entries.remove(index);
                }
                None => {
                    self.entries.pop_front();
                }
            }
        }
        self.next
    }
}

/// The project open in Kineto, as agents reach it, and what they asked.
#[derive(Debug)]
pub struct Desk {
    /// How long a tool call waits before it answers anyway.
    answer_wait: Duration,
    /// The project Kineto opened last, which the reading tools read.
    project: Mutex<Option<PathBuf>>,
    /// Its session while it is open, which writes go through.
    session: Mutex<Option<SessionRemote>>,
    book: Mutex<Book>,
    decided: Condvar,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn refusal(error: SubmitError) -> String {
    match error {
        SubmitError::InvalidShot => "There is no such shot among the shots in play.".to_owned(),
        SubmitError::Closed => "Kineto closed the project.".to_owned(),
    }
}

/// `micros` in dollars, as the director reads a price.
pub fn dollars(micros: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let dollars = micros as f64 / 1_000_000.0;
    if dollars < 0.01 {
        "under $0.01".to_owned()
    } else {
        format!("about ${dollars:.2}")
    }
}

impl Default for Desk {
    fn default() -> Self {
        Self::with_answer_wait(ANSWER_WAIT)
    }
}

impl Desk {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A desk whose tool calls answer after `wait` at most, for tests.
    #[must_use]
    pub fn with_answer_wait(wait: Duration) -> Self {
        Self {
            answer_wait: wait,
            project: Mutex::default(),
            session: Mutex::default(),
            book: Mutex::default(),
            decided: Condvar::new(),
        }
    }

    /// Kineto opened the project at `path`, whose session agents write
    /// through when there is one.
    pub fn open(&self, path: &Path, session: Option<SessionRemote>) {
        *lock(&self.project) = Some(path.to_path_buf());
        *lock(&self.session) = session;
    }

    /// Kineto closed its project: nothing is written until one is open,
    /// and commands waiting for the director lapse.
    pub fn close(&self) {
        lock(&self.session).take();
        let mut book = lock(&self.book);
        for entry in &mut book.entries {
            if matches!(entry.asked, Asked::Run(_)) && matches!(entry.state, State::Waiting) {
                entry.state = State::Lapsed;
            }
        }
        drop(book);
        self.decided.notify_all();
    }

    /// The project the reading tools read.
    #[must_use]
    pub fn project(&self) -> Option<PathBuf> {
        lock(&self.project).clone()
    }

    /// The open session, which writes go through.
    pub fn session(&self) -> Result<SessionRemote, String> {
        lock(&self.session)
            .clone()
            .filter(|session| !session.is_closed())
            .ok_or_else(|| "No project is open in Kineto: open one there first.".to_owned())
    }

    /// What waits for the director, oldest first.
    #[must_use]
    pub fn waiting(&self) -> Vec<Waiting> {
        let now = Instant::now();
        let mut book = lock(&self.book);
        book.entries
            .iter_mut()
            .filter_map(|entry| {
                entry.tend(now);
                matches!(entry.state, State::Waiting).then(|| Waiting {
                    id: entry.id,
                    kind: match entry.asked {
                        Asked::Run(_) => "run",
                        Asked::Connect => "connect",
                    },
                    agent: entry.agent.clone(),
                    what: entry.what.clone(),
                    cost: entry.cost.clone(),
                })
            })
            .collect()
    }

    /// The director's word on request `id`. Allowing a command sends it.
    /// Returns whether `id` was waiting.
    pub fn decide(&self, id: u64, allow: bool) -> bool {
        let mut book = lock(&self.book);
        let Some(entry) = book.find(id) else {
            return false;
        };
        entry.tend(Instant::now());
        if !matches!(entry.state, State::Waiting) {
            return false;
        }
        entry.state = match (&mut entry.asked, allow) {
            (_, false) => State::Declined,
            (Asked::Connect, true) => State::Allowed,
            (Asked::Run(command), true) => match command.take() {
                Some(command) => {
                    let label = format!("{}: {}", entry.agent, entry.what);
                    match self
                        .session()
                        .and_then(|session| session.submit(command, &label).map_err(refusal))
                    {
                        Ok(ticket) => State::Running(ticket),
                        Err(why) => State::Ended(Err(why)),
                    }
                }
                None => State::Ended(Err("It was sent already.".to_owned())),
            },
        };
        drop(book);
        self.decided.notify_all();
        true
    }

    /// Run `work` for `agent`: at once when it is free, after the
    /// director allows it when it is paid. Answers when it ended, or once
    /// [`ANSWER_WAIT`] has passed.
    pub fn run(&self, agent: &str, work: Work) -> Result<Value, String> {
        let session = self.session()?;
        match work.charge {
            Charge::Free => {
                let label = format!("{agent}: {}", work.what);
                let mut ticket = session.submit(work.command, &label).map_err(refusal)?;
                let ended = ticket.outcome(self.answer_wait);
                let id = lock(&self.book).keep(Entry {
                    id: 0,
                    agent: agent.to_owned(),
                    what: work.what,
                    cost: None,
                    asked: Asked::Run(None),
                    state: match &ended {
                        Some(ended) => State::Ended(ended.clone()),
                        None => State::Running(ticket),
                    },
                    at: Instant::now(),
                });
                match ended {
                    Some(Err(why)) => Err(why),
                    _ => self.request(id),
                }
            }
            Charge::Paid(cost) => {
                let id = lock(&self.book).keep(Entry {
                    id: 0,
                    agent: agent.to_owned(),
                    what: work.what,
                    cost: Some(cost),
                    asked: Asked::Run(Some(work.command)),
                    state: State::Waiting,
                    at: Instant::now(),
                });
                self.await_word(id, self.answer_wait);
                let answer = self.request(id)?;
                if answer["status"] == "declined" {
                    Err("The director declined it in Kineto.".to_owned())
                } else {
                    Ok(answer)
                }
            }
        }
    }

    /// Wait at most `wait` while request `id` waits for the director.
    fn await_word(&self, id: u64, wait: Duration) {
        let deadline = Instant::now() + wait;
        let mut book = lock(&self.book);
        loop {
            let now = Instant::now();
            let waiting = book.find(id).is_some_and(|entry| {
                entry.tend(now);
                matches!(entry.state, State::Waiting)
            });
            let left = deadline.saturating_duration_since(now);
            if !waiting || left.is_zero() {
                return;
            }
            book = self
                .decided
                .wait_timeout(book, left)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
    }

    /// Where request `id` stands, as `get_request` tells it.
    pub fn request(&self, id: u64) -> Result<Value, String> {
        let mut book = lock(&self.book);
        let entry = book
            .find(id)
            .ok_or_else(|| format!("Kineto knows no request {id}: it may have restarted."))?;
        entry.tend(Instant::now());
        let (status, why, note) = match &entry.state {
            State::Waiting => (
                "awaiting_director",
                None,
                Some("The director is asked in Kineto's window. Call get_request later."),
            ),
            State::Declined => ("declined", None, None),
            State::Allowed => ("allowed", None, None),
            State::Lapsed => (
                "lapsed",
                None,
                Some("The director did not answer, or Kineto closed the project; it was not sent."),
            ),
            State::Running(_) => (
                "running",
                None,
                Some(
                    "It runs in Kineto. Call get_request later; get_project and get_shot show the result.",
                ),
            ),
            State::Ended(Ok(())) => ("done", None, None),
            State::Ended(Err(why)) => ("failed", Some(why.clone()), None),
        };
        let mut answer = json!({"request": id, "what": entry.what, "status": status});
        if let Some(cost) = &entry.cost {
            answer["cost"] = json!(cost);
        }
        if let Some(why) = why {
            answer["why"] = json!(why);
        }
        if let Some(note) = note {
            answer["note"] = json!(note);
        }
        Ok(answer)
    }

    /// Ask the director whether `agent`, which `what` describes, may
    /// connect from the internet.
    pub fn ask_to_connect(&self, agent: &str, what: String) -> u64 {
        let id = lock(&self.book).keep(Entry {
            id: 0,
            agent: agent.to_owned(),
            what,
            cost: None,
            asked: Asked::Connect,
            state: State::Waiting,
            at: Instant::now(),
        });
        self.decided.notify_all();
        id
    }

    /// The director's word on request `id` to connect, waiting at most
    /// `wait` for it.
    pub fn verdict(&self, id: u64, wait: Duration) -> Verdict {
        self.await_word(id, wait);
        let mut book = lock(&self.book);
        match book.find(id) {
            Some(Entry {
                asked: Asked::Connect,
                state,
                ..
            }) => match state {
                State::Waiting => Verdict::Waiting,
                State::Allowed => Verdict::Allowed,
                State::Declined => Verdict::Declined,
                _ => Verdict::Gone,
            },
            _ => Verdict::Gone,
        }
    }

    /// What a command that asks the document model costs.
    pub fn model_charge(session: &SessionRemote, estimate: Option<u64>) -> Charge {
        match session.document_model() {
            Some(model) if model.billing.charged_per_call() => {
                Charge::Paid(estimate.map_or_else(|| "cost unknown".to_owned(), dollars))
            }
            _ => Charge::Free,
        }
    }

    /// What one generation of images costs.
    pub fn image_charge(session: &SessionRemote) -> Charge {
        match session.image_provider() {
            None => Charge::Free,
            Some(image) => match image.kind {
                ImageProviderKind::LocalDemo | ImageProviderKind::Agent => Charge::Free,
                _ if image.cost_known => Charge::Paid(dollars(image.generation_cost_micros)),
                _ => Charge::Paid("cost unknown".to_owned()),
            },
        }
    }

    /// What the next video of shot `shot_index` costs.
    pub fn video_charge(session: &SessionRemote, shot_index: u32) -> Charge {
        match session.video_provider() {
            None => Charge::Free,
            Some(video) if video.kind == VideoProviderKind::LocalDemo => Charge::Free,
            Some(_) => Charge::Paid(
                session
                    .video_estimate(shot_index)
                    .ok()
                    .flatten()
                    .and_then(|estimate| estimate.cost_micros)
                    .map_or_else(|| "cost unknown".to_owned(), dollars),
            ),
        }
    }
}
