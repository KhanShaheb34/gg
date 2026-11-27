//! The Pull requests page across repositories: what `gh` is asked (per GitHub host, one lean
//! GraphQL query listing the user's own open pull requests, those waiting for their review, and
//! those they are mentioned in or assigned to, 100 of each, newest first; then, for the rows
//! the page shows, one batched query by node id for their state, checks, review decision and
//! activity: the list paints before those come), how its answers are read
//! (leniently: a field it does not know reads as unknown), how the groups are told apart (each
//! pull request in one group only), what "new since you last looked" means, and what gg keeps
//! of it between runs (`pr-inbox.json` in gg's own config folder, never in a repository).
//!
//! Activity: a pull request first listed after the page's first load is *new* until it is
//! opened in gg (the user's own are never new); one opened before (or listed at that first
//! load) is *updated* when it has more comments or reviews than then, unless the latest is the
//! user's own, or (someone else's) its head commit changed. Opening it in gg records what it has
//! now. Nothing here runs `gh`; `repo/inbox.rs` does.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::dto::{
    InboxActivity, InboxChecks, InboxGroup, InboxGroupBy, InboxHost, InboxPr, InboxProblem,
    InboxState, PrInbox, ReviewDecision,
};
use crate::gh::GhFailure;
use crate::hosts::{is_plain_host, is_plain_word};
use crate::pr_details::{https_url, parse_time};

/// How many pull requests each search asks for (GitHub's most per page).
pub(crate) const PER_SEARCH: u32 = 100;

/// The list's query, by name (`GgInbox`, which the tests' fake `gh` knows it by): the
/// signed-in account and four searches of open pull requests in repositories that are not
/// archived, newest first, each pull request's lean fields only (what a row's text needs; the
/// rest is [`DETAILS_QUERY`]'s). `$first` is [`PER_SEARCH`].
pub(crate) const QUERY: &str = "query GgInbox($first:Int!){viewer{login}\
mine:search(query:\"is:pr is:open archived:false author:@me sort:updated-desc\",type:ISSUE,first:$first){issueCount nodes{...P}}\
review:search(query:\"is:pr is:open archived:false review-requested:@me sort:updated-desc\",type:ISSUE,first:$first){issueCount nodes{...P}}\
mentioned:search(query:\"is:pr is:open archived:false mentions:@me sort:updated-desc\",type:ISSUE,first:$first){issueCount nodes{...P}}\
assigned:search(query:\"is:pr is:open archived:false assignee:@me sort:updated-desc\",type:ISSUE,first:$first){issueCount nodes{...P}}}\
fragment P on PullRequest{id number title url isDraft updatedAt headRefName baseRefName \
isCrossRepository repository{owner{login} name} headRepository{owner{login} name} author{login}}";

/// The details' query (`GgInboxDetails`): for pull requests by node id (`$ids`, at most
/// [`DETAILS_BATCH`]), their state, draft, review decision, checks on their head commit, and
/// what their activity is told by (the latest comment's and review's author, the counts, the
/// head commit).
pub(crate) const DETAILS_QUERY: &str = "query GgInboxDetails($ids:[ID!]!){nodes(ids:$ids){\
... on PullRequest{id state isDraft reviewDecision \
comments(last:1){totalCount nodes{author{login}}} reviews(last:1){totalCount nodes{author{login}}} \
commits(last:1){nodes{commit{oid statusCheckRollup{state}}}}}}}";

/// Most pull requests one details query asks about (GitHub's most per `nodes`).
pub(crate) const DETAILS_BATCH: usize = 100;

/// Most pull requests remembered as seen that are not on the page any more.
const MAX_SEEN_GONE: usize = 500;

/// One pull request as read from the answer, with what its activity is told by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Entry {
    pub pr: InboxPr,
    /// What it had when read.
    pub seen: Seen,
    /// Its latest comment, and its latest review, are the user's own.
    pub own_last_comment: bool,
    pub own_last_review: bool,
    /// The user opened it.
    pub own: bool,
    /// Its GraphQL node id (what the details' query names it by).
    #[serde(default)]
    pub id: String,
    /// When its details were last read (unix ms); `None`: not yet.
    #[serde(default)]
    pub details_at: Option<i64>,
}

/// What the details' query says of one pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Details {
    pub state: InboxState,
    pub draft: bool,
    pub checks: InboxChecks,
    pub review: ReviewDecision,
    pub seen: Seen,
    pub own_last_comment: bool,
    pub own_last_review: bool,
}

/// What a pull request had: its comments, its reviews and its head commit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Seen {
    pub comments: u32,
    pub reviews: u32,
    pub head: Option<String>,
    /// When it was recorded (unix ms), for forgetting the oldest.
    #[serde(default)]
    pub at_ms: i64,
    /// Recorded before its details were read: the counts are filled in when they are, and
    /// until then nothing is said of its activity.
    #[serde(default)]
    pub pending: bool,
}

/// A host's answer, read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Answer {
    pub login: Option<String>,
    pub entries: Vec<Entry>,
    /// A search found more than it gave.
    pub partial: bool,
}

fn text<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

fn count(v: &Value, key: &str) -> u32 {
    v.get(key)
        .and_then(|c| c.get("totalCount"))
        .and_then(Value::as_u64)
        .map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX))
}

/// The login of the author of the last node of `v[key]` (`comments(last:1)`).
fn last_author<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key)?
        .get("nodes")?
        .as_array()?
        .last()?
        .get("author")?
        .get("login")?
        .as_str()
}

/// `owner/name` of the repository object `v` (`{owner{login} name}`), if both are plain.
fn repo_of(v: Option<&Value>) -> Option<(String, String)> {
    let v = v?;
    let owner = v.get("owner").and_then(|o| text(o, "login"))?;
    let name = text(v, "name")?;
    (is_plain_word(owner) && is_plain_word(name)).then(|| (owner.to_owned(), name.to_owned()))
}

fn checks_of(state: Option<&str>) -> InboxChecks {
    match state.map(str::to_ascii_uppercase).as_deref() {
        Some("SUCCESS") => InboxChecks::Passing,
        Some("FAILURE" | "ERROR") => InboxChecks::Failing,
        Some("PENDING" | "EXPECTED") => InboxChecks::Running,
        _ => InboxChecks::None,
    }
}

fn review_of(decision: Option<&str>) -> ReviewDecision {
    match decision.map(str::to_ascii_uppercase).as_deref() {
        Some("APPROVED") => ReviewDecision::Approved,
        Some("CHANGES_REQUESTED") => ReviewDecision::ChangesRequested,
        Some("REVIEW_REQUIRED") => ReviewDecision::ReviewRequired,
        _ => ReviewDecision::None,
    }
}

/// The key a pull request is known by: `host/owner/name#number`, lowercase.
pub(crate) fn key_of(host: &str, owner: &str, name: &str, number: u32) -> String {
    format!("{host}/{owner}/{name}#{number}").to_ascii_lowercase()
}

