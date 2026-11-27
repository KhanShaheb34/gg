//! Cloning a repository the user names, pure parts (the work is `repo/clone.rs` and
//! `RepoManager::clone_repo`):
//!
//! - What a link typed or pasted in the Clone dialog names ([`read_link`]): an `https://`,
//!   `http://`, `ssh://` or `git://` address, an scp-like `[user@]host:path`, or GitHub's
//!   `owner/name`. The link is hostile input: one starting with `-`, a remote helper's
//!   `transport::address` (`ext::` runs a command), a local path or `file://`, a password, a
//!   space or a control character is refused; what is kept is handed to git after `--`, never to
//!   a shell.
//! - The folder a clone goes into ([`check_folder`], [`check_dest`]): one plain name inside the
//!   parent folder chosen, never a path that leaves it; refused when something not empty is
//!   there already. [`make_dest`] makes it (and the parent folders missing), and
//!   [`Made::undo`] takes away only what the clone made.
//! - The default parent folder ([`default_clone_dir`]).
//! - The user's repositories through `gh api graphql` ([`REPOS_QUERY`], [`parse_repos`]).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::dto::InboxProblem;
use crate::hosts::{is_known_github, is_plain_host, is_plain_word, parse_remote};
use crate::pr_details::parse_time;

/// Most characters of a link.
pub const MAX_LINK: usize = 2048;
/// Most bytes of a folder name (the longest most file systems take).
const MAX_FOLDER: usize = 255;

/// What [`read_link`] found in a link. Serialized camelCase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkInfo {
    /// The host, owner and repository the link names, when it names one as `owner/name` on a
    /// host (what a repository already in gg is matched by).
    pub host: Option<String>,
    pub owner: Option<String>,
    pub name: Option<String>,
    /// The folder git would make for it: the link's last part without `.git`.
    pub folder: String,
    /// It was GitHub's `owner/name`: cloned from github.com, by the protocol `gh` is set to
    /// use there (`https` without `gh`).
    pub shorthand: bool,
}

/// What a clone is made from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloneSource {
    /// A repository on a GitHub host, by the protocol `gh` is set to use there.
    Repo {
        host: String,
        owner: String,
        name: String,
    },
    /// An address as [`read_link`] kept it, handed to git as it is.
    Url(String),
}

/// What the link `text` names, and where it is cloned from; or why it is refused, in one
/// plain sentence. Surrounding spaces, quotes and angle brackets, and a leading `git clone `
/// or `gh repo clone `, are taken off (a pasted command line). See the module comment.
pub fn read_link(text: &str) -> Result<(CloneSource, LinkInfo), String> {
    let mut link = text.trim();
    for prefix in ["git clone ", "gh repo clone "] {
        if let Some(rest) = link.strip_prefix(prefix) {
            link = rest.trim_start();
        }
    }
    let link = link
        .trim_matches(|c| matches!(c, '"' | '\'' | '`' | '<' | '>'))
        .trim();
    if link.is_empty() {
        return Err("Paste the repository's link, or type owner/name for GitHub.".to_owned());
    }
    if link.chars().count() > MAX_LINK {
        return Err(format!(
            "That is longer than a link can be ({MAX_LINK} characters)."
        ));
    }
    if link.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("A link has no spaces in it: paste one link only.".to_owned());
    }
    if link.starts_with('-') {
        return Err("A link cannot start with -.".to_owned());
    }
    if helper_transport(link) {
        return Err(
            "gg clones only from https, ssh and git addresses (not through a transport::)."
                .to_owned(),
        );
    }
    if let Some((owner, name)) = shorthand(link) {
        let name = name.strip_suffix(".git").unwrap_or(name);
        if !is_plain_word(name) {
            return Err(not_a_link());
        }
        let info = LinkInfo {
            host: Some("github.com".to_owned()),
            owner: Some(owner.to_owned()),
            name: Some(name.to_owned()),
            folder: name.to_owned(),
            shorthand: true,
        };
        let source = CloneSource::Repo {
            host: "github.com".to_owned(),
            owner: owner.to_owned(),
            name: name.to_owned(),
        };
        return Ok((source, info));
    }
    let url = if link.contains("://") {
        scheme_url(link)?
    } else {
        scp_like(link)?
    };
    let folder = folder_of(&url).ok_or_else(|| "That link names no repository.".to_owned())?;
    check_folder(&folder)?;
    let on = parse_remote(&url);
    let info = LinkInfo {
        host: on.as_ref().map(|r| r.host.clone()),
        owner: on.as_ref().map(|r| r.owner.clone()),
        name: on.as_ref().map(|r| r.name.clone()),
        folder,
        shorthand: false,
    };
    Ok((CloneSource::Url(url), info))
}

