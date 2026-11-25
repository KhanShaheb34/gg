//! The in-memory state of one open repository: its refs, worktrees and status, the commits
//! read so far with their layout, and everything derived from them (labels, rows, search).
//! Pure and synchronous: no git, no I/O, no async.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::dto::{
    BranchTip, CommitRow, FetchState, HeadInfo, InProgress, PrState, PullRequest, RefKind,
    RefLabel, RepoSnapshot, RowKind, ViewOptions, WorktreeInfo, WorktreeStatus,
};
use crate::git::log::{Commit, LOG_FORMAT, log_args};
use crate::git::refs::RefInfo;
use crate::git::stash::Stash;
use crate::graph::{LaneAllocator, RowLayout};

/// Hash of the synthetic row that stands for the active worktree's uncommitted changes.
pub const UNCOMMITTED: &str = "UNCOMMITTED";

/// Most rows one `rows()` call returns.
pub const MAX_PAGE: u32 = 1000;

/// What a refresh reads from git, apart from the log.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoInfo {
    /// HEAD of the active worktree.
    pub head: HeadInfo,
    pub refs: Vec<RefInfo>,
    pub remotes: Vec<String>,
    /// Paths canonicalized like the repository's root path.
    pub worktrees: Vec<WorktreeInfo>,
    /// Status of the active worktree.
    pub status: WorktreeStatus,
    /// What git is stopped in, in the active worktree; read with the status.
    pub in_progress: Option<InProgress>,
    /// The stashes, newest first.
    pub stashes: Vec<Stash>,
    /// The open pull request of each branch label that has one, by kind and short name (the
    /// repository's own to fill in from what `gh` listed; not read from git).
    pub pr_marks: BTreeMap<(RefKind, String), PullRequest>,
    /// Each remote's default branch, as its `HEAD` symref names it (`origin` → `main`).
    pub remote_heads: BTreeMap<String, String>,
}

/// What [`RepoState::update`] changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Update {
    Nothing,
    /// Only what git is stopped in, or the working tree's change counts with the uncommitted
    /// row where it was: no row moves, so no new generation. (The uncommitted row's subject
    /// names the count: rows read later carry the new one, and the UI shows the snapshot's
    /// `uncommittedSubject`.)
    StateOnly,
    /// Something the rows show (labels, the uncommitted row) may have changed.
    Rows,
}

/// The fixed facts about an open repository that a snapshot repeats.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoIdentity {
    pub repo_id: u32,
    pub name: String,
    pub root_path: String,
}

/// Commits, layout and labels of one repository for one set of refs and view options.
#[derive(Debug)]
pub struct RepoState {
    info: RepoInfo,
    view: ViewOptions,
    pub generation: u64,
    /// Commits in log order, each stored once.
    commits: Vec<Commit>,
    /// Layout of every row, the uncommitted row included.
    layouts: Vec<RowLayout>,
    /// Commit hash → position in `commits` (row index minus `row_offset()`).
    index: HashMap<String, u32>,
    allocator: LaneAllocator,
    /// The parent (HEAD's hash) of the uncommitted row, when there is one.
    uncommitted: Option<String>,
    /// Commit hash → its labels, in display order.
    labels: HashMap<String, Vec<RefLabel>>,
    /// Commit hash → paths of worktrees detached there.
    detached: HashMap<String, Vec<String>>,
    /// Commit of each stash shown → its base: a stash's row has that one parent. With all
    /// branches shown, every stash (they are tips of the log); with a branch selection, the
    /// stashes whose base is a loaded commit, put in memory.
    stash_bases: HashMap<String, String>,
    /// The index and untracked commits of the stashes shown, which never become rows.
    stash_helpers: HashSet<String>,
}

impl RepoState {
    /// A state with no commits loaded yet. `view` is normalized against `info`.
    pub fn new(info: RepoInfo, view: ViewOptions, generation: u64) -> Self {
        let view = normalize_view(view, &info);
        let mut state = Self {
            info,
            view,
            generation,
            commits: Vec::new(),
            layouts: Vec::new(),
            index: HashMap::new(),
            allocator: LaneAllocator::new(),
            uncommitted: None,
            labels: HashMap::new(),
            detached: HashMap::new(),
            stash_bases: HashMap::new(),
            stash_helpers: HashSet::new(),
        };
        state.rebuild_stashes();
        state.relayout();
        state.rebuild_labels();
        state
    }

    pub fn info(&self) -> &RepoInfo {
        &self.info
    }

    pub fn view(&self) -> &ViewOptions {
        &self.view
    }

    /// Arguments of the `git log` that lists this state's commits: with all branches shown,
    /// the stashes are tips too, by commit (their index and untracked commits come with
    /// them, and [`RepoState::append`] leaves those out).
    pub fn log_args(&self) -> Vec<String> {
        let mut args = log_args(&self.view, &names(&self.info.refs, RefKind::Remote));
        let stashes = tip_stashes(&self.info, &self.view);
        let end = args.len() - 1; // before the final `--`
        args.splice(end..end, stashes.iter().map(|s| s.hash.clone()));
        args
    }

    /// Whether the view selects any commit at all (an empty repository selects none).
    pub fn has_tips(&self) -> bool {
        !tips(&self.info, &self.view).is_empty() || !tip_stashes(&self.info, &self.view).is_empty()
    }

    /// Appends commits read from the log (newest first) and lays them out. Returns `true`
    /// when this inserted the uncommitted row at index 0 (HEAD's commit arrived in a view
    /// that does not select it directly), shifting every row index. A stash's commit gets
    /// its base as its only parent; its index and untracked commits are left out.
    pub fn append(&mut self, commits: Vec<Commit>) -> bool {
        self.commits.reserve(commits.len());
        self.layouts.reserve(commits.len());
        for mut commit in commits {
            if self.index.contains_key(&commit.hash) || self.stash_helpers.contains(&commit.hash) {
                continue; // git lists each commit once; never index one twice
            }
            if let Some(base) = self.stash_bases.get(&commit.hash) {
                commit.parents = vec![base.clone()];
            }
            self.layouts
                .push(self.allocator.push(&commit.hash, &commit.parents));
            self.index
                .insert(commit.hash.clone(), self.commits.len() as u32);
            self.commits.push(commit);
        }
        if self.uncommitted.is_none() && self.wanted_uncommitted().is_some() {
            self.relayout();
            return true;
        }
        false
    }

    /// Replaces refs, worktrees, status, in-progress state and view options (normalized
    /// against the new refs), keeping the commits. Returns what changed; only
    /// [`Update::Rows`] can change the rows (the caller decides whether that needs a new
    /// generation).
    pub fn update(&mut self, info: RepoInfo, view: ViewOptions) -> Update {
        let view = normalize_view(view, &info);
        if info == self.info && view == self.view {
            return Update::Nothing;
        }
        let rows_same = view == self.view
            && info.head == self.info.head
            && info.refs == self.info.refs
            && info.remotes == self.info.remotes
            && info.pr_marks == self.info.pr_marks
            && same_worktree_labels(&info.worktrees, &self.info.worktrees)
            && self.stashes_shown(&info, &view) == self.stashes_shown(&self.info, &self.view);
        self.info = info;
        self.view = view;
        if rows_same && self.wanted_uncommitted() == self.uncommitted {
            return Update::StateOnly;
        }
        let old_bases = std::mem::take(&mut self.stash_bases);
        self.rebuild_stashes();
        let stash_rows_change = old_bases.len() != self.stash_bases.len()
            || old_bases.keys().any(|w| !self.stash_bases.contains_key(w));
        if stash_rows_change {
            self.replace_stash_rows(&old_bases);
            self.relayout();
        } else if self.wanted_uncommitted() != self.uncommitted {
            self.relayout();
        }
        self.rebuild_labels();
        Update::Rows
    }