/// A pull request node of host `host`'s answer, if it is one gg can show: a number, a
/// repository with a plain owner and name, and an `https://` page on that host.
fn entry_of(node: &Value, host: &str, group: InboxGroup, viewer: Option<&str>) -> Option<Entry> {
    let number = u32::try_from(node.get("number")?.as_u64()?).ok()?;
    if number == 0 {
        return None;
    }
    let (owner, name) = repo_of(node.get("repository"))?;
    let url = https_url(text(node, "url")).filter(|u| {
        u.strip_prefix("https://")
            .and_then(|rest| rest.split(['/', ':']).next())
            .is_some_and(|h| h.eq_ignore_ascii_case(host))
    })?;
    let author = node
        .get("author")
        .and_then(|a| text(a, "login"))
        .map(str::to_owned);
    let own = author
        .as_deref()
        .zip(viewer)
        .is_some_and(|(a, b)| a.eq_ignore_ascii_case(b));
    let head_repo = repo_of(node.get("headRepository")).map(|(o, n)| format!("{o}/{n}"));
    let pr = InboxPr {
        key: key_of(host, &owner, &name, number),
        group,
        host: host.to_owned(),
        owner,
        name,
        number,
        title: text(node, "title").unwrap_or_default().to_owned(),
        url,
        draft: node
            .get("isDraft")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        head: text(node, "headRefName").unwrap_or_default().to_owned(),
        base: text(node, "baseRefName").unwrap_or_default().to_owned(),
        head_repo,
        cross: node
            .get("isCrossRepository")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        author,
        updated_at_ms: text(node, "updatedAt").and_then(parse_time),
        checks: InboxChecks::None,
        review: ReviewDecision::None,
        mentioned: false,
        assigned: false,
        activity: InboxActivity::None,
        state: InboxState::Open,
        details: false,
        pinned: false,
        listed: true,
    };
    let mut entry = Entry {
        pr,
        seen: Seen::default(),
        own_last_comment: false,
        own_last_review: false,
        own,
        id: text(node, "id").unwrap_or_default().to_owned(),
        details_at: None,
    };
    // A node with its details already (an answer of the full fields): taken as read.
    if node.get("commits").is_some()
        && let Some(d) = details_of(node, viewer)
    {
        entry.take_details(d, 0);
    }
    Some(entry)
}

/// The details in node `node` of the details' query (or of a list answer that has them):
/// `None` for a node that is not a pull request.
fn details_of(node: &Value, viewer: Option<&str>) -> Option<Details> {
    node.as_object().filter(|o| !o.is_empty())?;
    let mine = |login: Option<&str>| {
        login
            .zip(viewer)
            .is_some_and(|(a, b)| a.eq_ignore_ascii_case(b))
    };
    let commit = node
        .get("commits")
        .and_then(|c| c.get("nodes"))
        .and_then(Value::as_array)
        .and_then(|n| n.last())
        .and_then(|n| n.get("commit"));
    let rollup = commit
        .and_then(|c| c.get("statusCheckRollup"))
        .and_then(|r| text(r, "state"));
    let state = match text(node, "state").map(str::to_ascii_uppercase).as_deref() {
        Some("MERGED") => InboxState::Merged,
        Some("CLOSED") => InboxState::Closed,
        _ => InboxState::Open,
    };
    Some(Details {
        state,
        draft: node
            .get("isDraft")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        checks: checks_of(rollup),
        review: review_of(text(node, "reviewDecision")),
        seen: Seen {
            comments: count(node, "comments"),
            reviews: count(node, "reviews"),
            head: commit.and_then(|c| text(c, "oid")).map(str::to_owned),
            at_ms: 0,
            pending: false,
        },
        own_last_comment: mine(last_author(node, "comments")),
        own_last_review: mine(last_author(node, "reviews")),
    })
}

/// The details' answer: each pull request's by its node id. `Err` when it is not an answer,
/// or GitHub answered only errors (their first message). `viewer`: the account asked as.
pub(crate) fn parse_details(
    json: &[u8],
    viewer: Option<&str>,
) -> std::result::Result<Vec<(String, Details)>, String> {
    let v: Value = serde_json::from_slice(json).map_err(|e| e.to_string())?;
    let Some(nodes) = v
        .get("data")
        .and_then(|d| d.get("nodes"))
        .and_then(Value::as_array)
    else {
        return Err(first_error(&v)
            .unwrap_or("GitHub's answer has no data.")
            .to_owned());
    };
    Ok(nodes
        .iter()
        .filter_map(|n| {
            let id = text(n, "id")?.to_owned();
            Some((id, details_of(n, viewer)?))
        })
        .collect())
}

/// The first error message of a GraphQL answer, if any.
fn first_error(v: &Value) -> Option<&str> {
    v.get("errors")
        .and_then(Value::as_array)
        .and_then(|e| e.iter().find_map(|e| text(e, "message")))
        .map(str::trim)
        .filter(|m| !m.is_empty())
}

impl Entry {
    /// Takes `d` as its details, read at `now`.
    pub(crate) fn take_details(&mut self, d: Details, now: i64) {
        self.pr.state = d.state;
        self.pr.draft = d.draft;
        self.pr.checks = d.checks;
        self.pr.review = d.review;
        self.pr.details = true;
        self.seen = d.seen;
        self.own_last_comment = d.own_last_comment;
        self.own_last_review = d.own_last_review;
        self.details_at = Some(now);
    }

    /// Takes the details `old` had (the same pull request as listed before), until they are
    /// read again: the list's own fields (its title, draft, …) stay this answer's.
    fn keep_details_of(&mut self, old: &Entry) {
        // An answer with its own details (the full fields) is newer.
        if self.details_at.is_some() || old.details_at.is_none() {
            return;
        }
        self.pr.state = old.pr.state;
        self.pr.checks = old.pr.checks;
        self.pr.review = old.pr.review;
        self.pr.details = true;
        self.seen = old.seen.clone();
        self.own_last_comment = old.own_last_comment;
        self.own_last_review = old.own_last_review;
        self.details_at = old.details_at;
    }
}

/// Host `host`'s answer to [`QUERY`]: each pull request once, in the first group that has it
/// (the user's, then review requested, then mentioned, then assigned; mentioned and assigned
/// make one group), marked mentioned or assigned wherever it is. `Err` when it is not an
/// answer at all, or GitHub answered only errors (their first message).
pub(crate) fn parse_answer(json: &[u8], host: &str) -> std::result::Result<Answer, String> {
    let v: Value = serde_json::from_slice(json).map_err(|e| e.to_string())?;
    let data = match v.get("data") {
        Some(d) if d.is_object() => d,
        _ => {
            return Err(first_error(&v)
                .unwrap_or("GitHub's answer has no data.")
                .to_owned());
        }
    };
    let viewer = data.get("viewer").and_then(|v| text(v, "login"));
    let mut answer = Answer {
        login: viewer.map(str::to_owned),
        ..Answer::default()
    };
    let mut at: BTreeMap<String, usize> = BTreeMap::new();
    let searches = [
        ("mine", InboxGroup::Mine),
        ("review", InboxGroup::Review),
        ("mentioned", InboxGroup::Involved),
        ("assigned", InboxGroup::Involved),
    ];
    for (alias, group) in searches {
        let Some(search) = data.get(alias) else {
            continue;
        };
        let nodes = search
            .get("nodes")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        let total = search
            .get("issueCount")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        answer.partial |= total > nodes.len() as u64;
        for node in nodes {
            let Some(entry) = entry_of(node, host, group, viewer) else {
                continue;
            };
            let i = match at.get(&entry.pr.key) {
                Some(&i) => i,
                None => {
                    at.insert(entry.pr.key.clone(), answer.entries.len());
                    answer.entries.push(entry);
                    answer.entries.len() - 1
                }
            };
            let pr = &mut answer.entries[i].pr;
            pr.mentioned |= alias == "mentioned";
            pr.assigned |= alias == "assigned";
        }
    }
    Ok(answer)
}

