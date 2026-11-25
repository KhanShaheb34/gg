//! A pull request's merge status, checks and comments as `gh` gives them (`pr_details`):
//! `gh pr view <number> --repo <repo> --json …` for the pull request, its checks, its
//! conversation and its reviews, and one `gh api graphql` query for its review threads (the
//! comments on the code, which `gh pr view` leaves out), which also says which authors are
//! bots and where each review is on GitHub.
//!
//! Read leniently: a field missing or `null`, a state this version does not know, or a check
//! of another shape reads as "unknown", never as a failure of the whole answer. Only output
//! that is not a JSON object is one. Bodies are kept as written: they are untrusted Markdown
//! with HTML in it, which the UI renders without that HTML.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::dto::{
    CheckState, ChecksSummary, CommentKind, MergeStatus, PrCheck, PrComment, PrDetails,
    PrLifecycle, PrReply, PrThread, ReviewDecision, ReviewState,
};

/// The fields `gh pr view` is asked for.
pub(crate) const VIEW_FIELDS: &str = "number,state,isDraft,mergeable,mergeStateStatus,\
                                      reviewDecision,statusCheckRollup,comments,reviews,url,\
                                      updatedAt";

/// The review threads (their first 50, the first 30 comments of each), and the authors of
/// the reviews and conversation comments with their kind (`__typename`: `Bot` for a bot,
/// which `gh pr view` does not say) and each review's page. `$o`, `$r`: the repository's
/// owner and name; `$n`: the pull request's number.
pub(crate) const THREADS_QUERY: &str = "query($o:String!,$r:String!,$n:Int!){\
repository(owner:$o,name:$r){pullRequest(number:$n){\
reviewThreads(first:50){pageInfo{hasNextPage endCursor} nodes{id isResolved isOutdated path \
line originalLine comments(first:30){nodes{id databaseId url body createdAt \
author{login __typename}}}}} \
reviews(last:100){nodes{id url author{login __typename}}} \
comments(last:100){nodes{author{login __typename}}}}}}";

/// Most comments kept (the newest); the number left out is said.
pub const MAX_COMMENTS: usize = 200;

/// The login GitHub shows for a deleted account.
const GHOST: &str = "ghost";

/// `gh pr view`'s answer, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct View {
    number: u32,
    url: String,
    state: PrLifecycle,
    merge: MergeStatus,
    review: ReviewDecision,
    checks: Vec<PrCheck>,
    comments: Vec<Raw>,
    reviews: Vec<Raw>,
}

/// A comment or review as read, before its author's kind is known.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Raw {
    id: String,
    login: Option<String>,
    body: String,
    created_at_ms: Option<i64>,
    url: Option<String>,
    review_state: Option<ReviewState>,
    minimized: bool,
}

/// The GraphQL query's answer, read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Threads {
    threads: Vec<RawThread>,
    /// More threads than were read.
    partial: bool,
    /// Logins GraphQL calls a `Bot`, lowercase.
    bots: HashSet<String>,
    /// Each review's page, by its id.
    review_urls: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RawThread {
    path: String,
    line: Option<u32>,
    resolved: bool,
    outdated: bool,
    comments: Vec<Raw>,
}

fn text<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