    /// Takes out the rows of the stashes in `old_bases` that are no longer shown, and puts in
    /// rows for the stashes shown now that have none, each above its base, among the rows
    /// as old as it (as `git log --date-order` would list it). [`RepoState::needs_reload`]
    /// has checked that this is all that changes. Call [`RepoState::relayout`] after.
    fn replace_stash_rows(&mut self, old_bases: &HashMap<String, String>) {
        let gone: HashSet<&str> = old_bases
            .keys()
            .filter(|w| !self.stash_bases.contains_key(*w))
            .map(String::as_str)
            .collect();
        self.commits.retain(|c| !gone.contains(c.hash.as_str()));
        let mut index = index_of(&self.commits);
        // Place in the stash list (0 the newest) by stash commit.
        let place: HashMap<&str, usize> = self
            .info
            .stashes
            .iter()
            .enumerate()
            .map(|(n, s)| (s.hash.as_str(), n))
            .collect();
        for (n, st) in self.info.stashes.iter().enumerate() {
            if !self.stash_bases.contains_key(&st.hash) || index.contains_key(&st.hash) {
                continue;
            }
            let Some(&base) = index.get(&st.base) else {
                continue; // not loaded: needs_reload asked for the log instead
            };
            let base = base as usize;
            // `--date-order` goes by committer time; for equal times git keeps the order of
            // the tips it was given, the stashes newest first: above an older stash.
            let at = self.commits[..base]
                .iter()
                .position(|c| {
                    c.commit_time < st.commit_time
                        || (c.commit_time == st.commit_time
                            && place.get(c.hash.as_str()).is_some_and(|&m| m > n))
                })
                .unwrap_or(base);
            self.commits.insert(
                at,
                Commit {
                    hash: st.hash.clone(),
                    parents: vec![st.base.clone()],
                    author: st.author.clone(),
                    email: st.email.clone(),
                    time: st.time,
                    commit_time: st.commit_time,
                    subject: st.subject.clone(),
                },
            );
            index = index_of(&self.commits);
        }
        self.index = index;
    }

    /// Whether moving to `info` and `view` needs the log read again, because the set of
    /// commits reachable from the selected tips may differ from the commits this state has
    /// (or will have, once its load finishes): a selected tip went away, or a new tip is not
    /// a known commit. HEAD needs no rule of its own: with all branches shown it is one of
    /// the tips, and with selected branches it is not part of the log at all.
    ///
    /// Stashes are tips too, but a stash saved, dropped, shown or hidden is put in or taken
    /// out in memory ([`RepoState::update`]) when that is all that changes: a new stash's
    /// base is a loaded commit, and every other loaded commit stays reachable without the
    /// stashes that go. Not while the log is `loading`: its commits are not all there yet.
    pub fn needs_reload(&self, info: &RepoInfo, view: &ViewOptions, loading: bool) -> bool {
        let view = normalize_view(view.clone(), info);
        let old = tips(&self.info, &self.view);
        let new = tips(info, &view);
        // With a branch selection, stashes are no tips: they come and go in memory.
        let old_stashes: BTreeSet<&str> = tip_stashes(&self.info, &self.view)
            .iter()
            .map(|s| s.hash.as_str())
            .collect();
        let new_stashes = tip_stashes(info, &view);
        let known = |hash: &str| {
            old.contains(hash) || old_stashes.contains(hash) || self.index.contains_key(hash)
        };
        if old.iter().any(|t| !new.contains(t)) || new.iter().any(|t| !known(t)) {
            return true;
        }
        let added: Vec<&Stash> = new_stashes
            .iter()
            .filter(|s| !old_stashes.contains(s.hash.as_str()))
            .collect();
        let gone: Vec<&str> = old_stashes
            .iter()
            .copied()
            .filter(|w| !new_stashes.iter().any(|s| s.hash == *w))
            .collect();
        if added.is_empty() && gone.is_empty() {
            return false;
        }
        if loading {
            return true;
        }
        // A stash commit that is also something else (a branch's tip, a loaded commit) has
        // its index and untracked commits as rows, or should have: read the log.
        let odd_new = added.iter().any(|s| {
            self.index.contains_key(&s.hash)
                || new.contains(s.hash.as_str())
                || !self.index.contains_key(&s.base)
        });
        let odd_gone = gone
            .iter()
            .any(|w| new.contains(w) || !self.index.contains_key(*w));
        if odd_new || odd_gone {
            return true;
        }
        if gone.is_empty() {
            return false;
        }
        let starts = new
            .iter()
            .copied()
            .chain(new_stashes.iter().map(|s| s.hash.as_str()))
            .chain(added.iter().map(|s| s.base.as_str()));
        !self.reaches_all_but(starts, &gone)
    }