/// What failure `failure` (of host `host`) is, and how the page says it.
pub(crate) fn problem_of(failure: &GhFailure, host: &str) -> (InboxProblem, String) {
    match failure {
        GhFailure::NoGh => (
            InboxProblem::NoGh,
            "The GitHub CLI (gh) was not found. Install it from cli.github.com and sign in with \
             gh auth login."
                .to_owned(),
        ),
        GhFailure::SignedOut(_) => (
            InboxProblem::SignedOut,
            format!(
                "gh is not signed in to {host}. Run gh auth login in a terminal, then refresh."
            ),
        ),
        GhFailure::Failed { message, detail } => problem_of_text(&format!("{message}\n{detail}"))
            .map_or_else(
                || {
                    (
                        InboxProblem::Failed,
                        format!("Could not load them: {message}"),
                    )
                },
                |problem| (problem, said_for(problem, host)),
            ),
    }
}

/// The problem `text` (gh's words) tells of, when it is a rate limit or no connection.
pub(crate) fn problem_of_text(text: &str) -> Option<InboxProblem> {
    let lower = text.to_lowercase();
    if lower.contains("rate limit") {
        return Some(InboxProblem::RateLimited);
    }
    let offline = [
        "error connecting to",
        "no such host",
        "dial tcp",
        "network is unreachable",
        "connection refused",
        "could not resolve",
        "i/o timeout",
        "tls handshake timeout",
        "connection reset",
        "no route to host",
    ];
    offline
        .iter()
        .any(|w| lower.contains(w))
        .then_some(InboxProblem::Offline)
}

fn said_for(problem: InboxProblem, host: &str) -> String {
    match problem {
        InboxProblem::RateLimited => {
            format!("{host}'s rate limit was reached. gg asks again later; try Refresh in a while.")
        }
        InboxProblem::Offline => format!("{host} could not be reached. Check the connection."),
        _ => format!("Could not load them from {host}."),
    }
}

/// What gg keeps of the page between runs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct InboxData {
    /// The page was loaded once: what was listed then is the starting point (nothing new).
    pub started: bool,
    /// What each pull request had when last opened in gg (or first listed), by key.
    pub seen: BTreeMap<String, Seen>,
    /// Each host's pull requests as last loaded.
    pub hosts: BTreeMap<String, CachedHost>,
    /// The folder the last clone went into.
    pub clone_parent: Option<String>,
    /// The pinned pull requests' keys, in the order they were pinned.
    pub pins: Vec<String>,
    /// Each pinned pull request as last listed or read: shown while no list has it (merged,
    /// closed, or past the newest 100).
    pub pinned: BTreeMap<String, Entry>,
    /// How the page groups its list.
    pub group_by: InboxGroupBy,
}

/// One host's pull requests as last loaded, and how its last load ended.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct CachedHost {
    pub login: Option<String>,
    pub entries: Vec<Entry>,
    pub partial: bool,
    pub loaded_at_ms: Option<i64>,
    pub problem: Option<InboxProblem>,
    pub message: Option<String>,
    pub detail: Option<String>,
}

impl InboxData {
    /// Takes host `host`'s answer, loaded at `now`, as part of a load that is the page's first
    /// (`first`: what it lists counts as seen, whichever host answers first) or not: its list
    /// replaces the last one; the user's own pull requests are seen as they first show.
    pub(crate) fn take_answer(&mut self, host: &str, mut answer: Answer, now: i64, first: bool) {
        // What was read of each before stays until it is read again.
        if let Some(before) = self.hosts.get(host) {
            let old: std::collections::HashMap<&str, &Entry> = before
                .entries
                .iter()
                .map(|e| (e.pr.key.as_str(), e))
                .collect();
            for e in &mut answer.entries {
                if let Some(o) = old.get(e.pr.key.as_str()) {
                    e.keep_details_of(o);
                }
            }
        }
        for e in &answer.entries {
            if (first || e.own) && !self.seen.contains_key(&e.pr.key) {
                self.seen.insert(e.pr.key.clone(), Self::seen_now(e, now));
            }
            if self.pins.contains(&e.pr.key) {
                self.pinned.insert(e.pr.key.clone(), e.clone());
            }
        }
        self.started = true;
        self.hosts.insert(
            host.to_owned(),
            CachedHost {
                login: answer.login,
                entries: answer.entries,
                partial: answer.partial,
                loaded_at_ms: Some(now),
                problem: None,
                message: None,
                detail: None,
            },
        );
        self.forget_old_seen();
    }

    /// Host `host` could not be loaded: what was listed stays, marked with why.
    pub(crate) fn take_failure(
        &mut self,
        host: &str,
        problem: InboxProblem,
        message: String,
        detail: Option<String>,
    ) {
        let cached = self.hosts.entry(host.to_owned()).or_default();
        cached.problem = Some(problem);
        cached.message = Some(message);
        cached.detail = detail.filter(|d| !d.trim().is_empty());
    }

    /// What `e` has, recorded as seen at `now` (pending while its details are not read).
    fn seen_now(e: &Entry, now: i64) -> Seen {
        Seen {
            at_ms: now,
            pending: e.details_at.is_none(),
            ..e.seen.clone()
        }
    }

    /// Pull request `key`, listed or pinned.
    fn entry(&self, key: &str) -> Option<&Entry> {
        self.hosts
            .values()
            .flat_map(|h| &h.entries)
            .find(|e| e.pr.key == key)
            .or_else(|| self.pinned.get(key))
    }

    /// Records pull request `key` as seen now with what it has as last read. False when it is
    /// neither listed nor pinned.
    pub(crate) fn mark_seen(&mut self, key: &str, now: i64) -> bool {
        let Some(seen) = self.entry(key).map(|e| Self::seen_now(e, now)) else {
            return false;
        };
        self.seen.insert(key.to_owned(), seen);
        true
    }

    /// The node ids, by host, of the pull requests `keys` names (listed or pinned): what the
    /// details' query is asked about.
    pub(crate) fn ids_of(&self, keys: &[String]) -> BTreeMap<String, Vec<String>> {
        let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for key in keys {
            if let Some(e) = self.entry(key).filter(|e| !e.id.is_empty()) {
                let ids = out.entry(e.pr.host.clone()).or_default();
                if !ids.contains(&e.id) {
                    ids.push(e.id.clone());
                }
            }
        }
        out
    }