fn not_a_link() -> String {
    "That is not a link git can clone from: paste an https or ssh address, or type owner/name \
     for GitHub."
        .to_owned()
}

/// Whether `link` is a remote helper's `transport::address` (`ext::sh -c …`, `fd::3`): git
/// would run a program of that name, so none is taken.
fn helper_transport(link: &str) -> bool {
    let Some((transport, _)) = link.split_once("::") else {
        return false;
    };
    let mut chars = transport.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// GitHub's `owner/name` (the name may end in `.git`): an owner as GitHub has them (letters,
/// digits, hyphens), one `/`, nothing more.
fn shorthand(link: &str) -> Option<(&str, &str)> {
    let (owner, name) = link.split_once('/')?;
    let owner_ok = !owner.is_empty()
        && owner.len() <= 39
        && !owner.starts_with('-')
        && owner
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-');
    (owner_ok && !name.is_empty() && !name.contains('/') && !link.contains(':'))
        .then_some((owner, name))
}

/// A `scheme://` address git may be handed: `https`, `http`, `ssh`, `git`, `git+ssh` or
/// `ssh+git`, to a plain host, no password, a user that is a name, a path. A GitHub page's
/// address (`https://github.com/octo/cat/tree/main`) is the repository's.
fn scheme_url(link: &str) -> Result<String, String> {
    let (scheme, _) = link.split_once("://").unwrap_or_default();
    let scheme = scheme.to_ascii_lowercase();
    if scheme == "file" {
        return Err("gg clones from a host, not from a folder on this computer.".to_owned());
    }
    if !matches!(
        scheme.as_str(),
        "https" | "http" | "ssh" | "git" | "git+ssh" | "ssh+git"
    ) {
        return Err(format!(
            "gg clones only from https, ssh and git addresses, not {scheme}://."
        ));
    }
    let parsed = url::Url::parse(link).map_err(|_| not_a_link())?;
    let host = parsed.host_str().unwrap_or("");
    if !is_plain_host(host) {
        return Err(not_a_link());
    }
    if parsed.password().is_some() {
        return Err(
            "That link has a password in it. Leave it out: git asks your credential helper, and \
             gg never passes one on."
                .to_owned(),
        );
    }
    if parsed.username().starts_with('-') || parsed.query().is_some() {
        return Err(not_a_link());
    }
    let segments: Vec<&str> = parsed.path().split('/').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return Err("That link names no repository.".to_owned());
    }
    // A page of a repository on GitHub (its files, a pull request) stands for the repository.
    if matches!(scheme.as_str(), "https" | "http")
        && is_known_github(host)
        && segments.len() > 2
        && is_plain_word(segments[0])
        && is_plain_word(segments[1])
    {
        let port = parsed.port().map(|p| format!(":{p}")).unwrap_or_default();
        let name = segments[1].strip_suffix(".git").unwrap_or(segments[1]);
        return Ok(format!(
            "{scheme}://{host}{port}/{}/{name}.git",
            segments[0]
        ));
    }
    // Without the page's fragment: git does not read one.
    let mut url = parsed.clone();
    url.set_fragment(None);
    Ok(url.to_string())
}

/// An scp-like address, `[user@]host:path`: a host of at least two characters (not a drive
/// letter), plain; a user that is a name; a path that is not empty and starts with no `-`.
fn scp_like(link: &str) -> Result<String, String> {
    let Some((authority, path)) = link.split_once(':') else {
        return Err(not_a_link());
    };
    if authority.contains('/') || authority.contains('\\') || path.starts_with("//") {
        return Err(not_a_link());
    }
    let (user, host) = match authority.rsplit_once('@') {
        Some((user, host)) => (Some(user), host),
        None => (None, authority),
    };
    let user_ok = user.is_none_or(|u| {
        !u.is_empty()
            && !u.starts_with('-')
            && u.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    });
    let path = path.trim_start_matches('/');
    if host.len() < 2
        || !is_plain_host(host)
        || !user_ok
        || path.is_empty()
        || path.starts_with('-')
    {
        return Err(not_a_link());
    }
    Ok(link.to_owned())
}

/// The folder git makes for `url`: its path's last part, without `.git` (and a trailing `/`).
fn folder_of(url: &str) -> Option<String> {
    let path = match url.split_once("://") {
        Some((_, rest)) => rest.split_once('/').map_or("", |(_, p)| p),
        None => url.split_once(':').map_or("", |(_, p)| p),
    };
    let last = path.trim_end_matches('/').rsplit('/').next()?;
    let last = last.strip_suffix(".git").unwrap_or(last);
    // Percent-encoded by the parser: the folder is the name as written.
    let last = percent_decode(last)?;
    (!last.is_empty()).then_some(last)
}