    /// Whether walking parents from `starts` reaches every loaded commit except those in
    /// `gone`, and none of those.
    fn reaches_all_but<'a>(&self, starts: impl Iterator<Item = &'a str>, gone: &[&str]) -> bool {
        let mut seen = vec![false; self.commits.len()];
        let mut stack: Vec<usize> = starts
            .filter_map(|h| self.index.get(h).map(|&i| i as usize))
            .collect();
        while let Some(i) = stack.pop() {
            if std::mem::replace(&mut seen[i], true) {
                continue;
            }
            for p in &self.commits[i].parents {
                if let Some(&j) = self.index.get(p) {
                    stack.push(j as usize);
                }
            }
        }
        self.commits
            .iter()
            .zip(&seen)
            .all(|(c, &seen)| seen != gone.contains(&c.hash.as_str()))
    }

    pub fn total_rows(&self) -> u32 {
        self.layouts.len() as u32
    }

    /// Whether `hash` is a loaded commit.
    pub fn contains(&self, hash: &str) -> bool {
        self.index.contains_key(hash)
    }

    /// Row index of commit `hash` (or of the uncommitted row for [`UNCOMMITTED`]).
    pub fn find_row(&self, hash: &str) -> Option<u32> {
        if hash == UNCOMMITTED {
            return self.uncommitted.as_ref().map(|_| 0);
        }
        self.index.get(hash).map(|&i| i + self.row_offset())
    }

    /// Rows `start..start + count`, clamped to the rows there are (`count` at most
    /// [`MAX_PAGE`]). Past the end the result is empty.
    pub fn rows(&self, start: u32, count: u32) -> Vec<CommitRow> {
        let total = self.total_rows();
        let start = start.min(total);
        let end = start.saturating_add(count.min(MAX_PAGE)).min(total);
        (start..end).map(|i| self.row(i)).collect()
    }

    /// Ascending row indices of the commits whose subject, author or one of whose ref labels
    /// (as shown in this view) contains `query` (trimmed, case-insensitive), or whose hash
    /// starts with it when it is 4 or more hex digits. An empty query matches nothing.
    /// Message bodies are not held here; see [`RepoState::body_search_args`].
    pub fn search(&self, query: &str) -> Vec<u32> {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return Vec::new();
        }
        let hash_prefix = query.len() >= 4 && query.bytes().all(|b| b.is_ascii_hexdigit());
        let offset = self.row_offset();
        self.commits
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                (hash_prefix && c.hash.starts_with(&query))
                    || contains_lowercase(&c.subject, &query)
                    || contains_lowercase(&c.author, &query)
                    || self.labels.get(&c.hash).is_some_and(|labels| {
                        labels.iter().any(|l| contains_lowercase(&l.name, &query))
                    })
            })
            .map(|(i, _)| i as u32 + offset)
            .collect()
    }

    /// Arguments of a `git log` that prints the hash of every commit of this view whose
    /// message (subject and body) contains `query` (trimmed), ignoring case, as plain text.
    /// `None` for a blank query or a view without commits.
    pub fn body_search_args(&self, query: &str) -> Option<Vec<String>> {
        let query = query.trim();
        if query.is_empty() || !self.has_tips() {
            return None;
        }
        let mut args = self.log_args();
        // The log's own format and order are not needed: hashes only.
        args.retain(|a| a != LOG_FORMAT && a != "--date-order");
        if self.info.head.hash.is_none() {
            // An unborn HEAD names no commit (an orphan branch with other branches).
            args.retain(|a| a != "HEAD");
        }
        let end = args.len() - 1; // before the final `--`
        args.splice(
            end..end,
            [
                "--format=%H".to_owned(),
                "--fixed-strings".to_owned(),
                "--regexp-ignore-case".to_owned(),
                // One argument: a query starting with `-` stays the pattern.
                format!("--grep={query}"),
            ],
        );
        Some(args)
    }

    pub fn snapshot(&self, id: &RepoIdentity, loading: bool, fetch: &FetchState) -> RepoSnapshot {
        let (branches, branch_tips) = branches_with_tips(&self.info.refs, RefKind::Branch);
        let (remote_branches, remote_branch_tips) =
            branches_with_tips(&self.info.refs, RefKind::Remote);
        RepoSnapshot {
            repo_id: id.repo_id,
            name: id.name.clone(),
            root_path: id.root_path.clone(),
            // The repository's own to fill in: which worktree is active is not the rows'.
            active_worktree: String::new(),
            generation: self.generation,
            total_rows: self.total_rows(),
            loading,
            max_lane: self.allocator.max_lane(),
            head: self.info.head.clone(),
            branches,
            remote_branches,
            branch_tips,
            remote_branch_tips,
            remotes: self.info.remotes.clone(),
            worktrees: self.info.worktrees.clone(),
            status: self.info.status.clone(),
            uncommitted_subject: crate::dto::uncommitted_subject(&self.info.status),
            view_options: self.view.clone(),
            fetch: fetch.clone(),
            op: None,
            prs: PrState::default(),
            branch_prs: self
                .info
                .pr_marks
                .iter()
                .filter(|((kind, _), _)| *kind == RefKind::Branch)
                .map(|((_, name), pr)| (name.clone(), pr.clone()))
                .collect(),
            remote_heads: self.info.remote_heads.clone(),
            in_progress: self.info.in_progress.clone(),
            seq: 0,
        }
    }

    /// 1 when the uncommitted row is row 0, else 0.
    fn row_offset(&self) -> u32 {
        u32::from(self.uncommitted.is_some())
    }

    fn row(&self, i: u32) -> CommitRow {
        let layout = &self.layouts[i as usize];
        let offset = self.row_offset();
        if i < offset {
            return CommitRow {
                index: i,
                kind: RowKind::Uncommitted,
                hash: UNCOMMITTED.to_owned(),
                parents: self.uncommitted.iter().cloned().collect(),
                author: String::new(),
                email: String::new(),
                time: 0,
                subject: crate::dto::uncommitted_subject(&self.info.status),
                lane: layout.lane,
                color: layout.color,
                edges: layout.edges.clone(),
                labels: Vec::new(),
                is_head: false,
                is_merge: false,
                detached_worktrees: Vec::new(),
            };
        }
        let c = &self.commits[(i - offset) as usize];
        CommitRow {
            index: i,
            kind: if self.stash_bases.contains_key(&c.hash) {
                RowKind::Stash
            } else {
                RowKind::Commit
            },
            hash: c.hash.clone(),
            parents: c.parents.clone(),
            author: c.author.clone(),
            email: c.email.clone(),
            time: c.time,
            subject: c.subject.clone(),
            lane: layout.lane,
            color: layout.color,
            edges: layout.edges.clone(),
            labels: self.labels.get(&c.hash).cloned().unwrap_or_default(),
            is_head: self.info.head.hash.as_deref() == Some(c.hash.as_str()),
            is_merge: c.parents.len() > 1,
            detached_worktrees: self.detached.get(&c.hash).cloned().unwrap_or_default(),
        }
    }

    /// Lays out every row again from scratch: the uncommitted row (if any), then the
    /// commits. Needed when the uncommitted row appears, disappears or changes parent.
    fn relayout(&mut self) {
        self.uncommitted = self.wanted_uncommitted();
        self.allocator = LaneAllocator::new();
        self.layouts.clear();
        self.layouts
            .reserve(self.commits.len() + usize::from(self.uncommitted.is_some()));
        if let Some(head) = &self.uncommitted {
            self.layouts
                .push(self.allocator.push(UNCOMMITTED, std::slice::from_ref(head)));
        }
        for c in &self.commits {
            self.layouts.push(self.allocator.push(&c.hash, &c.parents));
        }
    }

    /// The parent of the uncommitted row, if there should be one: the active worktree has
    /// changes, HEAD has a commit, and that commit belongs to the view (all branches are
    /// shown, HEAD is a selected tip, or its commit has been loaded). Otherwise the row
    /// would draw a line to a commit that never comes.
    fn wanted_uncommitted(&self) -> Option<String> {
        let head = uncommitted_parent(&self.info)?;
        let in_view = self.view.branches.is_none()
            || self.index.contains_key(&head)
            || tips(&self.info, &self.view).contains(head.as_str());
        in_view.then_some(head)
    }

    /// Puts in the rows of the stashes a branch selection shows now that its log has been
    /// read whole: those whose base is one of its commits. Returns whether the rows
    /// changed. With all branches shown the stashes came with the log: nothing to do.
    pub fn log_read(&mut self) -> bool {
        if self.view.branches.is_none() {
            return false;
        }
        let wanted = self.stashes_shown(&self.info, &self.view);
        let had: BTreeSet<&str> = self.stash_bases.keys().map(String::as_str).collect();
        if wanted
            .iter()
            .map(|s| s.hash.as_str())
            .collect::<BTreeSet<_>>()
            == had
        {
            return false;
        }
        let old_bases = std::mem::take(&mut self.stash_bases);
        self.rebuild_stashes();
        self.replace_stash_rows(&old_bases);
        self.relayout();
        self.rebuild_labels();
        true
    }

    /// The stashes shown with `info` and `view`, given the commits loaded: with all branches,
    /// every stash (when stashes are shown); with a branch selection, those whose base is a
    /// loaded commit (and whose own commit is no other row).
    fn stashes_shown<'a>(&self, info: &'a RepoInfo, view: &ViewOptions) -> Vec<&'a Stash> {
        if !view.show_stashes {
            return Vec::new();
        }
        if view.branches.is_none() {
            return info.stashes.iter().collect();
        }
        info.stashes
            .iter()
            .filter(|s| {
                self.index.contains_key(&s.base)
                    && (!self.index.contains_key(&s.hash) || self.stash_bases.contains_key(&s.hash))
            })
            .collect()
    }

    /// The stashes' commits and helper commits, for the stashes this view shows.
    fn rebuild_stashes(&mut self) {
        let shown = self.stashes_shown(&self.info, &self.view);
        let bases: HashMap<String, String> = shown
            .iter()
            .map(|s| (s.hash.clone(), s.base.clone()))
            .collect();
        let helpers: HashSet<String> = shown
            .iter()
            .flat_map(|s| s.index_parent.iter().chain(&s.untracked_parent))
            .cloned()
            .collect();
        self.stash_bases = bases;
        self.stash_helpers = helpers;
    }

    fn rebuild_labels(&mut self) {
        self.labels = build_labels(&self.info, &self.view, &self.stash_bases);
        self.detached.clear();
        for wt in &self.info.worktrees {
            if let (None, Some(head)) = (&wt.branch, &wt.head) {
                self.detached
                    .entry(head.clone())
                    .or_default()
                    .push(wt.path.clone());
            }
        }
    }
}

/// Whether the worktrees `a` and `b` give the same labels: the same paths with the same
/// branches and commits (whether one is locked or prunable shows nowhere in the rows).
fn same_worktree_labels(a: &[WorktreeInfo], b: &[WorktreeInfo]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(a, b)| a.path == b.path && a.head == b.head && a.branch == b.branch)
}

/// Drops selected branch names that no longer exist; an empty selection becomes `None`
/// (show all).
pub fn normalize_view(mut view: ViewOptions, info: &RepoInfo) -> ViewOptions {
    if let Some(selected) = &mut view.branches {
        selected.retain(|name| {
            info.refs
                .iter()
                .any(|r| r.kind != RefKind::Tag && r.name == *name)
        });
        if selected.is_empty() {
            view.branches = None;
        }
    }
    view
}

/// The stashes that are tips of the log for `view`: all of them when it shows all branches
/// and stashes, else none (a branch selection shows stashes in memory).
fn tip_stashes<'a>(info: &'a RepoInfo, view: &ViewOptions) -> &'a [Stash] {
    if view.branches.is_none() && view.show_stashes {
        &info.stashes
    } else {
        &[]
    }
}

/// Commit hash → position, for `commits`.
fn index_of(commits: &[Commit]) -> HashMap<String, u32> {
    commits
        .iter()
        .enumerate()
        .map(|(i, c)| (c.hash.clone(), i as u32))
        .collect()
}

/// HEAD's hash when the active worktree has changes to show as the uncommitted row.
fn uncommitted_parent(info: &RepoInfo) -> Option<String> {
    let s = &info.status;
    let dirty = s.changed > 0 || s.untracked > 0 || s.conflicted > 0;
    dirty.then(|| info.head.hash.clone()).flatten()
}

/// Short names of the refs of `kind`, in `refs` order.
fn names(refs: &[RefInfo], kind: RefKind) -> Vec<String> {
    refs.iter()
        .filter(|r| r.kind == kind)
        .map(|r| r.name.clone())
        .collect()
}

/// Short names of the refs of `kind`, sorted, and the commit each points at (in the same
/// order).
fn branches_with_tips(refs: &[RefInfo], kind: RefKind) -> (Vec<String>, Vec<BranchTip>) {
    let mut of_kind: Vec<&RefInfo> = refs.iter().filter(|r| r.kind == kind).collect();
    // git lists them sorted by full name already: this costs nothing then.
    of_kind.sort_by(|a, b| a.name.cmp(&b.name));
    of_kind
        .into_iter()
        .map(|r| {
            let tip = BranchTip {
                hash: r.hash.clone(),
                time: r.time,
                author: r.author.clone(),
                subject: r.subject.clone(),
            };
            (r.name.clone(), tip)
        })
        .unzip()
}