    /// The account host `host` answered as, if known.
    pub(crate) fn login_of(&self, host: &str) -> Option<String> {
        self.hosts.get(host).and_then(|h| h.login.clone())
    }

    /// Takes host `host`'s details, read at `now`, into its pull requests (listed and pinned);
    /// what was recorded as seen before they were read is filled in with them.
    pub(crate) fn take_details(&mut self, host: &str, found: Vec<(String, Details)>, now: i64) {
        let by_id: std::collections::HashMap<String, Details> = found.into_iter().collect();
        let entries = self
            .hosts
            .get_mut(host)
            .map(|h| h.entries.iter_mut())
            .into_iter()
            .flatten()
            .chain(self.pinned.values_mut().filter(|e| e.pr.host == host));
        let mut read = Vec::new();
        for e in entries {
            if let Some(d) = by_id.get(&e.id) {
                e.take_details(d.clone(), now);
                read.push((e.pr.key.clone(), e.seen.clone()));
            }
        }
        for (key, now_has) in read {
            if let Some(seen) = self.seen.get_mut(&key)
                && seen.pending
            {
                *seen = Seen {
                    at_ms: seen.at_ms,
                    ..now_has
                };
            }
        }
    }

    /// Pins pull request `key` (or, `pinned` false, unpins it). False when it is neither
    /// listed nor pinned.
    pub(crate) fn set_pin(&mut self, key: &str, pinned: bool) -> bool {
        if !pinned {
            self.pins.retain(|k| k != key);
            self.pinned.remove(key);
            return true;
        }
        let Some(e) = self.entry(key).cloned() else {
            return false;
        };
        if !self.pins.iter().any(|k| k == key) {
            self.pins.push(key.to_owned());
        }
        self.pinned.insert(key.to_owned(), e);
        true
    }

    /// Forgets what was seen of pull requests no longer listed, past the newest
    /// [`MAX_SEEN_GONE`] of them.
    fn forget_old_seen(&mut self) {
        let listed: HashSet<&str> = self
            .hosts
            .values()
            .flat_map(|h| &h.entries)
            .map(|e| e.pr.key.as_str())
            .collect();
        let mut gone: Vec<(i64, String)> = self
            .seen
            .iter()
            .filter(|(k, _)| !listed.contains(k.as_str()) && !self.pins.contains(*k))
            .map(|(k, s)| (s.at_ms, k.clone()))
            .collect();
        if gone.len() <= MAX_SEEN_GONE {
            return;
        }
        gone.sort();
        let excess = gone.len() - MAX_SEEN_GONE;
        for (_, key) in gone.into_iter().take(excess) {
            self.seen.remove(&key);
        }
    }

    /// Forgets host `host` (not GitHub any more, or no longer asked).
    pub(crate) fn forget_host(&mut self, host: &str) {
        self.hosts.remove(host);
    }

    /// The page as it stands: each host's pull requests with their activity, grouped (the
    /// user's, review requested, the rest; each newest first), github.com's host first.
    pub(crate) fn page(&self, fresh: bool) -> PrInbox {
        let shown = |e: &Entry, listed: bool| InboxPr {
            activity: activity(e, self.seen.get(&e.pr.key)),
            pinned: self.pins.contains(&e.pr.key),
            listed,
            ..e.pr.clone()
        };
        let mut prs: Vec<InboxPr> = self
            .hosts
            .values()
            .flat_map(|h| &h.entries)
            .map(|e| shown(e, true))
            .collect();
        // A pinned one no list has any more: as last read.
        let listed: HashSet<String> = prs.iter().map(|p| p.key.clone()).collect();
        prs.extend(
            self.pins
                .iter()
                .filter(|k| !listed.contains(*k))
                .filter_map(|k| self.pinned.get(k))
                .map(|e| shown(e, false)),
        );
        // Stable: within a group, each host's answer is newest first already; across hosts,
        // by time.
        prs.sort_by(|a, b| {
            a.group
                .cmp(&b.group)
                .then_with(|| b.updated_at_ms.cmp(&a.updated_at_ms))
        });
        let mut hosts: Vec<InboxHost> = self
            .hosts
            .iter()
            .map(|(host, h)| InboxHost {
                host: host.clone(),
                login: h.login.clone(),
                problem: h.problem,
                message: h.message.clone(),
                detail: h.detail.clone(),
                partial: h.partial,
                loaded_at_ms: h.loaded_at_ms,
            })
            .collect();
        hosts.sort_by_key(|h| h.host != "github.com");
        PrInbox {
            prs,
            hosts,
            fresh,
            pins: self.pins.clone(),
            group_by: self.group_by,
        }
    }
}

/// Entry `e`'s activity against what was seen of it (see the module comment).
pub(crate) fn activity(e: &Entry, seen: Option<&Seen>) -> InboxActivity {
    let Some(seen) = seen else {
        return if e.own {
            InboxActivity::None
        } else {
            InboxActivity::New
        };
    };
    // Nothing to compare yet: what was seen, or what it has now, is still to be read.
    if seen.pending || e.details_at.is_none() {
        return InboxActivity::None;
    }
    let comments = e.seen.comments > seen.comments && !e.own_last_comment;
    let reviews = e.seen.reviews > seen.reviews && !e.own_last_review;
    // A change seen both times: a commit missing from one answer is no news.
    let commits =
        !e.own && matches!((&seen.head, &e.seen.head), (Some(was), Some(now)) if was != now);
    if comments || reviews || commits {
        InboxActivity::Updated
    } else {
        InboxActivity::None
    }
}

/// The page's file, `pr-inbox.json` in gg's config folder: read once, written whole and
/// atomically (a temp file renamed over it) after each change. A file that cannot be read
/// is started afresh (it is only a cache and what was seen), and left alone until gg writes
/// a new one.
pub struct InboxStore {
    file: PathBuf,
    data: Mutex<InboxData>,
}

/// The page's file name in the config folder.
pub(crate) const FILE_NAME: &str = "pr-inbox.json";

impl InboxStore {
    /// The store of config folder `dir`, as its file holds it.
    pub fn load(dir: &Path) -> Self {
        let file = dir.join(FILE_NAME);
        let data = fs::read(&file)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<InboxData>(&bytes).ok())
            .unwrap_or_default();
        Self {
            file,
            data: Mutex::new(data),
        }
    }

    /// A copy of what is kept.
    pub(crate) fn data(&self) -> InboxData {
        crate::lock_ignoring_poison(&self.data).clone()
    }

    /// Changes what is kept with `change` and writes it (on the caller's thread: call it off
    /// the async runtime); what `change` returns. A file that cannot be written keeps the
    /// change in memory for this run.
    pub(crate) fn change<T>(&self, change: impl FnOnce(&mut InboxData) -> T) -> T {
        let mut data = crate::lock_ignoring_poison(&self.data);
        let out = change(&mut data);
        let _ = self.write(&data);
        out
    }

    fn write(&self, data: &InboxData) -> io::Result<()> {
        let dir = self.file.parent().unwrap_or(Path::new("."));
        fs::create_dir_all(dir)?;
        let json = serde_json::to_vec(data).map_err(io::Error::other)?;
        let tmp = dir.join(format!("{FILE_NAME}.{}.tmp", std::process::id()));
        let written = (|| {
            let mut file = fs::File::create(&tmp)?;
            file.write_all(&json)?;
            file.sync_all()?;
            fs::rename(&tmp, &self.file)
        })();
        if written.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        written
    }
}