/// `text` with its `%XX` escapes read; `None` when they are not UTF-8.
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = text.get(i + 1..i + 3)
            && let Ok(b) = u8::from_str_radix(hex, 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
}

/// Names no folder can have on Windows (in any case, with or without an extension).
const RESERVED: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Whether `folder` can be the name of a clone's folder: one name, inside the parent (no `/`,
/// `\`, `.` or `..`), that every platform takes (none of `<>:"|?*`, no control character, not
/// ending in a dot or a space, not a name Windows keeps), not starting with `-`, at most 255
/// bytes. Why not, in one sentence.
pub fn check_folder(folder: &str) -> Result<(), String> {
    if folder.trim().is_empty() {
        return Err("Name the folder.".to_owned());
    }
    if folder == "." || folder == ".." {
        return Err(format!("A folder cannot be named {folder}."));
    }
    if folder.contains(['/', '\\']) {
        return Err(
            "A folder name has no / or \\ in it: it goes inside the folder above.".to_owned(),
        );
    }
    if let Some(c) = folder
        .chars()
        .find(|c| c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
    {
        let shown = if c.is_control() {
            "a control character".to_owned()
        } else {
            c.to_string()
        };
        return Err(format!("A folder name cannot have {shown} in it."));
    }
    if folder.starts_with('-') {
        return Err("A folder name cannot start with -.".to_owned());
    }
    if folder.ends_with(['.', ' ']) || folder.starts_with(' ') {
        return Err(
            "A folder name cannot start with a space or end with a dot or a space.".to_owned(),
        );
    }
    let stem = folder.split('.').next().unwrap_or("").to_ascii_lowercase();
    if RESERVED.contains(&stem.as_str()) {
        return Err(format!(
            "Windows keeps the name {folder} for itself: choose another."
        ));
    }
    if folder.len() > MAX_FOLDER {
        return Err("That folder name is too long.".to_owned());
    }
    Ok(())
}

/// Where a clone of `folder` into `parent` would go, and whether it can. Serialized camelCase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DestCheck {
    /// The folder the clone would be in.
    pub path: String,
    /// Why it cannot go there, in one sentence; `None` when it can.
    pub problem: Option<String>,
    /// `parent` is not there yet: the clone makes it.
    pub makes_parent: bool,
}

/// Whether a clone can go into a folder named `folder` in `parent` (see [`DestCheck`]): the
/// parent is a full path, and a folder or nothing; the name passes [`check_folder`]; there is
/// nothing of that name there, or an empty folder (git clones into one).
pub fn check_dest(parent: &Path, folder: &str) -> DestCheck {
    let folder = folder.trim();
    let path = parent.join(folder);
    let shown = path.to_string_lossy().into_owned();
    let refuse = |problem: String| DestCheck {
        path: shown.clone(),
        problem: Some(problem),
        makes_parent: false,
    };
    if parent.as_os_str().is_empty() {
        return refuse("Choose the folder to clone into.".to_owned());
    }
    if !parent.is_absolute() {
        return refuse("Type the folder's full path, or choose one.".to_owned());
    }
    if let Err(why) = check_folder(folder) {
        return refuse(why);
    }
    let makes_parent = match fs::metadata(parent) {
        Ok(m) if m.is_dir() => false,
        Ok(_) => return refuse(format!("{} is a file, not a folder.", parent.display())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => true,
        Err(e) => return refuse(format!("{} cannot be read: {e}.", parent.display())),
    };
    if !makes_parent && let Some(problem) = taken(parent, folder, &path) {
        return refuse(problem);
    }
    DestCheck {
        path: shown,
        problem: None,
        makes_parent,
    }
}

/// Why `path` (`folder` in `parent`) is taken, if it is: anything there but an empty folder.
fn taken(parent: &Path, folder: &str, path: &Path) -> Option<String> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
        Err(e) => return Some(format!("{} cannot be read: {e}.", path.display())),
    };
    let what = if meta.is_symlink() {
        "a link"
    } else if meta.is_dir() {
        match fs::read_dir(path).map(|mut entries| entries.next().is_none()) {
            Ok(true) => return None,
            Ok(false) => "a folder",
            Err(e) => return Some(format!("{} cannot be read: {e}.", path.display())),
        }
    } else {
        "a file"
    };
    let tail = if what == "a folder" {
        ", and it is not empty"
    } else {
        ""
    };
    Some(format!(
        "There is {what} named {folder} in {} already{tail}. Choose another name.",
        parent.display()
    ))
}