/// Distinct commits the log starts from, resolved the way [`log_args`] resolves them.
fn tips<'a>(info: &'a RepoInfo, view: &ViewOptions) -> BTreeSet<&'a str> {
    let hash_of = |kind: RefKind, name: &str| {
        info.refs
            .iter()
            .find(|r| r.kind == kind && r.name == name)
            .map(|r| r.hash.as_str())
    };
    match &view.branches {
        None => info
            .refs
            .iter()
            .filter(|r| match r.kind {
                RefKind::Branch => true,
                RefKind::Remote => view.show_remotes,
                RefKind::Tag => view.show_tags,
                RefKind::Stash => false,
            })
            .map(|r| r.hash.as_str())
            .chain(info.head.hash.as_deref())
            .collect(),
        Some(selected) => selected
            .iter()
            .filter_map(|name| {
                hash_of(RefKind::Remote, name).or_else(|| hash_of(RefKind::Branch, name))
            })
            .collect(),
    }
}

/// `origin/feature/x` → `("origin", "feature/x")`, using the longest matching remote name
/// (remote names may contain slashes). `None` if no configured remote matches.
fn split_remote<'a>(name: &'a str, remotes: &[String]) -> Option<(&'a str, &'a str)> {
    remotes
        .iter()
        .filter(|r| {
            name.len() > r.len() + 1
                && name.starts_with(r.as_str())
                && name.as_bytes()[r.len()] == b'/'
        })
        .max_by_key(|r| r.len())
        .map(|r| (&name[..r.len()], &name[r.len() + 1..]))
}

/// Labels per commit hash: branches (the current one first, then by name), then remote
/// branches, then tags, by name, then stashes (`stash@{n}`). A branch absorbs the remote branches of the same name at
/// the same commit. Remote and tag labels are left out when the view hides them.
fn build_labels(
    info: &RepoInfo,
    view: &ViewOptions,
    stashes_shown: &HashMap<String, String>,
) -> HashMap<String, Vec<RefLabel>> {
    // (commit, branch name) → remotes whose branch of that name is at that commit.
    let mut remotes_at: HashMap<(&str, &str), Vec<String>> = HashMap::new();
    if view.show_remotes {
        for r in info.refs.iter().filter(|r| r.kind == RefKind::Remote) {
            if let Some((remote, branch)) = split_remote(&r.name, &info.remotes) {
                remotes_at
                    .entry((r.hash.as_str(), branch))
                    .or_default()
                    .push(remote.to_owned());
            }
        }
    }
    let is_local_at = |hash: &str, name: &str| {
        info.refs
            .iter()
            .any(|r| r.kind == RefKind::Branch && r.hash == hash && r.name == name)
    };
    let mut labels: HashMap<String, Vec<RefLabel>> = HashMap::new();
    for r in &info.refs {
        let label = match r.kind {
            RefKind::Branch => {
                let mut remotes = remotes_at
                    .get(&(r.hash.as_str(), r.name.as_str()))
                    .cloned()
                    .unwrap_or_default();
                remotes.sort();
                RefLabel {
                    kind: r.kind,
                    name: r.name.clone(),
                    remotes,
                    current: info.head.branch.as_deref() == Some(r.name.as_str()),
                    worktree_path: info
                        .worktrees
                        .iter()
                        .find(|w| w.branch.as_deref() == Some(r.name.as_str()))
                        .map(|w| w.path.clone()),
                    upstream: r.upstream.clone(),
                    ahead: r.ahead,
                    behind: r.behind,
                    pr: info.pr_marks.get(&(r.kind, r.name.clone())).cloned(),
                }
            }
            RefKind::Remote if view.show_remotes => {
                let absorbed = split_remote(&r.name, &info.remotes)
                    .is_some_and(|(_, branch)| is_local_at(&r.hash, branch));
                if absorbed {
                    continue;
                }
                RefLabel {
                    pr: info.pr_marks.get(&(r.kind, r.name.clone())).cloned(),
                    ..plain_label(r)
                }
            }
            RefKind::Tag if view.show_tags => plain_label(r),
            RefKind::Remote | RefKind::Tag | RefKind::Stash => continue,
        };
        labels.entry(r.hash.clone()).or_default().push(label);
    }
    for st in info
        .stashes
        .iter()
        .filter(|s| stashes_shown.contains_key(&s.hash))
    {
        labels.entry(st.hash.clone()).or_default().push(RefLabel {
            kind: RefKind::Stash,
            name: st.selector.clone(),
            remotes: Vec::new(),
            current: false,
            worktree_path: None,
            upstream: None,
            ahead: 0,
            behind: 0,
            pr: None,
        });
    }
    for list in labels.values_mut() {
        list.sort_by(|a, b| (a.kind, !a.current, &a.name).cmp(&(b.kind, !b.current, &b.name)));
    }
    labels
}

fn plain_label(r: &RefInfo) -> RefLabel {
    RefLabel {
        kind: r.kind,
        name: r.name.clone(),
        remotes: Vec::new(),
        current: false,
        worktree_path: None,
        upstream: None,
        ahead: 0,
        behind: 0,
        pr: None,
    }
}