/// Most files GitHub lists for a pull request.
pub(crate) const MAX_FILES: usize = 3000;

/// The files in `gh api --paginate repos/<o>/<r>/pulls/<n>/files` output `json`: one JSON
/// array per page, one after the other. A renamed or copied file keeps its old path; a file
/// with no count of lines (no patch, nothing counted: binary, or only renamed) has none.
pub(crate) fn parse_files(json: &[u8]) -> std::result::Result<crate::dto::PrFiles, String> {
    let mut files = Vec::new();
    for page in serde_json::Deserializer::from_slice(json).into_iter::<Vec<Value>>() {
        for f in page.map_err(|e| e.to_string())? {
            let Some(path) = text(&f, "filename").filter(|p| !p.is_empty()) else {
                continue;
            };
            let status = match text(&f, "status").unwrap_or_default() {
                "added" => 'A',
                "removed" => 'D',
                "renamed" => 'R',
                "copied" => 'C',
                "changed" => 'T',
                _ => 'M',
            };
            let num = |k: &str| {
                f.get(k)
                    .and_then(Value::as_u64)
                    .and_then(|n| u32::try_from(n).ok())
            };
            let counted = f.get("patch").is_some() || num("changes").unwrap_or(0) > 0;
            files.push(crate::dto::PrFile {
                path: path.to_owned(),
                old_path: text(&f, "previous_filename")
                    .filter(|p| !p.is_empty() && matches!(status, 'R' | 'C'))
                    .map(str::to_owned),
                status,
                additions: counted.then(|| num("additions").unwrap_or(0)),
                deletions: counted.then(|| num("deletions").unwrap_or(0)),
            });
        }
    }
    let truncated = files.len() >= MAX_FILES;
    files.truncate(MAX_FILES);
    Ok(crate::dto::PrFiles { files, truncated })
}