/// What [`make_dest`] made for a clone, so that a clone that does not finish takes away only
/// that: the clone's folder (or, when it was there empty, what is in it) and the parent
/// folders it made, those still empty.
#[derive(Debug)]
pub struct Made {
    dest: PathBuf,
    made_dest: bool,
    /// The parent folders made, deepest first.
    made_parents: Vec<PathBuf>,
}

/// Makes the folder a clone of `folder` into `parent` goes into, after checking again (see
/// [`check_dest`]): the parent folders missing, then the clone's folder itself (git clones
/// into an empty folder). Why not, in one sentence.
pub fn make_dest(parent: &Path, folder: &str) -> Result<Made, String> {
    let check = check_dest(parent, folder);
    if let Some(problem) = check.problem {
        return Err(problem);
    }
    let mut missing = Vec::new();
    let mut at = Some(parent);
    while let Some(dir) = at {
        if fs::symlink_metadata(dir).is_ok() {
            break;
        }
        missing.push(dir.to_path_buf());
        at = dir.parent();
    }
    // Made top down; on failure what was made is taken away.
    let mut made_parents = Vec::new();
    for dir in missing.iter().rev() {
        if let Err(e) = fs::create_dir(dir) {
            undo_parents(&made_parents);
            return Err(format!("Could not make {}: {e}.", dir.display()));
        }
        made_parents.insert(0, dir.clone());
    }
    let dest = parent.join(folder.trim());
    let made_dest = match fs::create_dir(&dest) {
        Ok(()) => true,
        // An empty folder there already (checked above): git clones into it.
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            if let Some(problem) = taken(parent, folder.trim(), &dest) {
                undo_parents(&made_parents);
                return Err(problem);
            }
            false
        }
        Err(e) => {
            undo_parents(&made_parents);
            return Err(format!("Could not make {}: {e}.", dest.display()));
        }
    };
    Ok(Made {
        dest,
        made_dest,
        made_parents,
    })
}

impl Made {
    /// The clone's folder.
    pub fn dest(&self) -> &Path {
        &self.dest
    }

    /// Takes away what the clone made: its folder (or what is in it, when it was there
    /// empty), then the parent folders [`make_dest`] made, while they are empty. Links in it
    /// are removed, never followed.
    pub fn undo(&self) {
        if self.made_dest {
            let _ = fs::remove_dir_all(&self.dest);
        } else if let Ok(entries) = fs::read_dir(&self.dest) {
            for entry in entries.flatten() {
                let path = entry.path();
                let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
                let _ = if is_dir {
                    fs::remove_dir_all(&path)
                } else {
                    fs::remove_file(&path)
                };
            }
        }
        undo_parents(&self.made_parents);
    }
}

/// Removes the folders `made` (deepest first) while they are empty.
fn undo_parents(made: &[PathBuf]) {
    for dir in made {
        if fs::remove_dir(dir).is_err() {
            break;
        }
    }
}

/// git's arguments for cloning `url` into `dest`: `clone --progress -- <url> <dest>`, the
/// address and the folder after `--` (never read as options), and, unless the address is the
/// clone source's folder (`local`, tests and the devserver), only the transports a link may name
/// allowed (`https`, `http`, `ssh`, `git`), whatever the user's config allows. `upload_pack`
/// (tests and the devserver only: what the clone source answers through) goes before `--`.
pub(crate) fn clone_args(
    url: &str,
    dest: &str,
    local: bool,
    upload_pack: Option<String>,
) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    if !local {
        args.extend(["-c".into(), "protocol.allow=never".into()]);
        for protocol in ["https", "http", "ssh", "git"] {
            args.extend(["-c".into(), format!("protocol.{protocol}.allow=always")]);
        }
    }
    args.extend(["clone".into(), "--progress".into()]);
    if let Some(program) = upload_pack {
        args.push(format!("--upload-pack={program}"));
    }
    args.extend(["--", url, dest].into_iter().map(str::to_owned));
    args
}

/// What a clone of `what` (a name, set off) that git failed with `stderr` (exit `code`) says:
/// a headline and the line under it. A failure the core knows (credentials, no connection, no
/// such repository, a full disk) in plain words; anything else quotes git's telling line.
pub(crate) fn clone_failure(what: &str, stderr: &str, code: i32) -> String {
    use crate::notice::{said, telling_line};
    let lower = stderr.to_lowercase();
    let missing = lower.contains("repository not found")
        || (lower.contains("repository '") && lower.contains("' not found"))
        || lower.contains("does not appear to be a git repository");
    if missing {
        return said(
            format!("No repository at {what}"),
            "It does not exist, or your account cannot see it. Check the link, and the account \
             git signs in with.",
        );
    }
    if lower.contains("no space left on device") {
        return said(
            "The disk is full",
            "git could not write the clone. Make room, then try again.",
        );
    }
    if let Some(known) = crate::ops::explain::recognised(&["clone"], stderr) {
        return known;
    }
    said(
        format!("Could not clone {what}"),
        telling_line(stderr).unwrap_or_else(|| format!("git exited with code {code}.")),
    )
}