fn flag(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn items<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// `text`, upper case, for comparing GitHub's enum values.
fn upper(text: Option<&str>) -> String {
    text.unwrap_or_default().trim().to_ascii_uppercase()
}

/// `url` when it is an `https://` address with nothing odd in it; anything else is left out.
pub(crate) fn https_url(url: Option<&str>) -> Option<String> {
    let url = url?.trim();
    let scheme = url.get(..8)?;
    (scheme.eq_ignore_ascii_case("https://")
        && url.len() > 8
        && url.len() <= 2048
        && !url.chars().any(|c| c.is_control() || c.is_whitespace()))
    .then(|| url.to_owned())
}

/// An ISO 8601 time as GitHub gives it (`2026-11-24T12:46:19Z`, with fractions of a second
/// or an offset), in unix milliseconds; `None` for anything else, and for GitHub's "no time"
/// (`0001-01-01T00:00:00Z`) or any time before 1970.
pub(crate) fn parse_time(text: &str) -> Option<i64> {
    let b = text.trim().as_bytes();
    let num = |from: usize, len: usize| -> Option<i64> {
        let digits = b.get(from..from + len)?;
        digits.iter().try_fold(0_i64, |n, d| {
            d.is_ascii_digit().then(|| n * 10 + i64::from(d - b'0'))
        })
    };
    let at = |i: usize, c: u8| b.get(i) == Some(&c);
    if !(at(4, b'-') && at(7, b'-') && (at(10, b'T') || at(10, b't') || at(10, b' '))) {
        return None;
    }
    if !(at(13, b':') && at(16, b':')) {
        return None;
    }
    let (year, month, day) = (num(0, 4)?, num(5, 2)?, num(8, 2)?);
    let (hour, minute, second) = (num(11, 2)?, num(14, 2)?, num(17, 2)?);
    if year < 1970 || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let mut i = 19;
    let mut millis = 0_i64;
    if at(i, b'.') {
        i += 1;
        let start = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == start {
            return None;
        }
        let digits = &b[start..i.min(start + 3)];
        millis = digits.iter().fold(0, |n, d| n * 10 + i64::from(d - b'0'))
            * 10_i64.pow(3 - u32::try_from(digits.len()).ok()?);
    }
    let offset_secs = match b.get(i) {
        Some(b'Z' | b'z') if i + 1 == b.len() => 0,
        Some(sign @ (b'+' | b'-')) if i + 6 == b.len() && at(i + 3, b':') => {
            let secs = num(i + 1, 2)? * 3600 + num(i + 4, 2)? * 60;
            if *sign == b'+' { secs } else { -secs }
        }
        _ => return None,
    };
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + hour * 3600 + minute * 60 + second - offset_secs;
    Some(secs * 1000 + millis)
}

fn time(v: &Value, key: &str) -> Option<i64> {
    text(v, key).and_then(parse_time)
}

/// Where a pull request stands for merging, from what `gh pr view` says of it.
pub fn merge_status(is_draft: bool, mergeable: &str, merge_state: &str) -> MergeStatus {
    let merge_state = merge_state.trim().to_ascii_uppercase();
    if is_draft || merge_state == "DRAFT" {
        return MergeStatus::Draft;
    }
    if merge_state == "DIRTY" || mergeable.trim().eq_ignore_ascii_case("CONFLICTING") {
        return MergeStatus::Conflicts;
    }
    match merge_state.as_str() {
        "BEHIND" => MergeStatus::Behind,
        "BLOCKED" => MergeStatus::Blocked,
        "UNSTABLE" => MergeStatus::Unstable,
        "CLEAN" | "HAS_HOOKS" => MergeStatus::Ready,
        _ => MergeStatus::Unknown,
    }
}

fn review_decision(decision: Option<&str>) -> ReviewDecision {
    match upper(decision).as_str() {
        "APPROVED" => ReviewDecision::Approved,
        "CHANGES_REQUESTED" => ReviewDecision::ChangesRequested,
        "REVIEW_REQUIRED" => ReviewDecision::ReviewRequired,
        _ => ReviewDecision::None,
    }
}

fn review_state(state: Option<&str>) -> ReviewState {
    match upper(state).as_str() {
        "APPROVED" => ReviewState::Approved,
        "CHANGES_REQUESTED" => ReviewState::ChangesRequested,
        "COMMENTED" => ReviewState::Commented,
        "DISMISSED" => ReviewState::Dismissed,
        "PENDING" => ReviewState::Pending,
        _ => ReviewState::Unknown,
    }
}

fn lifecycle(state: Option<&str>) -> PrLifecycle {
    match upper(state).as_str() {
        "OPEN" => PrLifecycle::Open,
        "MERGED" => PrLifecycle::Merged,
        "CLOSED" => PrLifecycle::Closed,
        _ => PrLifecycle::Unknown,
    }
}

/// How a finished check run's conclusion reads.
fn concluded(conclusion: &str) -> CheckState {
    match conclusion {
        "SUCCESS" => CheckState::Passing,
        "FAILURE" | "TIMED_OUT" | "STARTUP_FAILURE" | "ACTION_REQUIRED" => CheckState::Failing,
        "CANCELLED" => CheckState::Cancelled,
        "SKIPPED" => CheckState::Skipped,
        "NEUTRAL" | "STALE" => CheckState::Neutral,
        _ => CheckState::Unknown,
    }
}

/// How a check run (`status`, `conclusion`) stands.
fn run_state(status: Option<&str>, conclusion: Option<&str>) -> CheckState {
    let (status, conclusion) = (upper(status), upper(conclusion));
    match status.as_str() {
        "QUEUED" | "IN_PROGRESS" | "WAITING" | "PENDING" | "REQUESTED" => CheckState::Pending,
        _ if !conclusion.is_empty() => concluded(&conclusion),
        _ => CheckState::Unknown,
    }
}

/// How a commit status (`state`) stands.
fn status_state(state: Option<&str>) -> CheckState {
    match upper(state).as_str() {
        "SUCCESS" => CheckState::Passing,
        "FAILURE" | "ERROR" => CheckState::Failing,
        "PENDING" | "EXPECTED" => CheckState::Pending,
        _ => CheckState::Unknown,
    }
}

/// One item of `statusCheckRollup`: a `CheckRun` or a `StatusContext` (told by
/// `__typename`, else by its fields); another shape reads as a check whose state is unknown.
fn check_of(item: &Value) -> PrCheck {
    let kind = text(item, "__typename");
    let status = kind == Some("StatusContext")
        || (kind.is_none() && item.get("context").is_some() && item.get("name").is_none());
    let named = |key: &str| {
        text(item, key)
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(str::to_owned)
    };
    if status {
        PrCheck {
            name: named("context").unwrap_or_else(|| "Status".to_owned()),
            workflow: None,
            state: status_state(text(item, "state")),
            started_at_ms: time(item, "createdAt"),
            completed_at_ms: None,
            url: https_url(text(item, "targetUrl")),
        }
    } else {
        PrCheck {
            name: named("name").unwrap_or_else(|| "Check".to_owned()),
            workflow: named("workflowName"),
            state: run_state(text(item, "status"), text(item, "conclusion")),
            started_at_ms: time(item, "startedAt"),
            completed_at_ms: time(item, "completedAt"),
            url: https_url(text(item, "detailsUrl")),
        }
    }
}

/// How many checks stand each way.
pub fn summarize(checks: &[PrCheck]) -> ChecksSummary {
    let mut s = ChecksSummary::default();
    for c in checks {
        s.total += 1;
        *match c.state {
            CheckState::Passing => &mut s.passing,
            CheckState::Failing => &mut s.failing,
            CheckState::Pending => &mut s.pending,
            CheckState::Skipped => &mut s.skipped,
            CheckState::Cancelled => &mut s.cancelled,
            CheckState::Neutral => &mut s.neutral,
            CheckState::Unknown => &mut s.unknown,
        } += 1;
    }
    s
}

fn login(v: &Value) -> Option<String> {
    v.get("author")
        .and_then(|a| text(a, "login"))
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
}

fn raw(v: &Value, time_key: &str, review: bool) -> Raw {
    Raw {
        id: text(v, "id").unwrap_or_default().to_owned(),
        login: login(v),
        body: text(v, "body").unwrap_or_default().to_owned(),
        created_at_ms: time(v, time_key),
        url: https_url(text(v, "url")),
        review_state: review.then(|| review_state(text(v, "state"))),
        minimized: flag(v, "isMinimized"),
    }
}

/// `json` not a JSON object: the plain error said for it.
fn object(json: &[u8]) -> std::result::Result<Value, String> {
    match serde_json::from_slice::<Value>(json) {
        Ok(v) if v.is_object() => Ok(v),
        Ok(_) => Err("gh's answer was not what was asked for.".to_owned()),
        Err(e) => Err(format!("gh's answer could not be read: {e}")),
    }
}

/// `gh pr view --json VIEW_FIELDS` output `json`, read (see the module comment).
pub(crate) fn parse_view(json: &[u8]) -> std::result::Result<View, String> {
    let v = object(json)?;
    let rollup = items(&v, "statusCheckRollup");
    Ok(View {
        number: v
            .get("number")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(0),
        url: https_url(text(&v, "url")).unwrap_or_default(),
        state: lifecycle(text(&v, "state")),
        merge: merge_status(
            flag(&v, "isDraft"),
            text(&v, "mergeable").unwrap_or_default(),
            text(&v, "mergeStateStatus").unwrap_or_default(),
        ),
        review: review_decision(text(&v, "reviewDecision")),
        checks: rollup.iter().map(check_of).collect(),
        comments: items(&v, "comments")
            .iter()
            .map(|c| raw(c, "createdAt", false))
            .collect(),
        reviews: items(&v, "reviews")
            .iter()
            .map(|r| raw(r, "submittedAt", true))
            .collect(),
    })
}

/// The GraphQL query's output `json` ([`THREADS_QUERY`]), read; the error GitHub gave when
/// there is no pull request in it.
pub(crate) fn parse_threads(json: &[u8]) -> std::result::Result<Threads, String> {
    let v = object(json)?;
    let pr = v
        .get("data")
        .and_then(|d| d.get("repository"))
        .and_then(|r| r.get("pullRequest"))
        .filter(|p| p.is_object());
    let Some(pr) = pr else {
        let said = items(&v, "errors")
            .iter()
            .find_map(|e| text(e, "message"))
            .map(str::trim)
            .filter(|m| !m.is_empty());
        return Err(said.map_or_else(
            || "GitHub did not give the review threads.".to_owned(),
            str::to_owned,
        ));
    };
    let mut out = Threads::default();
    let mut note_author = |node: &Value| {
        if let Some(author) = node.get("author")
            && text(author, "__typename") == Some("Bot")
            && let Some(login) = text(author, "login")
        {
            out.bots.insert(bare_login(login).to_ascii_lowercase());
        }
    };
    let threads = pr.get("reviewThreads").unwrap_or(&Value::Null);
    let nodes = items(threads, "nodes");
    for node in nodes {
        for c in items(node.get("comments").unwrap_or(&Value::Null), "nodes") {
            note_author(c);
        }
    }
    for r in items(pr.get("reviews").unwrap_or(&Value::Null), "nodes") {
        note_author(r);
    }
    for c in items(pr.get("comments").unwrap_or(&Value::Null), "nodes") {
        note_author(c);
    }
    out.partial = threads
        .get("pageInfo")
        .is_some_and(|p| flag(p, "hasNextPage"));
    out.threads = nodes
        .iter()
        .map(|t| {
            let line = ["line", "originalLine"].iter().find_map(|k| {
                t.get(*k)
                    .and_then(Value::as_u64)
                    .and_then(|n| u32::try_from(n).ok())
            });
            RawThread {
                path: text(t, "path").unwrap_or_default().to_owned(),
                line,
                resolved: flag(t, "isResolved"),
                outdated: flag(t, "isOutdated"),
                comments: items(t.get("comments").unwrap_or(&Value::Null), "nodes")
                    .iter()
                    .map(|c| raw(c, "createdAt", false))
                    .collect(),
            }
        })
        .collect();
    out.review_urls = items(pr.get("reviews").unwrap_or(&Value::Null), "nodes")
        .iter()
        .filter_map(|r| Some((text(r, "id")?.to_owned(), https_url(text(r, "url"))?)))
        .collect();
    Ok(out)
}

/// `login` without a bot's `[bot]`.
fn bare_login(login: &str) -> &str {
    login.strip_suffix("[bot]").unwrap_or(login)
}

/// Whether `login` is a bot's: its login ends in `[bot]` (as GitHub's REST API and
/// `gh` write an app's), or GitHub's GraphQL API called it a `Bot` anywhere in its answer
/// (`bots`, lowercase): `gh pr view` writes an app such as `coderabbitai` without the
/// suffix and does not say what it is.
pub fn is_bot(login: &str, bots: &HashSet<String>) -> bool {
    login.ends_with("[bot]") || bots.contains(&bare_login(login).to_ascii_lowercase())
}

/// The pull request `view` with the review threads `threads` (or why they could not be read),
/// as `pr_details` gives it, loaded at `loaded_at_ms`.
pub(crate) fn assemble(
    view: View,
    threads: std::result::Result<Threads, String>,
    loaded_at_ms: i64,
) -> PrDetails {
    let (threads, threads_error) = match threads {
        Ok(t) => (t, None),
        Err(why) => (Threads::default(), Some(why)),
    };
    let pr_url = view.url.clone();
    let who = |login: &Option<String>| {
        let login = login.as_deref().unwrap_or(GHOST);
        (bare_login(login).to_owned(), is_bot(login, &threads.bots))
    };
    let mut comments: Vec<PrComment> = Vec::new();
    for c in view.comments {
        let (author, is_bot) = who(&c.login);
        comments.push(PrComment {
            id: c.id,
            kind: CommentKind::Comment,
            author,
            is_bot,
            created_at_ms: c.created_at_ms,
            body: c.body,
            url: c.url.unwrap_or_else(|| pr_url.clone()),
            review_state: None,
            thread: None,
            minimized: c.minimized,
        });
    }
    // A review with nothing written in its summary says nothing here (its verdict is in the
    // review decision, its code comments in their threads).
    for r in view
        .reviews
        .into_iter()
        .filter(|r| !r.body.trim().is_empty())
    {
        let (author, is_bot) = who(&r.login);
        let url = threads
            .review_urls
            .get(&r.id)
            .cloned()
            .or(r.url)
            .unwrap_or_else(|| pr_url.clone());
        comments.push(PrComment {
            id: r.id,
            kind: CommentKind::Review,
            author,
            is_bot,
            created_at_ms: r.created_at_ms,
            body: r.body,
            url,
            review_state: r.review_state,
            thread: None,
            minimized: r.minimized,
        });
    }
    for t in threads.threads.iter().cloned() {
        let mut thread_comments = t.comments.into_iter();
        let Some(first) = thread_comments.next() else {
            continue;
        };
        let replies = thread_comments
            .map(|c| {
                let (author, is_bot) = who(&c.login);
                PrReply {
                    id: c.id,
                    author,
                    is_bot,
                    created_at_ms: c.created_at_ms,
                    body: c.body,
                    url: c.url.unwrap_or_else(|| pr_url.clone()),
                }
            })
            .collect();
        let (author, is_bot) = who(&first.login);
        comments.push(PrComment {
            id: first.id,
            kind: CommentKind::Thread,
            author,
            is_bot,
            created_at_ms: first.created_at_ms,
            body: first.body,
            url: first.url.unwrap_or_else(|| pr_url.clone()),
            review_state: None,
            thread: Some(PrThread {
                path: t.path,
                line: t.line,
                resolved: t.resolved,
                outdated: t.outdated,
                replies,
            }),
            minimized: false,
        });
    }
    // Oldest first; one without a time last.
    comments.sort_by_key(|c| c.created_at_ms.unwrap_or(i64::MAX));
    let omitted = comments.len().saturating_sub(MAX_COMMENTS);
    comments.drain(..omitted);
    PrDetails {
        number: view.number,
        url: view.url,
        state: view.state,
        merge: view.merge,
        review: view.review,
        checks_summary: summarize(&view.checks),
        checks: view.checks,
        comments,
        comments_omitted: u32::try_from(omitted).unwrap_or(u32::MAX),
        threads_error,
        threads_partial: threads.partial,
        loaded_at_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEW_FINISHED: &[u8] = include_bytes!("../tests/fixtures/gh/pr22-view.json");
    const VIEW_PENDING: &[u8] = include_bytes!("../tests/fixtures/gh/pr25-view-pending.json");
    const THREADS: &[u8] = include_bytes!("../tests/fixtures/gh/pr22-threads.json");

    fn view(json: &str) -> View {
        parse_view(json.as_bytes()).unwrap()
    }

    #[test]
    fn times_read_as_github_writes_them() {
        assert_eq!(parse_time("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_time("2026-11-24T12:46:19Z"), Some(1_795_524_379_000));
        assert_eq!(
            parse_time("2026-11-24T12:46:19.250Z"),
            Some(1_795_524_379_250)
        );
        assert_eq!(
            parse_time("2026-11-24T14:46:19+02:00"),
            Some(1_795_524_379_000)
        );
        assert_eq!(parse_time("2024-02-29T00:00:00Z"), Some(1_709_164_800_000));
        // GitHub's "no time", and what is not a time.
        for not in [
            "0001-01-01T00:00:00Z",
            "",
            "yesterday",
            "2026-13-01T00:00:00Z",
            "2026-11-24T12:46:19",
            "2026-11-24T12:46:19Zjunk",
            "2026-11-24T12:46:19.Z",
            "２０２６-11-24T12:46:19Z",
        ] {
            assert_eq!(parse_time(not), None, "{not}");
        }
    }

    /// Every `mergeStateStatus` GitHub documents, and one it may add later.
    #[test]
    fn every_merge_state_maps_to_a_status() {
        for (state, want) in [
            ("BEHIND", MergeStatus::Behind),
            ("BLOCKED", MergeStatus::Blocked),
            ("CLEAN", MergeStatus::Ready),
            ("DIRTY", MergeStatus::Conflicts),
            ("DRAFT", MergeStatus::Draft),
            ("HAS_HOOKS", MergeStatus::Ready),
            ("UNKNOWN", MergeStatus::Unknown),
            ("UNSTABLE", MergeStatus::Unstable),
            ("SOMETHING_NEW", MergeStatus::Unknown),
            ("", MergeStatus::Unknown),
            ("clean", MergeStatus::Ready),
        ] {
            assert_eq!(merge_status(false, "MERGEABLE", state), want, "{state}");
        }
        // A draft is a draft, whatever else; conflicts show whatever the state says.
        assert_eq!(
            merge_status(true, "CONFLICTING", "DIRTY"),
            MergeStatus::Draft
        );
        assert_eq!(
            merge_status(false, "CONFLICTING", "UNKNOWN"),
            MergeStatus::Conflicts
        );
        assert_eq!(
            merge_status(false, "CONFLICTING", "BLOCKED"),
            MergeStatus::Conflicts
        );
    }

    /// The captured finished run: three passing CI jobs and an app's neutral check (no
    /// workflow), a merged pull request whose mergeability GitHub no longer works out.
    #[test]
    fn a_finished_run_reads() {
        let v = parse_view(VIEW_FINISHED).unwrap();
        assert_eq!(v.number, 22);
        assert_eq!(v.state, PrLifecycle::Merged);
        assert_eq!(v.merge, MergeStatus::Unknown);
        assert_eq!(v.review, ReviewDecision::None);
        assert_eq!(v.checks.len(), 4);
        let app = &v.checks[3];
        assert_eq!(
            (app.name.as_str(), app.workflow.as_deref(), app.state),
            ("CodeRabbit", None, CheckState::Neutral)
        );
        let mac = &v.checks[0];
        assert_eq!(mac.name, "macos-latest");
        assert_eq!(mac.workflow.as_deref(), Some("CI"));
        assert_eq!(mac.state, CheckState::Passing);
        assert_eq!(
            mac.completed_at_ms.unwrap() - mac.started_at_ms.unwrap(),
            (11 * 60 + 52) * 1000
        );
        assert!(
            mac.url
                .as_deref()
                .unwrap()
                .starts_with("https://github.com/")
        );
        let s = summarize(&v.checks);
        assert_eq!((s.total, s.passing, s.neutral), (4, 3, 1));
    }

    /// The captured running checks: no completion time (GitHub's year 1), pending, and the
    /// pull request unstable for it.
    #[test]
    fn running_checks_read_as_pending() {
        let v = parse_view(VIEW_PENDING).unwrap();
        assert_eq!(v.state, PrLifecycle::Open);
        assert_eq!(v.merge, MergeStatus::Unstable);
        assert!(v.comments.is_empty() && v.reviews.is_empty());
        assert_eq!(v.checks.len(), 3);
        for c in &v.checks {
            assert_eq!(c.state, CheckState::Pending, "{c:?}");
            assert!(c.started_at_ms.is_some());
            assert_eq!(c.completed_at_ms, None);
        }
        let s = summarize(&v.checks);
        assert_eq!((s.total, s.pending), (3, 3));
    }

    /// Both kinds of rollup item, every state each can be in, and what this version does not
    /// know: an unknown state, a shape of neither kind, `null` fields.
    #[test]
    fn check_runs_and_commit_statuses_read() {
        let run = |status: &str, conclusion: &str| {
            check_of(&serde_json::json!({
                "__typename": "CheckRun", "name": "build", "status": status,
                "conclusion": conclusion
            }))
            .state
        };
        for (status, conclusion, want) in [
            ("COMPLETED", "SUCCESS", CheckState::Passing),
            ("COMPLETED", "FAILURE", CheckState::Failing),
            ("COMPLETED", "TIMED_OUT", CheckState::Failing),
            ("COMPLETED", "STARTUP_FAILURE", CheckState::Failing),
            ("COMPLETED", "ACTION_REQUIRED", CheckState::Failing),
            ("COMPLETED", "CANCELLED", CheckState::Cancelled),
            ("COMPLETED", "SKIPPED", CheckState::Skipped),
            ("COMPLETED", "NEUTRAL", CheckState::Neutral),
            ("COMPLETED", "STALE", CheckState::Neutral),
            ("COMPLETED", "", CheckState::Unknown),
            ("COMPLETED", "SOMETHING_NEW", CheckState::Unknown),
            ("QUEUED", "", CheckState::Pending),
            ("IN_PROGRESS", "", CheckState::Pending),
            ("WAITING", "", CheckState::Pending),
            ("PENDING", "", CheckState::Pending),
            ("REQUESTED", "", CheckState::Pending),
            ("SOMETHING_NEW", "", CheckState::Unknown),
        ] {
            assert_eq!(run(status, conclusion), want, "{status} {conclusion}");
        }
        let status = |state: &str| {
            check_of(&serde_json::json!({
                "__typename": "StatusContext", "context": "ci/legacy", "state": state,
                "targetUrl": "https://ci.example.com/build/7", "createdAt": "2026-11-24T12:00:00Z"
            }))
        };
        for (state, want) in [
            ("SUCCESS", CheckState::Passing),
            ("FAILURE", CheckState::Failing),
            ("ERROR", CheckState::Failing),
            ("PENDING", CheckState::Pending),
            ("EXPECTED", CheckState::Pending),
            ("SOMETHING_NEW", CheckState::Unknown),
        ] {
            assert_eq!(status(state).state, want, "{state}");
        }
        let s = status("SUCCESS");
        assert_eq!(s.name, "ci/legacy");
        assert_eq!(s.workflow, None);
        assert_eq!(s.url.as_deref(), Some("https://ci.example.com/build/7"));
        assert!(s.started_at_ms.is_some());
        // A status without its __typename is still told by its fields.
        let untyped = check_of(&serde_json::json!({"context": "lint", "state": "FAILURE"}));
        assert_eq!(
            (untyped.name.as_str(), untyped.state),
            ("lint", CheckState::Failing)
        );
        // Neither shape, nulls, and a page that is not https: unknown, unnamed, no page.
        let odd = check_of(&serde_json::json!({
            "__typename": "SomethingNew", "name": null, "status": null,
            "detailsUrl": "javascript:alert(1)"
        }));
        assert_eq!(odd.name, "Check");
        assert_eq!(odd.state, CheckState::Unknown);
        assert_eq!(odd.url, None);
        assert_eq!(check_of(&serde_json::json!(42)).state, CheckState::Unknown);
        // One odd item does not spoil the others.
        let v = view(
            r#"{"number": 3, "statusCheckRollup": [7, null, {"__typename": "CheckRun",
                "name": "ok", "status": "COMPLETED", "conclusion": "SUCCESS"}]}"#,
        );
        let states: Vec<CheckState> = v.checks.iter().map(|c| c.state).collect();
        assert_eq!(
            states,
            [
                CheckState::Unknown,
                CheckState::Unknown,
                CheckState::Passing
            ]
        );
    }

    /// Empty lists, missing and `null` fields read as nothing there; only output that is not
    /// a JSON object fails, with a plain message.
    #[test]
    fn missing_fields_read_as_nothing_and_bad_json_fails_plainly() {
        let v = view(r#"{"number": 9}"#);
        assert_eq!(v.state, PrLifecycle::Unknown);
        assert_eq!(v.merge, MergeStatus::Unknown);
        assert_eq!(v.review, ReviewDecision::None);
        assert!(v.checks.is_empty() && v.comments.is_empty() && v.reviews.is_empty());
        let v = view(
            r#"{"number": 9, "state": null, "isDraft": null, "statusCheckRollup": null,
                "comments": null, "reviews": [], "reviewDecision": null, "url": null}"#,
        );
        assert!(v.checks.is_empty());
        assert_eq!(v.url, "");
        for bad in [&b"not json"[..], b"[]", b"null", b"", b"{\"number\": 1"] {
            let err = parse_view(bad).expect_err("fails");
            assert!(err.starts_with("gh's answer"), "{err}");
        }
        assert!(parse_threads(b"<html>").is_err());
    }

    #[test]
    fn review_decisions_read() {
        for (said, want) in [
            ("APPROVED", ReviewDecision::Approved),
            ("CHANGES_REQUESTED", ReviewDecision::ChangesRequested),
            ("REVIEW_REQUIRED", ReviewDecision::ReviewRequired),
            ("", ReviewDecision::None),
            ("SOMETHING_NEW", ReviewDecision::None),
        ] {
            let v = view(&format!(r#"{{"reviewDecision": "{said}"}}"#));
            assert_eq!(v.review, want, "{said}");
        }
    }

    /// Bots: `[bot]` at the end of the login, or GraphQL's `Bot` for that login anywhere in
    /// its answer; `gh pr view` writes CodeRabbit as `coderabbitai`, a plain login.
    #[test]
    fn bots_are_told_by_suffix_or_by_graphql() {
        let bots: HashSet<String> = ["coderabbitai".to_owned()].into();
        assert!(is_bot("coderabbitai", &bots));
        assert!(is_bot("CodeRabbitAI", &bots));
        assert!(is_bot("coderabbitai[bot]", &bots));
        assert!(is_bot("github-actions[bot]", &HashSet::new()));
        assert!(!is_bot("coderabbitai", &HashSet::new()));
        assert!(!is_bot("KhanShaheb34", &bots));
        assert!(!is_bot("robot", &bots));
        // In the captured answers, the conversation comment and the review summary are
        // CodeRabbit's (a bot, by the thread's typename), the reply is the owner's.
        let details = assemble(
            parse_view(VIEW_FINISHED).unwrap(),
            parse_threads(THREADS),
            0,
        );
        let who: Vec<(CommentKind, &str, bool)> = details
            .comments
            .iter()
            .map(|c| (c.kind, c.author.as_str(), c.is_bot))
            .collect();
        assert_eq!(
            who,
            [
                (CommentKind::Comment, "coderabbitai", true),
                (CommentKind::Thread, "coderabbitai", true),
                (CommentKind::Review, "coderabbitai", true),
            ]
        );
        let reply = &details.comments[1].thread.as_ref().unwrap().replies[0];
        assert_eq!(
            (reply.author.as_str(), reply.is_bot),
            ("KhanShaheb34", false)
        );
        // A bot the threads do not name, without the suffix: a person, as far as gg can tell.
        let alone = assemble(
            parse_view(VIEW_FINISHED).unwrap(),
            Err("could not".into()),
            0,
        );
        assert!(!alone.comments[0].is_bot);
        // A bot login's [bot] is left out of the name shown.
        let v = view(
            r#"{"comments": [{"id": "c", "author": {"login": "dependabot[bot]"}, "body": "x",
                "createdAt": "2026-11-24T12:00:00Z"}]}"#,
        );
        let d = assemble(v, Ok(Threads::default()), 0);
        assert_eq!(
            (d.comments[0].author.as_str(), d.comments[0].is_bot),
            ("dependabot", true)
        );
    }

    /// One list, oldest first: the conversation comment, the thread, the review summary (its
    /// empty sibling dropped); the thread with its path, line, state and reply; each with
    /// its page.
    #[test]
    fn comments_reviews_and_threads_make_one_list() {
        let d = assemble(
            parse_view(VIEW_FINISHED).unwrap(),
            parse_threads(THREADS),
            77,
        );
        assert_eq!(d.loaded_at_ms, 77);
        assert_eq!(d.threads_error, None);
        assert!(!d.threads_partial);
        assert_eq!(d.comments.len(), 3, "the empty review summary is dropped");
        let times: Vec<i64> = d
            .comments
            .iter()
            .map(|c| c.created_at_ms.unwrap())
            .collect();
        assert!(times.windows(2).all(|w| w[0] <= w[1]), "{times:?}");
        let thread = &d.comments[1];
        assert_eq!(thread.kind, CommentKind::Thread);
        assert!(
            thread
                .url
                .starts_with("https://github.com/KhanShaheb34/gg/pull/22#discussion_r")
        );
        let t = thread.thread.as_ref().unwrap();
        assert_eq!(t.path, "src/lib/components/Toolbar.svelte");
        assert_eq!(t.line, Some(274));
        assert!(t.resolved && !t.outdated);
        assert_eq!(t.replies.len(), 1);
        let review = &d.comments[2];
        assert_eq!(review.kind, CommentKind::Review);
        assert_eq!(review.review_state, Some(ReviewState::Commented));
        // Its page: GraphQL's, else the pull request's.
        assert_eq!(
            review.url,
            "https://github.com/KhanShaheb34/gg/pull/22#pullrequestreview-1"
        );
        assert!(d.comments[0].url.contains("#issuecomment-"));
        let no_threads = assemble(
            parse_view(VIEW_FINISHED).unwrap(),
            Ok(Threads::default()),
            0,
        );
        assert_eq!(
            no_threads.comments[1].url,
            "https://github.com/KhanShaheb34/gg/pull/22"
        );
    }

    /// The threads failed, the rest is shown and says why they are missing; a partial list
    /// is said; GitHub's own error is passed on.
    #[test]
    fn threads_failing_leaves_the_rest() {
        let d = assemble(
            parse_view(VIEW_FINISHED).unwrap(),
            Err("GraphQL: API rate limit exceeded".into()),
            0,
        );
        assert_eq!(
            d.threads_error.as_deref(),
            Some("GraphQL: API rate limit exceeded")
        );
        assert_eq!(d.comments.len(), 2);
        assert_eq!(d.checks.len(), 4);
        let err = parse_threads(
            br#"{"errors": [{"type": "NOT_FOUND", "message": "Could not resolve to a PullRequest with the number of 999."}], "data": {"repository": {"pullRequest": null}}}"#,
        )
        .unwrap_err();
        assert!(err.contains("Could not resolve"), "{err}");
        assert_eq!(
            parse_threads(br#"{"data": null}"#).unwrap_err(),
            "GitHub did not give the review threads."
        );
        let partial = parse_threads(
            br#"{"data": {"repository": {"pullRequest": {"reviewThreads": {"pageInfo": {"hasNextPage": true}, "nodes": [{"path": "a", "comments": {"nodes": []}}, {"path": "b", "originalLine": 4, "isOutdated": true, "comments": {"nodes": [{"id": "x", "body": "old", "author": null}]}}]}}}}}"#,
        )
        .unwrap();
        let v = view(r#"{"number": 1, "url": "https://github.com/o/r/pull/1"}"#);
        let d = assemble(v, Ok(partial), 0);
        assert!(d.threads_partial);
        assert_eq!(d.comments.len(), 1, "a thread without comments is left out");
        let t = d.comments[0].thread.as_ref().unwrap();
        assert_eq!((t.line, t.outdated), (Some(4), true));
        assert_eq!(d.comments[0].author, "ghost");
        assert_eq!(d.comments[0].url, "https://github.com/o/r/pull/1");
    }

    /// Past the limit, the newest are kept and the rest counted.
    #[test]
    fn only_the_newest_comments_are_kept() {
        let comments: Vec<Value> = (0..MAX_COMMENTS + 5)
            .map(|i| {
                serde_json::json!({
                    "id": format!("c{i}"), "author": {"login": "ada"}, "body": "hi",
                    "createdAt": format!("2026-01-01T00:{:02}:{:02}Z", i / 60, i % 60),
                })
            })
            .collect();
        let json = serde_json::json!({"number": 1, "comments": comments}).to_string();
        let d = assemble(view(&json), Ok(Threads::default()), 0);
        assert_eq!(d.comments.len(), MAX_COMMENTS);
        assert_eq!(d.comments_omitted, 5);
        assert_eq!(d.comments[0].id, "c5");
    }

    /// What the UI reads: camelCase fields, the states as the UI's types spell them.
    #[test]
    fn details_serialize_as_the_ui_reads_them() {
        let d = assemble(
            parse_view(VIEW_FINISHED).unwrap(),
            parse_threads(THREADS),
            5,
        );
        let v = serde_json::to_value(&d).unwrap();
        assert_eq!(v["state"], "merged");
        assert_eq!(v["merge"], "unknown");
        assert_eq!(v["review"], "none");
        assert_eq!(v["loadedAtMs"], 5);
        assert_eq!(v["checksSummary"]["passing"], 3);
        assert_eq!(v["checks"][0]["state"], "passing");
        assert_eq!(v["checks"][0]["workflow"], "CI");
        assert!(v["checks"][0]["startedAtMs"].is_i64());
        assert_eq!(v["comments"][0]["kind"], "comment");
        assert_eq!(v["comments"][0]["isBot"], true);
        assert_eq!(v["comments"][1]["thread"]["resolved"], true);
        assert_eq!(
            v["comments"][1]["thread"]["replies"][0]["author"],
            "KhanShaheb34"
        );
        assert_eq!(v["comments"][2]["reviewState"], "commented");
        assert_eq!(v["threadsError"], Value::Null);
        assert_eq!(v["threadsPartial"], false);
        assert_eq!(v["commentsOmitted"], 0);
        for (status, said) in [
            (MergeStatus::Draft, "draft"),
            (MergeStatus::Conflicts, "conflicts"),
            (MergeStatus::Behind, "behind"),
            (MergeStatus::Blocked, "blocked"),
            (MergeStatus::Unstable, "unstable"),
            (MergeStatus::Ready, "ready"),
        ] {
            assert_eq!(serde_json::to_value(status).unwrap(), said);
        }
        assert_eq!(
            serde_json::to_value(ReviewDecision::ChangesRequested).unwrap(),
            "changes_requested"
        );
    }

    #[test]
    fn only_https_pages_are_kept() {
        assert_eq!(
            https_url(Some("https://github.com/o/r")).as_deref(),
            Some("https://github.com/o/r")
        );
        for bad in [
            "http://github.com/o/r",
            "javascript:alert(1)",
            "https://",
            "https://a b",
            "data:text/html,x",
            "",
        ] {
            assert_eq!(https_url(Some(bad)), None, "{bad}");
        }
        assert_eq!(
            https_url(Some(&format!("https://h/{}", "a".repeat(3000)))),
            None
        );
    }
}