/// Whether `haystack`, lowercased, contains `needle` (already lowercase), without
/// allocating. ASCII text is compared byte-wise; other text char by char.
fn contains_lowercase(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    if haystack.is_ascii() && needle.is_ascii() {
        return haystack
            .as_bytes()
            .windows(needle.len())
            .any(|w| w.eq_ignore_ascii_case(needle.as_bytes()));
    }
    haystack.char_indices().any(|(i, _)| {
        let mut lowered = haystack[i..].chars().flat_map(char::to_lowercase);
        needle.chars().all(|n| lowered.next() == Some(n))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::EdgeHalf;

    /// A 40-hex-digit hash made of `c`.
    fn h(c: char) -> String {
        c.to_string().repeat(40)
    }

    fn commit(hash: char, parents: &[char], subject: &str, author: &str) -> Commit {
        Commit {
            hash: h(hash),
            parents: parents.iter().map(|&p| h(p)).collect(),
            author: author.into(),
            email: format!("{}@example.com", author.to_lowercase()),
            time: 1_700_000_000,
            commit_time: 1_700_000_000,
            subject: subject.into(),
        }
    }

    /// a ← b ← d (merge of b and c), c ← a: newest first d, c, b, a.
    fn history() -> Vec<Commit> {
        vec![
            commit('d', &['b', 'c'], "Merge feature", "Ada"),
            commit('c', &['a'], "Feature work", "Grace"),
            commit('b', &['a'], "Fix Parser bug", "Ada"),
            commit('a', &[], "Initial ünïcode", "Linus"),
        ]
    }

    fn rf(kind: RefKind, name: &str, hash: char) -> RefInfo {
        RefInfo {
            kind,
            name: name.into(),
            hash: h(hash),
            upstream: None,
            ahead: 0,
            behind: 0,
            time: 0,
            author: String::new(),
            subject: String::new(),
        }
    }

    fn worktree(path: &str, head: char, branch: Option<&str>, is_main: bool) -> WorktreeInfo {
        WorktreeInfo {
            path: path.into(),
            head: Some(h(head)),
            branch: branch.map(str::to_owned),
            is_main,
            ..WorktreeInfo::default()
        }
    }

    fn info() -> RepoInfo {
        RepoInfo {
            head: HeadInfo {
                branch: Some("main".into()),
                hash: Some(h('d')),
            },
            refs: vec![
                rf(RefKind::Branch, "main", 'd'),
                rf(RefKind::Branch, "feature", 'c'),
            ],
            remotes: vec![],
            worktrees: vec![worktree("/repo", 'd', Some("main"), true)],
            status: WorktreeStatus::default(),
            in_progress: None,
            stashes: Vec::new(),
            pr_marks: BTreeMap::new(),
            remote_heads: BTreeMap::new(),
        }
    }

    fn all() -> ViewOptions {
        ViewOptions {
            branches: None,
            show_remotes: true,
            show_tags: true,
            show_stashes: true,
        }
    }

    fn loaded(info: RepoInfo) -> RepoState {
        let mut s = RepoState::new(info, all(), 1);
        s.append(history());
        s
    }

    fn dirty(mut info: RepoInfo, changed: u32, untracked: u32) -> RepoInfo {
        info.status = WorktreeStatus {
            changed,
            untracked,
            conflicted: 0,
            conflicted_paths: Vec::new(),
        };
        info
    }

    fn label_names(s: &RepoState, row: u32) -> Vec<(RefKind, String)> {
        s.rows(row, 1)[0]
            .labels
            .iter()
            .map(|l| (l.kind, l.name.clone()))
            .collect()
    }

    #[test]
    fn rows_carry_commits_and_layout() {
        let s = loaded(info());
        assert_eq!(s.total_rows(), 4);
        let rows = s.rows(0, 10);
        assert_eq!(rows.len(), 4);
        let d = &rows[0];
        assert_eq!(
            (d.index, d.kind, d.hash.as_str()),
            (0, RowKind::Commit, h('d').as_str())
        );
        assert!(d.is_merge && d.is_head);
        assert_eq!(d.parents, [h('b'), h('c')]);
        assert_eq!(d.email, "ada@example.com");
        assert_eq!(d.subject, "Merge feature");
        assert_eq!((d.lane, d.color), (0, 0));
        assert!(
            d.edges
                .iter()
                .any(|e| e.half == EdgeHalf::Bottom && e.to == 1)
        );
        assert!(!rows[1].is_head && !rows[1].is_merge);
        assert_eq!(rows[1].lane, 1, "c is on the merged branch's lane");
        let snap = s.snapshot(&identity(), false, &FetchState::default());
        assert_eq!(snap.max_lane, 1);
    }

    fn identity() -> RepoIdentity {
        RepoIdentity {
            repo_id: 3,
            name: "repo".into(),
            root_path: "/repo".into(),
        }
    }

    #[test]
    fn rows_are_clamped() {
        let s = loaded(info());
        assert_eq!(s.rows(2, 100).len(), 2);
        assert_eq!(s.rows(3, 1)[0].index, 3);
        assert!(s.rows(4, 10).is_empty());
        assert!(s.rows(u32::MAX, u32::MAX).is_empty());
        assert!(s.rows(0, 0).is_empty());
    }

    #[test]
    fn batches_append_like_one() {
        let mut s = RepoState::new(info(), all(), 1);
        let mut commits = history();
        let tail = commits.split_off(2);
        s.append(commits);
        s.append(tail);
        assert_eq!(s.rows(0, 10), loaded(info()).rows(0, 10));
        assert_eq!(s.find_row(&h('a')), Some(3));
        // A repeated commit is ignored rather than indexed twice.
        s.append(vec![commit('a', &[], "again", "X")]);
        assert_eq!(s.total_rows(), 4);
    }

    #[test]
    fn uncommitted_row_comes_first() {
        let s = loaded(dirty(info(), 2, 1));
        assert_eq!(s.total_rows(), 5);
        let rows = s.rows(0, 2);
        assert_eq!(rows[0].kind, RowKind::Uncommitted);
        assert_eq!(rows[0].hash, UNCOMMITTED);
        assert_eq!(rows[0].subject, "Uncommitted changes (3)");
        assert_eq!(rows[0].parents, [h('d')]);
        assert!(!rows[0].is_head && rows[0].labels.is_empty());
        assert_eq!((rows[1].index, rows[1].hash.clone()), (1, h('d')));
        assert_eq!(rows[1].lane, rows[0].lane);
        assert_eq!(s.find_row(&h('d')), Some(1));
        assert_eq!(s.find_row(UNCOMMITTED), Some(0));
        assert_eq!(s.search("parser"), [3]);
    }

    #[test]
    fn conflicts_alone_make_the_tree_dirty_and_unborn_head_never_does() {
        let mut i = info();
        i.status.conflicted = 1;
        assert_eq!(loaded(i).total_rows(), 5);
        let mut i = dirty(info(), 1, 0);
        i.head.hash = None;
        let s = RepoState::new(i, all(), 1);
        assert_eq!(s.total_rows(), 0);
        assert_eq!(s.find_row(UNCOMMITTED), None);
    }

    #[test]
    fn a_change_of_in_progress_state_alone_leaves_the_rows() {
        let mut s = loaded(info());
        let mut stopped = info();
        stopped.in_progress = Some(crate::dto::InProgress {
            kind: crate::dto::InProgressKind::Merge,
            files: Vec::new(),
            file_count: 0,
            step: None,
            total: None,
            subject: None,
            incoming: None,
            rebasing: None,
            commit: None,
            empty: false,
        });
        assert_eq!(s.update(stopped.clone(), all()), Update::StateOnly);
        assert_eq!(s.info().in_progress, stopped.in_progress);
        assert_eq!(s.update(stopped, all()), Update::Nothing);
        assert_eq!(s.update(dirty(info(), 1, 0), all()), Update::Rows);
    }

    #[test]
    fn update_toggles_the_uncommitted_row_and_relayouts() {
        let mut s = loaded(info());
        let clean_rows = s.rows(0, 10);
        assert_eq!(s.update(dirty(info(), 1, 0), all()), Update::Rows);
        assert_eq!(s.total_rows(), 5);
        assert_eq!(s.rows(0, 1)[0].kind, RowKind::Uncommitted);
        assert_eq!(s.rows(0, 10)[1..].len(), 4);
        // Only the subject changes when the count does: no row moves.
        let before = s.rows(0, 10);
        assert_eq!(s.update(dirty(info(), 4, 0), all()), Update::StateOnly);
        let after = s.rows(0, 10);
        assert_eq!(after[0].subject, "Uncommitted changes (4)");
        assert_eq!(after[1..], before[1..]);
        assert_eq!(s.update(info(), all()), Update::Rows);
        assert_eq!(s.rows(0, 10), clean_rows);
        assert_eq!(s.update(info(), all()), Update::Nothing, "nothing changed");
    }

    #[test]
    fn labels_absorb_same_named_remotes_at_the_same_commit() {
        let mut i = info();
        i.remotes = vec!["origin".into(), "up".into(), "up/stream".into()];
        i.refs.extend([
            rf(RefKind::Remote, "up/main", 'd'),
            rf(RefKind::Remote, "origin/main", 'd'),
            rf(RefKind::Remote, "origin/feature", 'b'), // not where `feature` is
            rf(RefKind::Remote, "up/stream/feature", 'c'), // remote `up/stream`
            rf(RefKind::Remote, "gone/main", 'd'),      // no such remote
        ]);
        let s = loaded(i);
        let main = &s.rows(0, 1)[0].labels;
        assert_eq!(main[0].name, "main");
        assert_eq!(main[0].remotes, ["origin", "up"]);
        assert_eq!(
            label_names(&s, 0),
            [
                (RefKind::Branch, "main".into()),
                (RefKind::Remote, "gone/main".into())
            ]
        );
        let feature = &s.rows(1, 1)[0].labels;
        assert_eq!(feature.len(), 1, "{feature:?}");
        assert_eq!(feature[0].remotes, ["up/stream"]);
        assert_eq!(
            label_names(&s, 2),
            [(RefKind::Remote, "origin/feature".into())]
        );
    }

    #[test]
    fn labels_order_and_branch_details() {
        let mut i = info();
        i.refs = vec![
            rf(RefKind::Tag, "v2", 'd'),
            rf(RefKind::Branch, "zeta", 'd'),
            rf(RefKind::Tag, "v1", 'd'),
            RefInfo {
                upstream: Some("origin/main".into()),
                ahead: 2,
                behind: 1,
                ..rf(RefKind::Branch, "main", 'd')
            },
            rf(RefKind::Branch, "alpha", 'd'),
            rf(RefKind::Remote, "origin/x", 'd'),
        ];
        i.remotes = vec!["origin".into()];
        i.worktrees.push(worktree("/wt", 'd', Some("zeta"), false));
        let s = loaded(i);
        assert_eq!(
            label_names(&s, 0),
            [
                (RefKind::Branch, "main".into()),
                (RefKind::Branch, "alpha".into()),
                (RefKind::Branch, "zeta".into()),
                (RefKind::Remote, "origin/x".into()),
                (RefKind::Tag, "v1".into()),
                (RefKind::Tag, "v2".into()),
            ]
        );
        let labels = &s.rows(0, 1)[0].labels;
        assert!(labels[0].current);
        assert_eq!(labels[0].worktree_path.as_deref(), Some("/repo"));
        assert_eq!(labels[0].upstream.as_deref(), Some("origin/main"));
        assert_eq!((labels[0].ahead, labels[0].behind), (2, 1));
        assert!(!labels[1].current && labels[1].worktree_path.is_none());
        assert!(!labels[2].current);
        assert_eq!(labels[2].worktree_path.as_deref(), Some("/wt"));
    }

    #[test]
    fn labels_follow_view_options() {
        let mut i = info();
        i.remotes = vec!["origin".into()];
        i.refs.extend([
            rf(RefKind::Remote, "origin/main", 'd'),
            rf(RefKind::Remote, "origin/old", 'a'),
            rf(RefKind::Tag, "v1", 'a'),
        ]);
        let mut s = loaded(i.clone());
        assert_eq!(s.rows(0, 1)[0].labels[0].remotes, ["origin"]);
        let no_remotes = ViewOptions {
            show_remotes: false,
            ..all()
        };
        assert_eq!(s.update(i.clone(), no_remotes), Update::Rows);
        assert!(s.rows(0, 1)[0].labels[0].remotes.is_empty());
        assert_eq!(label_names(&s, 3), [(RefKind::Tag, "v1".into())]);
        let no_tags = ViewOptions {
            show_tags: false,
            show_stashes: true,
            ..all()
        };
        assert_eq!(s.update(i, no_tags), Update::Rows);
        assert_eq!(label_names(&s, 3), [(RefKind::Remote, "origin/old".into())]);
    }

    #[test]
    fn detached_worktrees_are_listed_on_their_commit() {
        let mut i = info();
        i.worktrees.push(worktree("/wt/one", 'b', None, false));
        i.worktrees.push(worktree("/wt/two", 'b', None, false));
        i.worktrees.push(WorktreeInfo {
            head: None,
            ..worktree("/wt/unborn", 'b', None, false)
        });
        let s = loaded(i);
        assert_eq!(s.rows(2, 1)[0].detached_worktrees, ["/wt/one", "/wt/two"]);
        assert!(s.rows(0, 1)[0].detached_worktrees.is_empty());
    }

    #[test]
    fn search_subject_author_and_hash_prefix() {
        let s = loaded(info());
        assert_eq!(s.search("PARSER"), [2]);
        assert_eq!(s.search("  ada "), [0, 2]);
        assert_eq!(s.search("ÜNÏ"), [3]);
        assert_eq!(s.search(&"c".repeat(6)), [1]);
        assert_eq!(s.search(&"C".repeat(4)), [1]);
        assert!(
            s.search("ccc").is_empty(),
            "3 digits is too short for a hash"
        );
        assert!(s.search("").is_empty());
        assert!(s.search(" \t ").is_empty());
        assert!(s.search("zzz").is_empty());
    }

    #[test]
    fn search_matches_ref_names_shown() {
        let mut i = info();
        i.refs.push(rf(RefKind::Tag, "Release-1", 'a'));
        let s = loaded(i.clone());
        assert_eq!(s.search("release"), [3]);
        assert_eq!(
            s.search("MAIN"),
            [0],
            "branch `main` at d; no subject says it"
        );
        let mut hidden = RepoState::new(
            i,
            ViewOptions {
                show_tags: false,
                show_stashes: true,
                ..all()
            },
            1,
        );
        hidden.append(history());
        assert!(
            hidden.search("release").is_empty(),
            "hidden tags do not match"
        );
    }

    #[test]
    fn body_search_args_put_the_query_in_one_argument_before_the_separator() {
        let s = loaded(info());
        let args = s.body_search_args("  -x [a] ").unwrap();
        assert_eq!(args.first().map(String::as_str), Some("log"));
        assert_eq!(args.last().map(String::as_str), Some("--"));
        assert!(args.contains(&"--grep=-x [a]".to_owned()), "{args:?}");
        assert!(args.contains(&"--fixed-strings".to_owned()));
        assert!(args.contains(&"--regexp-ignore-case".to_owned()));
        assert!(args.contains(&"--format=%H".to_owned()));
        assert!(!args.iter().any(|a| a == LOG_FORMAT));
        assert!(s.body_search_args(" ").is_none());
        let mut unborn = info();
        unborn.head.hash = None;
        let s = RepoState::new(unborn, all(), 1);
        assert!(
            !s.body_search_args("x")
                .unwrap()
                .contains(&"HEAD".to_owned())
        );
    }

    #[test]
    fn contains_lowercase_handles_ascii_and_unicode() {
        assert!(contains_lowercase("Fix Parser", "parser"));
        assert!(contains_lowercase("abc", ""));
        assert!(!contains_lowercase("ab", "abc"));
        assert!(contains_lowercase("Straße ÄRGER", "ärger"));
        assert!(contains_lowercase("ÉCOLE", "école"));
        assert!(!contains_lowercase("école", "ecole"));
        assert!(contains_lowercase("emoji ✨ Here", "✨ here"));
    }

    #[test]
    fn find_row_of_unknown_hash_is_none() {
        let s = loaded(info());
        assert_eq!(s.find_row(&h('c')), Some(1));
        assert_eq!(s.find_row(&h('e')), None);
        assert_eq!(s.find_row(UNCOMMITTED), None);
        assert!(s.contains(&h('a')) && !s.contains(&h('e')));
    }

    #[test]
    fn normalize_view_drops_missing_and_empty_selections() {
        let select = |names: &[&str]| ViewOptions {
            branches: Some(names.iter().map(|n| (*n).to_owned()).collect()),
            ..all()
        };
        let mut i = info();
        i.refs.push(rf(RefKind::Remote, "origin/main", 'd'));
        i.refs.push(rf(RefKind::Tag, "v1", 'd'));
        assert_eq!(normalize_view(select(&[]), &i).branches, None);
        assert_eq!(normalize_view(select(&["gone"]), &i).branches, None);
        assert_eq!(
            normalize_view(select(&["v1"]), &i).branches,
            None,
            "tags are not branches"
        );
        assert_eq!(
            normalize_view(select(&["feature", "gone", "origin/main"]), &i).branches,
            Some(vec!["feature".into(), "origin/main".into()])
        );
        assert_eq!(normalize_view(all(), &i), all());
        let s = RepoState::new(i, select(&["gone"]), 1);
        assert_eq!(s.view().branches, None);
    }

    #[test]
    fn reload_only_when_reachable_commits_may_change() {
        let s = loaded(info());
        let same = info();
        assert!(!s.needs_reload(&same, &all(), false));

        // A new branch at a loaded commit: same commits.
        let mut i = info();
        i.refs.push(rf(RefKind::Branch, "side", 'b'));
        assert!(!s.needs_reload(&i, &all(), false));
        // A tag moved between loaded commits while another ref keeps the old tip.
        let mut i = info();
        i.refs[1].hash = h('a');
        i.refs.push(rf(RefKind::Branch, "keep", 'c'));
        assert!(!s.needs_reload(&i, &all(), false));

        // A new commit.
        let mut i = info();
        i.refs[0].hash = h('e');
        i.head.hash = Some(h('e'));
        assert!(s.needs_reload(&i, &all(), false));
        // A branch deleted: its commits may go.
        let mut i = info();
        i.refs.remove(1);
        assert!(s.needs_reload(&i, &all(), false));
        // Showing fewer refs drops tips.
        let mut i = info();
        i.refs.push(rf(RefKind::Tag, "only-tag", 'f'));
        let with_tag = RepoState::new(i.clone(), all(), 1);
        let no_tags = ViewOptions {
            show_tags: false,
            show_stashes: true,
            ..all()
        };
        assert!(with_tag.needs_reload(&i, &no_tags, false));
        // Selecting one branch.
        let feature = ViewOptions {
            branches: Some(vec!["feature".into()]),
            ..all()
        };
        assert!(s.needs_reload(&info(), &feature, false));
    }

    #[test]
    fn head_outside_the_selection_never_reloads() {
        let feature = feature_view();
        let mut s = RepoState::new(info(), feature.clone(), 1);
        s.append(feature_log());
        let mut i = info();
        i.head.hash = Some(h('a'));
        assert!(
            !s.needs_reload(&i, &feature, false),
            "HEAD moved to a loaded commit"
        );
        // HEAD is not part of a selected-branches log, so where it goes cannot matter.
        i.head.hash = Some(h('b'));
        assert!(
            !s.needs_reload(&i, &feature, false),
            "HEAD moved outside the selection"
        );
        i.head.hash = Some(h('e'));
        assert!(
            !s.needs_reload(&i, &feature, false),
            "HEAD moved to a new commit"
        );
    }

    fn feature_view() -> ViewOptions {
        ViewOptions {
            branches: Some(vec!["feature".into()]),
            ..all()
        }
    }

    /// The log of `feature` in `history()`: c, a.
    fn feature_log() -> Vec<Commit> {
        vec![
            commit('c', &['a'], "Feature work", "Grace"),
            commit('a', &[], "Initial", "Linus"),
        ]
    }

    #[test]
    fn uncommitted_row_is_hidden_when_head_is_outside_the_view() {
        // HEAD (d, on main) is dirty but the view shows only `feature` (c, a).
        let mut s = RepoState::new(dirty(info(), 1, 0), feature_view(), 1);
        assert!(!s.append(feature_log()));
        let mut clean = RepoState::new(info(), feature_view(), 1);
        clean.append(feature_log());
        assert_eq!(s.total_rows(), 2);
        assert_eq!(s.rows(0, 10), clean.rows(0, 10));
        assert_eq!(s.find_row(UNCOMMITTED), None);
        let fetch = FetchState::default();
        assert_eq!(s.snapshot(&identity(), false, &fetch).max_lane, 0);
        // Going clean and dirty again changes nothing either.
        assert_eq!(s.update(info(), feature_view()), Update::StateOnly);
        assert_eq!(
            s.update(dirty(info(), 2, 0), feature_view()),
            Update::StateOnly
        );
        assert_eq!(s.rows(0, 10), clean.rows(0, 10));
    }

    #[test]
    fn uncommitted_row_shows_when_head_is_a_selected_tip() {
        let mut i = dirty(info(), 1, 0);
        i.head = HeadInfo {
            branch: Some("feature".into()),
            hash: Some(h('c')),
        };
        let mut s = RepoState::new(i, feature_view(), 1);
        assert_eq!(s.total_rows(), 1, "before any commit arrives");
        assert!(!s.append(feature_log()), "row 0 was there from the start");
        assert_eq!(s.rows(0, 1)[0].kind, RowKind::Uncommitted);
        assert_eq!(s.total_rows(), 3);
    }

    #[test]
    fn uncommitted_row_appears_when_head_commit_streams_in() {
        // HEAD is detached at `a`, which `feature` reaches but does not point at.
        let mut i = dirty(info(), 1, 0);
        i.head = HeadInfo {
            branch: None,
            hash: Some(h('a')),
        };
        let mut s = RepoState::new(i, feature_view(), 1);
        let mut log = feature_log();
        let a = log.pop().unwrap();
        assert!(!s.append(log));
        assert_eq!(s.total_rows(), 1);
        assert!(s.append(vec![a]), "row 0 inserted: indices shifted");
        let rows = s.rows(0, 10);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].kind, RowKind::Uncommitted);
        assert_eq!(rows[0].parents, [h('a')]);
        assert_eq!(s.find_row(&h('a')), Some(2));
        // The same layout as if row 0 had been there from the start.
        let mut whole = RepoState::new(s.info().clone(), feature_view(), 1);
        whole.append(feature_log());
        assert_eq!(whole.rows(0, 10), rows);
    }

    #[test]
    fn pending_state_with_same_tips_needs_no_reload() {
        let pending = RepoState::new(info(), all(), 1);
        assert!(!pending.needs_reload(&info(), &all(), false));
        let mut i = dirty(info(), 1, 0);
        i.refs.push(rf(RefKind::Branch, "same-tip", 'd'));
        assert!(!pending.needs_reload(&i, &all(), false));
        let mut i = info();
        i.refs.push(rf(RefKind::Branch, "new-tip", 'b'));
        assert!(
            pending.needs_reload(&i, &all(), false),
            "b is not known to be reachable yet"
        );
    }

    #[test]
    fn has_tips_and_log_args() {
        let empty = RepoInfo {
            head: HeadInfo {
                branch: Some("main".into()),
                hash: None,
            },
            ..RepoInfo::default()
        };
        assert!(!RepoState::new(empty, all(), 1).has_tips());
        let mut i = info();
        i.refs.push(rf(RefKind::Remote, "origin/dev", 'c'));
        let s = RepoState::new(
            i,
            ViewOptions {
                branches: Some(vec!["origin/dev".into()]),
                ..all()
            },
            1,
        );
        assert!(s.has_tips());
        assert!(s.log_args().contains(&"refs/remotes/origin/dev".to_owned()));
    }

    /// Stash `w` (`stash@{n}`) on `base`, with index commit `i` and untracked commit `u`.
    fn stash(w: char, n: u32, base: char, i: char, u: Option<char>) -> Stash {
        Stash {
            hash: h(w),
            selector: format!("stash@{{{n}}}"),
            base: h(base),
            index_parent: Some(h(i)),
            untracked_parent: u.map(h),
            author: "Ada".into(),
            email: "ada@example.com".into(),
            time: 1_700_000_100,
            commit_time: 1_700_000_100,
            subject: format!("WIP on main: stash {w}"),
        }
    }

    /// The commit git lists for stash `s`, with all its parents, and its helpers: the index
    /// commit (on the base) and the untracked commit (a root).
    fn stash_commits(s: &Stash) -> Vec<Commit> {
        let mut w = Commit {
            hash: s.hash.clone(),
            parents: vec![s.base.clone()],
            author: s.author.clone(),
            email: s.email.clone(),
            time: s.time,
            commit_time: s.commit_time,
            subject: s.subject.clone(),
        };
        let mut out = Vec::new();
        let index = s.index_parent.clone().unwrap();
        w.parents.push(index.clone());
        out.push(Commit {
            hash: index,
            parents: vec![s.base.clone()],
            subject: "index on main".into(),
            ..w.clone()
        });
        if let Some(u) = &s.untracked_parent {
            w.parents.push(u.clone());
            out.push(Commit {
                hash: u.clone(),
                parents: vec![],
                subject: "untracked files on main".into(),
                ..w.clone()
            });
        }
        out.insert(0, w);
        out
    }

    fn with_stashes(stashes: Vec<Stash>) -> RepoInfo {
        RepoInfo { stashes, ..info() }
    }

    fn no_stashes() -> ViewOptions {
        ViewOptions {
            show_stashes: false,
            ..all()
        }
    }

    /// `history()` as git lists it with the stashes of `info` as extra tips (each newer than
    /// every commit), into a state for `view`.
    fn loaded_with(info: RepoInfo, view: ViewOptions) -> RepoState {
        let mut s = RepoState::new(info.clone(), view, 1);
        let mut log: Vec<Commit> = Vec::new();
        if s.view().show_stashes && s.view().branches.is_none() {
            for st in &info.stashes {
                log.extend(stash_commits(st));
            }
        }
        log.extend(history());
        s.append(log);
        s
    }

    #[test]
    fn a_stash_is_one_row_with_one_parent_and_its_label() {
        let st = stash('w', 0, 'd', 'i', Some('u'));
        let s = loaded_with(with_stashes(vec![st.clone()]), all());
        assert_eq!(
            s.total_rows(),
            5,
            "w, d, c, b, a: no index or untracked row"
        );
        let rows = s.rows(0, 10);
        let w = &rows[0];
        assert_eq!((w.kind, w.hash.as_str()), (RowKind::Stash, h('w').as_str()));
        assert_eq!(w.parents, [h('d')]);
        assert!(!w.is_merge && !w.is_head);
        assert_eq!(w.subject, "WIP on main: stash w");
        assert_eq!(w.time, 1_700_000_100);
        assert_eq!(label_names(&s, 0), [(RefKind::Stash, "stash@{0}".into())]);
        assert!(rows[1..].iter().all(|r| r.kind == RowKind::Commit));
        assert_eq!(s.find_row(&h('i')), None);
        assert_eq!(s.find_row(&h('u')), None);
        // One line, from the stash down to its base.
        assert_eq!(
            w.edges,
            vec![crate::graph::Edge {
                from: 0,
                to: 0,
                color: 0,
                half: EdgeHalf::Bottom
            }]
        );
        assert_eq!(rows[1].lane, 0);
        assert_eq!(
            s.snapshot(&identity(), false, &FetchState::default())
                .max_lane,
            1
        );
        // The log lists it, before the separator.
        let args = s.log_args();
        let w_at = args
            .iter()
            .position(|a| *a == h('w'))
            .expect("stash is a tip");
        assert!(w_at < args.len() - 1);
        assert!(s.body_search_args("x").unwrap().contains(&h('w')));
    }

    #[test]
    fn a_stash_on_a_commit_no_branch_has_keeps_that_commit() {
        // e (on a) is on no branch; only the stash leads to it.
        let st = stash('w', 0, 'e', 'i', None);
        let mut s = RepoState::new(with_stashes(vec![st.clone()]), all(), 1);
        let mut log = stash_commits(&st);
        log.push(commit('e', &['a'], "Gone branch", "Ada"));
        log.extend(history());
        s.append(log);
        let hashes: Vec<String> = s.rows(0, 10).into_iter().map(|r| r.hash).collect();
        assert_eq!(hashes, [h('w'), h('e'), h('d'), h('c'), h('b'), h('a')]);
        assert_eq!(s.rows(0, 1)[0].parents, [h('e')]);
    }

    #[test]
    fn hidden_stashes_are_no_tips_rows_or_labels() {
        let i = with_stashes(vec![stash('w', 0, 'd', 'i', None)]);
        let s = loaded_with(i.clone(), no_stashes());
        assert_eq!(s.total_rows(), 4);
        assert!(!s.log_args().contains(&h('w')));
        assert!(
            s.rows(0, 10)
                .iter()
                .all(|r| r.labels.iter().all(|l| l.kind != RefKind::Stash))
        );
        // A branch selection shows no stashes either.
        let feature = loaded_with(i, feature_view());
        assert!(!feature.log_args().contains(&h('w')));
    }

    #[test]
    fn search_treats_stash_rows_like_commits() {
        let s = loaded_with(with_stashes(vec![stash('w', 0, 'd', 'i', None)]), all());
        assert_eq!(s.search("stash@{0}"), [0]);
        assert_eq!(s.search("stash w"), [0]);
        assert_eq!(s.search("ada"), [0, 1, 3]);
    }

    #[test]
    fn a_new_stash_on_a_loaded_commit_is_added_without_a_reload() {
        let mut s = loaded(info());
        let before = s.rows(0, 10);
        let st = stash('w', 0, 'd', 'i', None);
        let i = with_stashes(vec![st]);
        assert!(!s.needs_reload(&i, &all(), false));
        assert!(s.needs_reload(&i, &all(), true), "not while the log loads");
        assert_eq!(s.update(i.clone(), all()), Update::Rows);
        assert_eq!(s.total_rows(), 5);
        assert_eq!(s.rows(0, 10), loaded_with(i.clone(), all()).rows(0, 10));
        assert_eq!(s.rows(1, 10).len(), before.len());
        // On a commit not loaded: the log must be read.
        let mut i = info();
        i.stashes = vec![stash('w', 0, 'e', 'i', None)];
        assert!(loaded(info()).needs_reload(&i, &all(), false));
    }

    /// Stashes saved in the same second (equal committer times) are ordered by their
    /// place in the stash list, newest first, whether put in memory or loaded with the log
    /// (git keeps the order of the tips it is given for equal times; the newest is given
    /// first).
    #[test]
    fn stashes_saved_in_the_same_second_go_newest_first() {
        let older = with_stashes(vec![stash('v', 0, 'd', 'j', None)]);
        let mut s = loaded_with(older, all());
        // A second stash on the same commit, in the same second: it is stash@{0} now.
        let both = with_stashes(vec![
            stash('w', 0, 'd', 'i', None),
            stash('v', 1, 'd', 'j', None),
        ]);
        assert!(!s.needs_reload(&both, &all(), false));
        assert_eq!(s.update(both.clone(), all()), Update::Rows);
        let hashes: Vec<String> = s.rows(0, 3).into_iter().map(|r| r.hash).collect();
        assert_eq!(hashes, [h('w'), h('v'), h('d')]);
        assert_eq!(s.rows(0, 10), loaded_with(both, all()).rows(0, 10));
    }

    #[test]
    fn a_dropped_stash_goes_without_a_reload_unless_its_base_goes_too() {
        let two = with_stashes(vec![
            stash('w', 0, 'd', 'i', None),
            stash('v', 1, 'b', 'j', Some('u')),
        ]);
        let mut s = loaded_with(two.clone(), all());
        assert_eq!(s.total_rows(), 6);
        // Dropping stash@{0}: v becomes stash@{0}, w's row goes.
        let one = with_stashes(vec![stash('v', 0, 'b', 'j', Some('u'))]);
        assert!(!s.needs_reload(&one, &all(), false));
        assert!(s.needs_reload(&one, &all(), true));
        assert_eq!(s.update(one.clone(), all()), Update::Rows);
        assert_eq!(s.rows(0, 10), loaded_with(one.clone(), all()).rows(0, 10));
        assert_eq!(label_names(&s, 0), [(RefKind::Stash, "stash@{0}".into())]);
        // A renumbering alone (same stashes, new selectors) changes labels only.
        let renumbered = with_stashes(vec![stash('v', 3, 'b', 'j', Some('u'))]);
        assert!(!s.needs_reload(&renumbered, &all(), true));
        assert_eq!(s.update(renumbered, all()), Update::Rows);
        assert_eq!(label_names(&s, 0), [(RefKind::Stash, "stash@{3}".into())]);
        assert_eq!(s.update(info(), all()), Update::Rows);
        assert_eq!(s.rows(0, 10), loaded(info()).rows(0, 10));

        // A stash whose base only it leads to: its commits go too, so the log is read again.
        let st = stash('w', 0, 'e', 'i', None);
        let mut s = RepoState::new(with_stashes(vec![st.clone()]), all(), 1);
        let mut log = stash_commits(&st);
        log.push(commit('e', &['a'], "Gone branch", "Ada"));
        log.extend(history());
        s.append(log);
        assert!(s.needs_reload(&info(), &all(), false));
    }

    #[test]
    fn showing_and_hiding_stashes_needs_no_reload() {
        let i = with_stashes(vec![
            stash('w', 0, 'd', 'i', Some('u')),
            stash('v', 1, 'b', 'j', None),
        ]);
        let mut s = loaded_with(i.clone(), all());
        assert!(!s.needs_reload(&i, &no_stashes(), false));
        assert_eq!(s.update(i.clone(), no_stashes()), Update::Rows);
        assert_eq!(s.rows(0, 10), loaded(i.clone()).rows(0, 10));
        assert!(!s.needs_reload(&i, &all(), false));
        assert_eq!(s.update(i.clone(), all()), Update::Rows);
        assert_eq!(s.rows(0, 10), loaded_with(i, all()).rows(0, 10));
    }

    #[test]
    fn snapshot_sorts_names_and_reports_state() {
        let mut i = dirty(info(), 1, 2);
        i.refs = vec![
            RefInfo {
                time: 30,
                author: "Zed".into(),
                subject: "Zeta's tip".into(),
                ..rf(RefKind::Branch, "zeta", 'd')
            },
            RefInfo {
                time: 20,
                author: "Ada".into(),
                subject: "Main's tip".into(),
                ..rf(RefKind::Branch, "main", 'b')
            },
            rf(RefKind::Remote, "origin/zeta", 'd'),
            rf(RefKind::Remote, "origin/alpha", 'a'),
            rf(RefKind::Tag, "v1", 'a'),
        ];
        i.remotes = vec!["origin".into()];
        let mut s = loaded(i.clone());
        s.generation = 9;
        let fetch = FetchState {
            running: true,
            last_error: Some("e".into()),
            last_error_detail: Some("fatal: e\n".into()),
            last_at_ms: Some(5),
        };
        let snap = s.snapshot(&identity(), true, &fetch);
        assert_eq!(snap.repo_id, 3);
        assert_eq!(snap.generation, 9);
        assert_eq!(snap.total_rows, 5);
        assert!(snap.loading);
        assert_eq!(snap.branches, ["main", "zeta"]);
        assert_eq!(snap.remote_branches, ["origin/alpha", "origin/zeta"]);
        // Each name's commit goes where the name went.
        let tip = |hash: char, time: i64, author: &str, subject: &str| BranchTip {
            hash: h(hash),
            time,
            author: author.into(),
            subject: subject.into(),
        };
        assert_eq!(
            snap.branch_tips,
            [
                tip('b', 20, "Ada", "Main's tip"),
                tip('d', 30, "Zed", "Zeta's tip")
            ]
        );
        assert_eq!(
            snap.remote_branch_tips,
            [tip('a', 0, "", ""), tip('d', 0, "", "")]
        );
        assert_eq!(snap.remotes, ["origin"]);
        assert_eq!(snap.status, i.status);
        assert_eq!(snap.uncommitted_subject, "Uncommitted changes (3)");
        assert_eq!(snap.head, i.head);
        assert_eq!(snap.worktrees, i.worktrees);
        assert_eq!(snap.view_options, all());
        assert_eq!(snap.fetch, fetch);
    }
}