/// The folder clones go into when the settings name none: `~/Developer` on macOS (the folder
/// Xcode and Finder know), `~/Projects` on Windows and Linux.
pub fn default_clone_dir(home: &Path) -> PathBuf {
    home.join(if cfg!(target_os = "macos") {
        "Developer"
    } else {
        "Projects"
    })
}

/// How many of the user's repositories are listed: the most recently pushed to.
pub const REPOS_CAP: u32 = 100;

/// The user's repositories (`gh api graphql`, variable `first`): those they own, collaborate
/// on, or reach through an organisation they are a member of, most recently pushed to first.
pub const REPOS_QUERY: &str = "query GgRepos($first: Int!) { viewer { login \
repositories(first: $first, ownerAffiliations: [OWNER, COLLABORATOR, ORGANIZATION_MEMBER], \
affiliations: [OWNER, COLLABORATOR, ORGANIZATION_MEMBER], \
orderBy: {field: PUSHED_AT, direction: DESC}) { totalCount \
nodes { name owner { login } isPrivate isArchived isFork description pushedAt } } } }";

/// One of the user's repositories. Serialized camelCase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListedRepo {
    pub host: String,
    pub owner: String,
    pub name: String,
    pub private: bool,
    pub archived: bool,
    pub fork: bool,
    /// Its one-line description, if it has one.
    pub description: Option<String>,
    /// When it was last pushed to (unix ms), if known.
    pub pushed_at_ms: Option<i64>,
}

/// The user's repositories on a host, or why they could not be listed. Serialized camelCase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoList {
    pub host: String,
    /// The account `gh` answered as.
    pub login: Option<String>,
    /// At most [`REPOS_CAP`], most recently pushed to first.
    pub repos: Vec<ListedRepo>,
    /// How many there are in all.
    pub total: u32,
    /// Why they could not be listed (the Pull requests page's reasons); `None` when they were.
    pub problem: Option<InboxProblem>,
    /// What to say of the problem, in plain words.
    pub message: Option<String>,
    /// `gh`'s own words, on failure.
    pub detail: Option<String>,
}

impl RepoList {
    /// A list that could not be had: `problem`, said as `message`, with `gh`'s `detail`.
    pub fn failed(
        host: &str,
        problem: InboxProblem,
        message: String,
        detail: Option<String>,
    ) -> Self {
        Self {
            host: host.to_owned(),
            login: None,
            repos: Vec::new(),
            total: 0,
            problem: Some(problem),
            message: Some(message),
            detail: detail.filter(|d| !d.trim().is_empty()),
        }
    }
}