/// Whether `host`, `owner` and `name` can name a pull request's repository to `gh`: a plain
/// host name, and a plain owner and name.
pub(crate) fn plain_repo(host: &str, owner: &str, name: &str) -> bool {
    is_plain_host(host) && is_plain_word(owner) && is_plain_word(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(number: u32, repo: &str, author: &str) -> Value {
        let (owner, name) = repo.split_once('/').unwrap();
        serde_json::json!({
            "number": number,
            "title": format!("PR {number}"),
            "url": format!("https://github.com/{repo}/pull/{number}"),
            "isDraft": false,
            "updatedAt": "2026-03-01T10:00:00Z",
            "headRefName": "fix/typo",
            "baseRefName": "main",
            "isCrossRepository": false,
            "repository": {"owner": {"login": owner}, "name": name},
            "headRepository": {"owner": {"login": owner}, "name": name},
            "author": {"login": author},
            "reviewDecision": null,
            "comments": {"totalCount": 2, "nodes": [{"author": {"login": "ada"}}]},
            "reviews": {"totalCount": 0, "nodes": []},
            "commits": {"nodes": [{"commit": {"oid": "a1", "statusCheckRollup": {"state": "SUCCESS"}}}]}
        })
    }

    fn answer(groups: &[(&str, Vec<Value>)]) -> Vec<u8> {
        let mut data = serde_json::json!({"viewer": {"login": "octo"}});
        for (alias, nodes) in groups {
            data[*alias] = serde_json::json!({"issueCount": nodes.len(), "nodes": nodes});
        }
        serde_json::to_vec(&serde_json::json!({ "data": data })).unwrap()
    }

    #[test]
    fn each_pull_request_is_in_one_group_and_marked_where_else_it_is() {
        let json = answer(&[
            ("mine", vec![node(1, "octo/cat", "octo")]),
            (
                "review",
                vec![node(2, "octo/cat", "ada"), node(3, "hub/dog", "ada")],
            ),
            (
                "mentioned",
                vec![
                    node(1, "octo/cat", "octo"),
                    node(3, "hub/dog", "ada"),
                    node(4, "hub/dog", "grace"),
                ],
            ),
            (
                "assigned",
                vec![node(4, "hub/dog", "grace"), node(5, "hub/owl", "grace")],
            ),
        ]);
        let a = parse_answer(&json, "github.com").unwrap();
        assert_eq!(a.login.as_deref(), Some("octo"));
        let got: Vec<(u32, InboxGroup, bool, bool)> = a
            .entries
            .iter()
            .map(|e| (e.pr.number, e.pr.group, e.pr.mentioned, e.pr.assigned))
            .collect();
        assert_eq!(
            got,
            [
                (1, InboxGroup::Mine, true, false),
                (2, InboxGroup::Review, false, false),
                (3, InboxGroup::Review, true, false),
                (4, InboxGroup::Involved, true, true),
                (5, InboxGroup::Involved, false, true),
            ]
        );
        assert!(!a.partial);
        let e = &a.entries[0];
        assert_eq!(e.pr.key, "github.com/octo/cat#1");
        assert_eq!(e.pr.checks, InboxChecks::Passing);
        assert_eq!(e.pr.head_repo.as_deref(), Some("octo/cat"));
        assert!(e.own);
        assert_eq!(e.seen.comments, 2);
        assert_eq!(e.seen.head.as_deref(), Some("a1"));
        assert!(e.pr.updated_at_ms.is_some());
    }

    /// Same number in two repositories is two pull requests; the same pull request found by
    /// another case of its owner's name is one.
    #[test]
    fn pull_requests_are_told_apart_by_host_repository_and_number() {
        let json = answer(&[
            ("mine", vec![node(7, "octo/cat", "octo")]),
            (
                "mentioned",
                vec![node(7, "Octo/Cat", "octo"), node(7, "hub/cat", "ada")],
            ),
        ]);
        let a = parse_answer(&json, "github.com").unwrap();
        assert_eq!(a.entries.len(), 2);
        assert!(a.entries[0].pr.mentioned);
    }

    #[test]
    fn checks_reviews_and_odd_nodes_read_leniently() {
        let states = [
            (Some("SUCCESS"), InboxChecks::Passing),
            (Some("FAILURE"), InboxChecks::Failing),
            (Some("ERROR"), InboxChecks::Failing),
            (Some("PENDING"), InboxChecks::Running),
            (Some("EXPECTED"), InboxChecks::Running),
            (Some("SOMETHING_NEW"), InboxChecks::None),
            (None, InboxChecks::None),
        ];
        for (said, want) in states {
            assert_eq!(checks_of(said), want, "{said:?}");
        }
        assert_eq!(
            review_of(Some("CHANGES_REQUESTED")),
            ReviewDecision::ChangesRequested
        );
        assert_eq!(review_of(None), ReviewDecision::None);

        let mut no_commit = node(8, "octo/cat", "ada");
        no_commit["commits"] = serde_json::json!({"nodes": []});
        no_commit["author"] = Value::Null;
        no_commit["headRepository"] = Value::Null;
        let mut other_host = node(9, "octo/cat", "ada");
        other_host["url"] = "https://evil.example/octo/cat/pull/9".into();
        let mut odd_name = node(10, "octo/cat", "ada");
        odd_name["repository"]["name"] = "-rf".into();
        let not_pr = serde_json::json!({});
        let json = answer(&[("review", vec![no_commit, other_host, odd_name, not_pr])]);
        let a = parse_answer(&json, "github.com").unwrap();
        assert_eq!(a.entries.len(), 1);
        let e = &a.entries[0];
        assert_eq!(e.pr.checks, InboxChecks::None);
        assert_eq!(e.pr.author, None);
        assert_eq!(e.pr.head_repo, None);
        assert_eq!(e.seen.head, None);
        // An enterprise host's pages are on that host.
        let mut ghe = node(3, "team/app", "ada");
        ghe["url"] = "https://ghe.example/team/app/pull/3".into();
        let a = parse_answer(&answer(&[("review", vec![ghe])]), "ghe.example").unwrap();
        assert_eq!(a.entries[0].pr.key, "ghe.example/team/app#3");
    }

    #[test]
    fn more_found_than_given_is_partial_and_errors_are_said() {
        let mut v: Value =
            serde_json::from_slice(&answer(&[("mine", vec![node(1, "octo/cat", "octo")])]))
                .unwrap();
        v["data"]["mine"]["issueCount"] = 340.into();
        let a = parse_answer(&serde_json::to_vec(&v).unwrap(), "github.com").unwrap();
        assert!(a.partial);
        let errors = br#"{"errors":[{"type":"RATE_LIMITED","message":"API rate limit exceeded for user ID 1."}]}"#;
        assert_eq!(
            parse_answer(errors, "github.com").unwrap_err(),
            "API rate limit exceeded for user ID 1."
        );
        assert!(parse_answer(b"not json", "github.com").is_err());
        // Data with an error beside it (one search failed): what came is read.
        let partly =
            br#"{"data":{"viewer":{"login":"octo"},"mine":null},"errors":[{"message":"x"}]}"#;
        assert!(
            parse_answer(partly, "github.com")
                .unwrap()
                .entries
                .is_empty()
        );
    }

    #[test]
    fn failures_are_told_apart() {
        let failed = |message: &str| GhFailure::Failed {
            message: message.into(),
            detail: String::new(),
        };
        let cases = [
            (GhFailure::NoGh, InboxProblem::NoGh),
            (GhFailure::SignedOut("x".into()), InboxProblem::SignedOut),
            (
                failed("GraphQL: API rate limit exceeded for user ID 1."),
                InboxProblem::RateLimited,
            ),
            (
                failed("HTTP 403: You have exceeded a secondary rate limit."),
                InboxProblem::RateLimited,
            ),
            (
                failed("error connecting to api.github.com"),
                InboxProblem::Offline,
            ),
            (
                failed(
                    "Post \"https://api.github.com/graphql\": dial tcp: lookup api.github.com: no such host",
                ),
                InboxProblem::Offline,
            ),
            (failed("HTTP 502: Bad gateway"), InboxProblem::Failed),
        ];
        for (failure, want) in cases {
            let (got, message) = problem_of(&failure, "github.com");
            assert_eq!(got, want, "{failure:?}");
            assert!(!message.is_empty());
        }
        let (_, said) = problem_of(&failed("HTTP 502: Bad gateway"), "github.com");
        assert_eq!(said, "Could not load them: HTTP 502: Bad gateway");
    }

    fn entry(key: &str, own: bool, comments: u32, reviews: u32, head: &str) -> Entry {
        let (host_repo, number) = key.split_once('#').unwrap();
        let parts: Vec<&str> = host_repo.split('/').collect();
        let mut n = node(
            number.parse().unwrap(),
            &format!("{}/{}", parts[1], parts[2]),
            "ada",
        );
        n["url"] = format!(
            "https://{}/{}/{}/pull/{number}",
            parts[0], parts[1], parts[2]
        )
        .into();
        let json = answer(&[("review", vec![n])]);
        let mut e = parse_answer(&json, parts[0]).unwrap().entries.remove(0);
        e.own = own;
        e.seen.comments = comments;
        e.seen.reviews = reviews;
        e.seen.head = Some(head.into());
        e
    }

    #[test]
    fn activity_is_what_changed_since_it_was_last_seen() {
        let seen = Seen {
            comments: 2,
            reviews: 1,
            head: Some("a1".into()),
            at_ms: 1,
            pending: false,
        };
        let k = "github.com/octo/cat#1";
        // Never seen: new, unless it is the user's own.
        assert_eq!(
            activity(&entry(k, false, 2, 1, "a1"), None),
            InboxActivity::New
        );
        assert_eq!(
            activity(&entry(k, true, 2, 1, "a1"), None),
            InboxActivity::None
        );
        // As seen: nothing.
        assert_eq!(
            activity(&entry(k, false, 2, 1, "a1"), Some(&seen)),
            InboxActivity::None
        );
        // Another comment or review: updated, unless the latest is the user's.
        let mut more = entry(k, false, 3, 1, "a1");
        assert_eq!(activity(&more, Some(&seen)), InboxActivity::Updated);
        more.own_last_comment = true;
        assert_eq!(activity(&more, Some(&seen)), InboxActivity::None);
        let mut reviewed = entry(k, true, 2, 2, "a1");
        assert_eq!(activity(&reviewed, Some(&seen)), InboxActivity::Updated);
        reviewed.own_last_review = true;
        assert_eq!(activity(&reviewed, Some(&seen)), InboxActivity::None);
        // New commits: on someone else's only (the user pushed their own).
        assert_eq!(
            activity(&entry(k, false, 2, 1, "b2"), Some(&seen)),
            InboxActivity::Updated
        );
        assert_eq!(
            activity(&entry(k, true, 2, 1, "b2"), Some(&seen)),
            InboxActivity::None
        );
        // A head missing from the answer is no new commit.
        let mut headless = entry(k, false, 2, 1, "a1");
        headless.seen.head = None;
        assert_eq!(activity(&headless, Some(&seen)), InboxActivity::None);
        // Fewer comments (one deleted): nothing.
        assert_eq!(
            activity(&entry(k, false, 1, 1, "a1"), Some(&seen)),
            InboxActivity::None
        );
    }

    #[test]
    fn the_first_load_is_the_starting_point_and_opening_records_what_it_has() {
        let mut data = InboxData::default();
        let first = Answer {
            login: Some("octo".into()),
            entries: vec![
                entry("github.com/octo/cat#1", false, 2, 0, "a1"),
                entry("github.com/octo/cat#2", true, 0, 0, "a1"),
            ],
            partial: false,
        };
        data.take_answer("github.com", first, 100, true);
        let page = data.page(true);
        assert!(page.fresh);
        assert!(page.prs.iter().all(|p| p.activity == InboxActivity::None));
        assert_eq!(page.hosts[0].login.as_deref(), Some("octo"));
        assert_eq!(page.hosts[0].loaded_at_ms, Some(100));

        // Next load: #1 has a comment more, #3 is new, #4 is the user's own (never new).
        let next = Answer {
            login: Some("octo".into()),
            entries: vec![
                entry("github.com/octo/cat#1", false, 3, 0, "a1"),
                entry("github.com/octo/cat#3", false, 0, 0, "a1"),
                entry("github.com/octo/cat#4", true, 0, 0, "a1"),
            ],
            partial: true,
        };
        data.take_answer("github.com", next, 200, false);
        let page = data.page(true);
        let of = |n: u32| page.prs.iter().find(|p| p.number == n).unwrap().activity;
        assert_eq!(of(1), InboxActivity::Updated);
        assert_eq!(of(3), InboxActivity::New);
        assert_eq!(of(4), InboxActivity::None);
        assert!(page.prs.iter().all(|p| p.number != 2));
        assert!(page.hosts[0].partial);

        // Opened: nothing new on them any more.
        assert!(data.mark_seen("github.com/octo/cat#1", 300));
        assert!(data.mark_seen("github.com/octo/cat#3", 300));
        assert!(!data.mark_seen("github.com/octo/cat#99", 300));
        let page = data.page(false);
        assert!(page.prs.iter().all(|p| p.activity == InboxActivity::None));
        assert!(!page.fresh);
    }

    #[test]
    fn a_failure_keeps_what_was_listed_and_says_why() {
        let mut data = InboxData::default();
        data.take_answer(
            "github.com",
            Answer {
                login: Some("octo".into()),
                entries: vec![entry("github.com/octo/cat#1", false, 0, 0, "a1")],
                partial: false,
            },
            100,
            true,
        );
        data.take_failure(
            "github.com",
            InboxProblem::Offline,
            "github.com could not be reached.".into(),
            Some("dial tcp".into()),
        );
        let page = data.page(true);
        assert_eq!(page.prs.len(), 1);
        let host = &page.hosts[0];
        assert_eq!(host.problem, Some(InboxProblem::Offline));
        assert_eq!(host.loaded_at_ms, Some(100));
        assert_eq!(host.detail.as_deref(), Some("dial tcp"));
        // Loaded again: the problem goes.
        data.take_answer("github.com", Answer::default(), 200, false);
        assert_eq!(data.page(true).hosts[0].problem, None);
    }

    #[test]
    fn groups_come_in_order_newest_first_github_first() {
        let mut data = InboxData::default();
        let mut a = entry("github.com/octo/cat#1", false, 0, 0, "a");
        a.pr.group = InboxGroup::Involved;
        a.pr.updated_at_ms = Some(50);
        let mut b = entry("github.com/octo/cat#2", false, 0, 0, "a");
        b.pr.group = InboxGroup::Mine;
        b.pr.updated_at_ms = Some(10);
        let mut c = entry("ghe.example/team/app#3", false, 0, 0, "a");
        c.pr.group = InboxGroup::Mine;
        c.pr.updated_at_ms = Some(20);
        data.take_answer(
            "github.com",
            Answer {
                entries: vec![a, b],
                ..Answer::default()
            },
            1,
            true,
        );
        data.take_answer(
            "ghe.example",
            Answer {
                entries: vec![c],
                ..Answer::default()
            },
            1,
            true,
        );
        let page = data.page(true);
        let order: Vec<u32> = page.prs.iter().map(|p| p.number).collect();
        assert_eq!(order, [3, 2, 1]);
        let hosts: Vec<&str> = page.hosts.iter().map(|h| h.host.as_str()).collect();
        assert_eq!(hosts, ["github.com", "ghe.example"]);
    }

    #[test]
    fn what_was_seen_of_pull_requests_long_gone_is_forgotten() {
        let mut data = InboxData::default();
        for i in 0..(MAX_SEEN_GONE + 20) {
            data.seen.insert(
                format!("github.com/octo/cat#{}", i + 1000),
                Seen {
                    at_ms: i as i64,
                    ..Seen::default()
                },
            );
        }
        data.take_answer(
            "github.com",
            Answer {
                entries: vec![entry("github.com/octo/cat#1", false, 0, 0, "a")],
                ..Answer::default()
            },
            10_000,
            true,
        );
        assert_eq!(data.seen.len(), MAX_SEEN_GONE + 1);
        assert!(data.seen.contains_key("github.com/octo/cat#1"));
        assert!(!data.seen.contains_key("github.com/octo/cat#1000"));
    }

    #[test]
    fn the_store_keeps_its_data_between_runs_and_starts_afresh_from_a_bad_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = InboxStore::load(dir.path());
        store.change(|d| {
            d.take_answer(
                "github.com",
                Answer {
                    login: Some("octo".into()),
                    entries: vec![entry("github.com/octo/cat#1", false, 1, 0, "a")],
                    partial: false,
                },
                5,
                true,
            );
            d.clone_parent = Some("/work".into());
        });
        let again = InboxStore::load(dir.path());
        assert_eq!(again.data(), store.data());
        assert_eq!(again.data().page(false).prs.len(), 1);
        fs::write(dir.path().join(FILE_NAME), b"{ not json").unwrap();
        assert_eq!(InboxStore::load(dir.path()).data(), InboxData::default());
        // No temp file is left behind.
        let names: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, [FILE_NAME]);
    }

    /// The list as the lean query gives it: no details until they are read, by node id.
    fn lean(number: u32, repo: &str, author: &str, updated: &str) -> Value {
        let mut n = node(number, repo, author);
        for k in ["comments", "reviews", "commits", "reviewDecision"] {
            n.as_object_mut().unwrap().remove(k);
        }
        n["id"] = format!("PR_{number}").into();
        n["updatedAt"] = updated.into();
        n
    }

    fn details_answer(nodes: Vec<Value>) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({"data": {"nodes": nodes}})).unwrap()
    }

    fn details_node(number: u32, state: &str, checks: &str, comments: u32, last: &str) -> Value {
        serde_json::json!({
            "id": format!("PR_{number}"), "state": state, "isDraft": false,
            "reviewDecision": "APPROVED",
            "comments": {"totalCount": comments, "nodes": [{"author": {"login": last}}]},
            "reviews": {"totalCount": 0, "nodes": []},
            "commits": {"nodes": [{"commit": {"oid": "c1", "statusCheckRollup": {"state": checks}}}]}
        })
    }

    #[test]
    fn the_list_comes_first_and_details_by_id_after() {
        let json = answer(&[(
            "review",
            vec![lean(1, "octo/cat", "ada", "2026-03-01T10:00:00Z")],
        )]);
        let a = parse_answer(&json, "github.com").unwrap();
        let e = &a.entries[0];
        assert_eq!(e.id, "PR_1");
        assert!(!e.pr.details && e.details_at.is_none());
        assert_eq!(e.pr.checks, InboxChecks::None);

        let mut data = InboxData::default();
        data.take_answer("github.com", a, 100, true);
        // On the first load, what it has is to be read: pending, and nothing said of it.
        assert!(data.seen["github.com/octo/cat#1"].pending);
        assert_eq!(data.page(true).prs[0].activity, InboxActivity::None);
        let ids = data.ids_of(&["github.com/octo/cat#1".into(), "github.com/x/y#9".into()]);
        assert_eq!(ids["github.com"], ["PR_1"]);

        let found = parse_details(
            &details_answer(vec![
                details_node(1, "OPEN", "FAILURE", 3, "ada"),
                serde_json::json!({}),
            ]),
            Some("octo"),
        )
        .unwrap();
        assert_eq!(found.len(), 1);
        data.take_details("github.com", found, 200);
        let pr = &data.page(false).prs[0];
        assert!(pr.details);
        assert_eq!(pr.checks, InboxChecks::Failing);
        assert_eq!(pr.review, ReviewDecision::Approved);
        // The pending record took what was read: no activity from it.
        assert!(!data.seen["github.com/octo/cat#1"].pending);
        assert_eq!(data.seen["github.com/octo/cat#1"].comments, 3);
        assert_eq!(pr.activity, InboxActivity::None);

        // Listed again (lean): what was read stays until it is read again.
        let again = parse_answer(&json, "github.com").unwrap();
        data.take_answer("github.com", again, 300, false);
        let pr = &data.page(true).prs[0];
        assert!(pr.details);
        assert_eq!(pr.checks, InboxChecks::Failing);
        // A comment more, someone else's: updated.
        data.take_details(
            "github.com",
            parse_details(
                &details_answer(vec![details_node(1, "OPEN", "SUCCESS", 4, "grace")]),
                Some("octo"),
            )
            .unwrap(),
            400,
        );
        assert_eq!(data.page(false).prs[0].activity, InboxActivity::Updated);
        assert!(parse_details(br#"{"errors":[{"message":"nope"}]}"#, None).is_err());
    }

    #[test]
    fn pins_stay_with_their_last_state_and_group_by_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let store = InboxStore::load(dir.path());
        store.change(|d| {
            let a = parse_answer(
                &answer(&[(
                    "review",
                    vec![
                        lean(1, "octo/cat", "ada", "2026-03-01T10:00:00Z"),
                        lean(2, "octo/cat", "ada", "2026-03-01T10:00:00Z"),
                    ],
                )]),
                "github.com",
            )
            .unwrap();
            d.take_answer("github.com", a, 1, true);
            assert!(d.set_pin("github.com/octo/cat#2", true));
            assert!(
                !d.set_pin("github.com/octo/cat#99", true),
                "not on the page"
            );
            d.group_by = InboxGroupBy::Repository;
        });
        let d = InboxStore::load(dir.path()).data();
        assert_eq!(d.pins, ["github.com/octo/cat#2"]);
        assert_eq!(d.group_by, InboxGroupBy::Repository);
        let page = d.page(false);
        assert_eq!(page.group_by, InboxGroupBy::Repository);
        assert!(
            page.prs
                .iter()
                .any(|p| p.number == 2 && p.pinned && p.listed)
        );

        // Merged since: no list has it, it stays pinned with its state.
        let mut d = d;
        d.take_details(
            "github.com",
            parse_details(
                &details_answer(vec![details_node(2, "MERGED", "SUCCESS", 1, "ada")]),
                None,
            )
            .unwrap(),
            5,
        );
        let a = parse_answer(
            &answer(&[(
                "review",
                vec![lean(1, "octo/cat", "ada", "2026-03-01T10:00:00Z")],
            )]),
            "github.com",
        )
        .unwrap();
        d.take_answer("github.com", a, 6, false);
        let page = d.page(true);
        let gone = page.prs.iter().find(|p| p.number == 2).unwrap();
        assert!(gone.pinned && !gone.listed);
        assert_eq!(gone.state, InboxState::Merged);
        // Its details are still asked by key.
        assert_eq!(
            d.ids_of(&["github.com/octo/cat#2".into()])["github.com"],
            ["PR_2"]
        );
        assert!(d.set_pin("github.com/octo/cat#2", false));
        assert!(d.page(true).prs.iter().all(|p| p.number != 2));
    }

    #[test]
    fn files_read_across_pages_with_renames_and_without_counts() {
        let pages = br#"[
          {"filename":"src/a.ts","status":"modified","additions":3,"deletions":1,"changes":4,"patch":"@@"},
          {"filename":"docs/new.md","previous_filename":"docs/old.md","status":"renamed","additions":0,"deletions":0,"changes":0}
        ]
        [
          {"filename":"logo.png","status":"added","additions":0,"deletions":0,"changes":0},
          {"filename":"gone.txt","status":"removed","additions":0,"deletions":2,"changes":2,"patch":"@@"},
          {"filename":"x","status":"copied","previous_filename":"y","additions":1,"deletions":0,"changes":1,"patch":"@@"},
          {"status":"modified"}
        ]"#;
        let f = parse_files(pages).unwrap();
        assert!(!f.truncated);
        let got: Vec<(&str, char, Option<&str>, Option<u32>)> = f
            .files
            .iter()
            .map(|x| {
                (
                    x.path.as_str(),
                    x.status,
                    x.old_path.as_deref(),
                    x.additions,
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                ("src/a.ts", 'M', None, Some(3)),
                ("docs/new.md", 'R', Some("docs/old.md"), None),
                ("logo.png", 'A', None, None),
                ("gone.txt", 'D', None, Some(0)),
                ("x", 'C', Some("y"), Some(1)),
            ]
        );
        assert_eq!(parse_files(b"").unwrap().files.len(), 0);
        assert!(parse_files(b"{not json").is_err());
        // GitHub's most: said to be cut.
        let many: Vec<Value> = (0..MAX_FILES)
            .map(|i| serde_json::json!({"filename": format!("f{i}"), "status": "added"}))
            .collect();
        let f = parse_files(&serde_json::to_vec(&many).unwrap()).unwrap();
        assert!(f.truncated);
        assert_eq!(f.files.len(), MAX_FILES);
    }

    #[test]
    fn the_query_asks_four_searches_once() {
        for alias in [
            "mine:search(",
            "review:search(",
            "mentioned:search(",
            "assigned:search(",
        ] {
            assert_eq!(QUERY.matches(alias).count(), 1, "{alias}");
        }
        assert!(QUERY.starts_with("query GgInbox($first:Int!)"));
        // The list is lean: what a row's text needs, nothing that makes GitHub look further.
        for slow in ["statusCheckRollup", "reviewDecision", "comments", "reviews"] {
            assert!(!QUERY.contains(slow), "{slow}");
            assert!(DETAILS_QUERY.contains(slow), "{slow}");
        }
        assert!(DETAILS_QUERY.starts_with("query GgInboxDetails($ids:[ID!]!)"));
        assert!(QUERY.contains("review-requested:@me"));
        assert!(plain_repo("github.com", "octo", "cat"));
        assert!(!plain_repo("github.com", "-octo", "cat"));
        assert!(!plain_repo("evil host", "octo", "cat"));
    }
}