/// `gh`'s answer to [`REPOS_QUERY`] on `host`, read leniently: a repository without an owner or
/// a name a remote could have is left out; anything else missing is unknown. `Err` with
/// GitHub's message when the answer holds errors and no list.
pub fn parse_repos(stdout: &[u8], host: &str) -> Result<RepoList, String> {
    let value: Value =
        serde_json::from_slice(stdout).map_err(|_| "gh's answer could not be read.".to_owned())?;
    let viewer = &value["data"]["viewer"];
    if viewer.is_null() {
        let message = value["errors"][0]["message"]
            .as_str()
            .unwrap_or("gh's answer has no repositories in it.");
        return Err(message.to_owned());
    }
    let repos = viewer["repositories"]["nodes"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|node| {
            let owner = node["owner"]["login"].as_str()?;
            let name = node["name"].as_str()?;
            if !is_plain_word(owner) || !is_plain_word(name) {
                return None;
            }
            let description = node["description"]
                .as_str()
                .map(|d| d.split_whitespace().collect::<Vec<_>>().join(" "))
                .filter(|d| !d.is_empty());
            Some(ListedRepo {
                host: host.to_owned(),
                owner: owner.to_owned(),
                name: name.to_owned(),
                private: node["isPrivate"].as_bool().unwrap_or(false),
                archived: node["isArchived"].as_bool().unwrap_or(false),
                fork: node["isFork"].as_bool().unwrap_or(false),
                description,
                pushed_at_ms: node["pushedAt"].as_str().and_then(parse_time),
            })
        })
        .take(REPOS_CAP as usize)
        .collect::<Vec<_>>();
    let total = viewer["repositories"]["totalCount"]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(repos.len() as u32);
    Ok(RepoList {
        host: host.to_owned(),
        login: viewer["login"].as_str().map(str::to_owned),
        total: total.max(repos.len() as u32),
        repos,
        problem: None,
        message: None,
        detail: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url_of(link: &str) -> String {
        match read_link(link).unwrap_or_else(|e| panic!("{link}: {e}")).0 {
            CloneSource::Url(url) => url,
            other => panic!("{link}: {other:?}"),
        }
    }

    fn info(link: &str) -> LinkInfo {
        read_link(link).unwrap_or_else(|e| panic!("{link}: {e}")).1
    }

    #[test]
    fn links_name_their_repository_and_folder() {
        for (link, url) in [
            (
                "https://github.com/octo/cat.git",
                "https://github.com/octo/cat.git",
            ),
            ("https://github.com/octo/cat", "https://github.com/octo/cat"),
            (
                "  https://github.com/octo/cat  ",
                "https://github.com/octo/cat",
            ),
            (
                "<https://github.com/octo/cat>",
                "https://github.com/octo/cat",
            ),
            (
                "git clone https://github.com/octo/cat.git",
                "https://github.com/octo/cat.git",
            ),
            (
                "ssh://git@github.com/octo/cat.git",
                "ssh://git@github.com/octo/cat.git",
            ),
            ("git@github.com:octo/cat.git", "git@github.com:octo/cat.git"),
            (
                "git://git.example.org/octo/cat.git",
                "git://git.example.org/octo/cat.git",
            ),
            (
                "https://git.example.org/team/sub/cat.git",
                "https://git.example.org/team/sub/cat.git",
            ),
        ] {
            assert_eq!(url_of(link), url, "{link}");
            assert_eq!(info(link).folder, "cat", "{link}");
            assert!(!info(link).shorthand, "{link}");
        }
        let i = info("git@github.com:octo/cat.git");
        assert_eq!(
            (i.host.as_deref(), i.owner.as_deref(), i.name.as_deref()),
            (Some("github.com"), Some("octo"), Some("cat"))
        );
        // A subgroup: no owner/name to match by, but a folder.
        assert_eq!(info("https://git.example.org/team/sub/cat.git").owner, None);
    }

    #[test]
    fn a_github_page_stands_for_its_repository() {
        assert_eq!(
            url_of("https://github.com/octo/cat/tree/main/src"),
            "https://github.com/octo/cat.git"
        );
        assert_eq!(
            url_of("https://github.com/octo/cat/pull/7#discussion"),
            "https://github.com/octo/cat.git"
        );
        assert_eq!(info("https://github.com/octo/cat/pull/7").folder, "cat");
        // Not GitHub: the path is the repository's.
        assert_eq!(
            url_of("https://git.example.org/octo/cat/extra"),
            "https://git.example.org/octo/cat/extra"
        );
    }

    #[test]
    fn owner_slash_name_is_github_by_gh() {
        let (source, i) = read_link("octo/cat").unwrap();
        assert_eq!(
            source,
            CloneSource::Repo {
                host: "github.com".into(),
                owner: "octo".into(),
                name: "cat".into()
            }
        );
        assert!(i.shorthand);
        assert_eq!(i.folder, "cat");
        assert_eq!(
            read_link("gh repo clone octo/cat.git").unwrap().0,
            CloneSource::Repo {
                host: "github.com".into(),
                owner: "octo".into(),
                name: "cat".into()
            }
        );
    }

    #[test]
    fn hostile_links_are_refused() {
        for link in [
            "",
            "   ",
            "-uploadpack=touch /tmp/x",
            "--upload-pack=touch",
            "ext::sh -c touch% /tmp/pwned",
            "ext::sh",
            "fd::3",
            "FD::17,18",
            "transport::https://github.com/octo/cat",
            "file:///etc",
            "file://localhost/srv/repo.git",
            "/srv/repo.git",
            "./repo",
            "~/repo",
            "C:\\repos\\cat",
            "C:/repos/cat",
            "ftp://example.org/cat.git",
            "https://user:token@github.com/octo/cat.git",
            "https://github.com/octo/cat .git",
            "https://github.com/octo/cat\n--upload-pack=x",
            "https://-oProxyCommand=x/octo/cat",
            "ssh://-oProxyCommand=touch/octo/cat",
            "ssh://-u@github.com/octo/cat",
            "-oProxyCommand=touch:octo/cat",
            "git@-host:octo/cat",
            "-git@github.com:octo/cat",
            "git@github.com:-octo/cat",
            "git@github.com:",
            "https://github.com/",
            "https://github.com",
            "https://exa$mple.org/octo/cat",
            "https://github.com/octo/cat?x=1",
            "octo/cat/extra",
            "-octo/cat",
            "octo/..",
        ] {
            assert!(read_link(link).is_err(), "{link:?} was taken");
        }
        let long = format!("https://github.com/octo/{}", "a".repeat(MAX_LINK));
        assert!(read_link(&long).is_err());
        assert!(
            read_link("https://user:t@github.com/octo/cat")
                .unwrap_err()
                .contains("password")
        );
    }

    #[test]
    fn folder_names_stay_inside_their_parent() {
        for ok in ["cat", "cat.web", "cat_2", "Kätzchen", "a b"] {
            assert_eq!(check_folder(ok), Ok(()), "{ok}");
        }
        for bad in [
            "", " ", ".", "..", "a/b", "../x", "a\\b", "-x", "x.", "x ", " x", "a:b", "a*b", "con",
            "NUL.txt", "lpt1", "a\u{7}b",
        ] {
            assert!(check_folder(bad).is_err(), "{bad:?} was taken");
        }
        assert!(check_folder(&"a".repeat(256)).is_err());
    }

    #[test]
    fn a_destination_is_refused_when_something_is_there() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path();
        let ok = check_dest(parent, "cat");
        assert_eq!(ok.problem, None);
        assert!(!ok.makes_parent);
        assert_eq!(ok.path, parent.join("cat").to_string_lossy());
        // An empty folder: git clones into it.
        fs::create_dir(parent.join("empty")).unwrap();
        assert_eq!(check_dest(parent, "empty").problem, None);
        fs::create_dir(parent.join("full")).unwrap();
        fs::write(parent.join("full/a"), "a").unwrap();
        let full = check_dest(parent, "full").problem.unwrap();
        assert!(full.contains("not empty"), "{full}");
        fs::write(parent.join("file"), "a").unwrap();
        assert!(
            check_dest(parent, "file")
                .problem
                .unwrap()
                .contains("a file")
        );
        // A parent not there yet is made.
        let later = check_dest(&parent.join("later/deeper"), "cat");
        assert_eq!(later.problem, None);
        assert!(later.makes_parent);
        // A parent that is a file, a relative one, a name that leaves it.
        assert!(check_dest(&parent.join("file"), "cat").problem.is_some());
        assert!(check_dest(Path::new("relative"), "cat").problem.is_some());
        assert!(check_dest(parent, "../escape").problem.is_some());
        assert!(check_dest(Path::new(""), "cat").problem.is_some());
    }

    #[test]
    fn undo_takes_away_only_what_the_clone_made() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("keep.txt"), "mine").unwrap();
        // A new folder in a parent made for it, two levels deep: all of it goes.
        let parent = root.join("Developer/team");
        let made = make_dest(&parent, "cat").unwrap();
        assert!(made.dest().is_dir());
        fs::create_dir_all(made.dest().join(".git/objects")).unwrap();
        fs::write(made.dest().join(".git/objects/pack"), "half").unwrap();
        made.undo();
        assert!(!root.join("Developer").exists());
        assert!(root.join("keep.txt").exists());

        // An empty folder that was there: emptied again, kept; its parent untouched.
        fs::create_dir_all(root.join("code/empty")).unwrap();
        fs::write(root.join("code/other.txt"), "mine").unwrap();
        let made = make_dest(&root.join("code"), "empty").unwrap();
        fs::write(made.dest().join("README"), "half").unwrap();
        fs::create_dir(made.dest().join(".git")).unwrap();
        made.undo();
        assert!(root.join("code/empty").is_dir());
        assert_eq!(fs::read_dir(root.join("code/empty")).unwrap().count(), 0);
        assert!(root.join("code/other.txt").exists());

        // A parent made that something else has since written to stays.
        let made = make_dest(&root.join("shared/deep"), "cat").unwrap();
        fs::write(root.join("shared/note.txt"), "theirs").unwrap();
        made.undo();
        assert!(!root.join("shared/deep").exists());
        assert!(root.join("shared/note.txt").exists());

        // A link in the clone's folder is removed, not followed.
        #[cfg(unix)]
        {
            let outside = root.join("outside");
            fs::create_dir(&outside).unwrap();
            fs::write(outside.join("precious"), "x").unwrap();
            let made = make_dest(root, "linked").unwrap();
            std::os::unix::fs::symlink(&outside, made.dest().join("link")).unwrap();
            made.undo();
            assert!(outside.join("precious").exists());
            assert!(!root.join("linked").exists());
        }
    }

    #[test]
    fn make_dest_refuses_what_check_dest_refuses() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("full")).unwrap();
        fs::write(dir.path().join("full/a"), "a").unwrap();
        assert!(make_dest(dir.path(), "full").is_err());
        assert!(make_dest(dir.path(), "..").is_err());
        assert!(dir.path().join("full/a").exists());
    }

    #[test]
    fn git_takes_the_link_after_double_dash_and_only_safe_transports() {
        let args = clone_args("https://github.com/octo/cat.git", "/d/cat", false, None);
        assert_eq!(
            args,
            [
                "-c",
                "protocol.allow=never",
                "-c",
                "protocol.https.allow=always",
                "-c",
                "protocol.http.allow=always",
                "-c",
                "protocol.ssh.allow=always",
                "-c",
                "protocol.git.allow=always",
                "clone",
                "--progress",
                "--",
                "https://github.com/octo/cat.git",
                "/d/cat",
            ]
        );
        let local = clone_args("file:///r/octo/cat.git", "/d/cat", true, None);
        assert_eq!(local[..3], ["clone", "--progress", "--"]);
        let held = clone_args(
            "file:///r/octo/cat.git",
            "/d/cat",
            true,
            Some("/t/hook".into()),
        );
        assert_eq!(
            held[..4],
            ["clone", "--progress", "--upload-pack=/t/hook", "--"]
        );
    }

    #[test]
    fn failures_are_said_plainly() {
        let said =
            |stderr: &str| crate::notice::notice(&clone_failure("octo/cat", stderr, 128)).plain();
        assert!(said("remote: Repository not found.\nfatal: repository 'https://github.com/octo/cat.git/' not found\n")
            .starts_with("No repository at octo/cat."));
        assert!(
            said("ERROR: Repository not found.\nfatal: Could not read from remote repository.")
                .starts_with("No repository at")
        );
        assert!(said("fatal: unable to access 'https://github.com/octo/cat.git/': Could not resolve host: github.com")
            .starts_with("Could not reach the remote"));
        assert!(
            said(
                "fatal: could not read Username for 'https://github.com': terminal prompts disabled"
            )
            .contains("username and password")
        );
        assert!(
            said("git@github.com: Permission denied (publickey).")
                .contains("SSH server refused your key")
        );
        assert!(
            said("error: unable to write file: No space left on device")
                .starts_with("The disk is full")
        );
        assert_eq!(
            said("fatal: something odd\n"),
            "Could not clone octo/cat. Something odd."
        );
        assert_eq!(
            said(""),
            "Could not clone octo/cat. git exited with code 128."
        );
    }

    #[test]
    fn the_default_folder_is_the_platforms() {
        let home = Path::new("/home/ada");
        let dir = default_clone_dir(home);
        if cfg!(target_os = "macos") {
            assert_eq!(dir, home.join("Developer"));
        } else {
            assert_eq!(dir, home.join("Projects"));
        }
    }

    #[test]
    fn repositories_are_read_leniently() {
        let answer = serde_json::json!({"data": {"viewer": {
            "login": "octo",
            "repositories": {"totalCount": 240, "nodes": [
                {"name": "cat", "owner": {"login": "octo"}, "isPrivate": true, "isArchived": false,
                 "isFork": false, "description": "  A  cat\nthat purrs ", "pushedAt": "2026-03-01T10:00:00Z"},
                {"name": "dog", "owner": {"login": "hub"}, "isPrivate": false, "description": null,
                 "pushedAt": null},
                {"name": "--bad", "owner": {"login": "octo"}},
                {"owner": {"login": "octo"}},
                null
            ]}
        }}});
        let list = parse_repos(answer.to_string().as_bytes(), "github.com").unwrap();
        assert_eq!(list.login.as_deref(), Some("octo"));
        assert_eq!(list.total, 240);
        assert_eq!(list.repos.len(), 2);
        let cat = &list.repos[0];
        assert_eq!((cat.owner.as_str(), cat.name.as_str()), ("octo", "cat"));
        assert!(cat.private);
        assert_eq!(cat.description.as_deref(), Some("A cat that purrs"));
        assert_eq!(cat.pushed_at_ms, Some(1_772_359_200_000));
        assert_eq!(list.repos[1].description, None);
        assert_eq!(list.repos[1].pushed_at_ms, None);
        assert!(!list.repos[1].private);

        let failed = serde_json::json!({"errors": [{"message": "API rate limit exceeded"}]});
        assert_eq!(
            parse_repos(failed.to_string().as_bytes(), "github.com").unwrap_err(),
            "API rate limit exceeded"
        );
        assert!(parse_repos(b"not json", "github.com").is_err());
    }
}
