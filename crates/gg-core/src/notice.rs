//! What a notice says, written by the operations as one string with its parts marked, and
//! turned into a [`Notice`] (a headline, a quieter line under it, names set off) once, when
//! the outcome is made.
//!
//! An operation writes `said("Pushed " + name(branch), "Upstream set")`: [`name`] and [`path`]
//! wrap a name in marks (characters from Unicode's private use area, taken out of the name
//! itself first, so a name can never close its own mark), and [`said`] puts a mark between the
//! headline and the line under it. Everything an operation does with its message as a string
//! (joining two, adding a sentence, checking what it says) keeps working; the marks never
//! leave the core: [`notice`] reads them into parts, [`Notice::plain`] is the sentence without
//! them.

use crate::dto::{Notice, Part};

const REF_OPEN: char = '\u{E000}';
const REF_CLOSE: char = '\u{E001}';
const PATH_OPEN: char = '\u{E002}';
const PATH_CLOSE: char = '\u{E003}';
/// Between the headline and the line under it.
const NOTE: char = '\u{E004}';

const MARKS: [char; 5] = [REF_OPEN, REF_CLOSE, PATH_OPEN, PATH_CLOSE, NOTE];

/// Most characters of git's own line a notice quotes (see [`telling_line`]).
const MAX_QUOTED: usize = 160;

/// `text` without the marks.
fn unmarked(text: &str) -> String {
    text.chars().filter(|c| !MARKS.contains(c)).collect()
}

/// `name` (a branch, a remote branch, a tag, a commit's short hash, a remote, a stash), set off.
pub(crate) fn name(name: &str) -> String {
    format!("{REF_OPEN}{}{REF_CLOSE}", unmarked(name))
}

/// `path`, set off as a path (the UI shows it from the home folder).
pub(crate) fn path(path: impl AsRef<str>) -> String {
    format!("{PATH_OPEN}{}{PATH_CLOSE}", unmarked(path.as_ref()))
}

/// A headline with the line under it, as a constant: `said_const!("Headline", "The line.")`
/// ([`said`] for text made at run time).
macro_rules! said_const {
    ($title:literal, $note:literal) => {
        concat!($title, "\u{E004}", $note)
    };
}
pub(crate) use said_const;

/// A headline with `note` under it (none when empty).
pub(crate) fn said(title: impl AsRef<str>, note: impl AsRef<str>) -> String {
    let (title, note) = (title.as_ref(), note.as_ref());
    if note.is_empty() {
        title.to_owned()
    } else {
        format!("{title}{NOTE}{note}")
    }
}

/// The headline of `message`.
pub(crate) fn title_of(message: &str) -> &str {
    message.split_once(NOTE).map_or(message, |(title, _)| title)
}

/// The line under the headline of `message`, empty when it has none.
pub(crate) fn note_of(message: &str) -> &str {
    message.split_once(NOTE).map_or("", |(_, note)| note)
}

/// `message` with `more` added to the line under its headline (made the line if it has none).
/// A message written as plain sentences is shaped first, as [`notice`] shapes it, so its
/// headline stays short.
pub(crate) fn add_note(message: &str, more: &str) -> String {
    let (title, note) = split(message);
    let joined = if note.is_empty() {
        more.to_owned()
    } else {
        format!("{note} {more}")
    };
    said(title, joined)
}

/// The headline of `message` and the line under it: as marked, else as [`plain_split`] finds
/// them (the line's first letter in upper case).
fn split(message: &str) -> (String, String) {
    if message.contains(NOTE) {
        (title_of(message).to_owned(), note_of(message).to_owned())
    } else if let Some((end, start)) = plain_split(message) {
        (
            message[..end].to_owned(),
            capitalized(message.get(start..).unwrap_or("")),
        )
    } else {
        (message.to_owned(), String::new())
    }
}

/// `message` as one sentence: the headline, then the line under it (see [`plain`]).
pub(crate) fn as_sentence(message: &str) -> String {
    notice(message).plain()
}

/// The parts of `text`, its marks read.
fn parts(text: &str) -> Vec<Part> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut open: Option<char> = None;
    let flush = |buf: &mut String, out: &mut Vec<Part>, kind: Option<char>| {
        if buf.is_empty() {
            return;
        }
        let text = std::mem::take(buf);
        out.push(match kind {
            Some(REF_OPEN) => Part::Ref(text),
            Some(PATH_OPEN) => Part::Path(text),
            _ => Part::Text(text),
        });
    };
    for c in text.chars() {
        match c {
            REF_OPEN | PATH_OPEN => {
                flush(&mut buf, &mut out, open);
                open = Some(c);
            }
            REF_CLOSE | PATH_CLOSE => {
                flush(&mut buf, &mut out, open);
                open = None;
            }
            NOTE => {}
            c => buf.push(c),
        }
    }
    flush(&mut buf, &mut out, open);
    out
}

/// Most characters of what comes before a colon for it to be read as a headline ("Removed the
/// gg command: ~/bin/gg."); a longer lead is a sentence that happens to have a colon.
const MAX_LEAD: usize = 48;

/// Where a message written as plain sentences, with no headline marked, splits into a
/// headline and the line under it: at a colon in its first sentence after a short lead
/// ("Nothing was committed: the message is empty."), else after its first sentence. Byte
/// offsets of the headline's end and the line's start; `None` for one sentence. Never inside a
/// name. The UI shapes its own notices the same way (`src/lib/notice.ts`, `shape`).
fn plain_split(message: &str) -> Option<(usize, usize)> {
    let mut inside = false;
    let mut first_sentence = None;
    let mut colon = None;
    let chars: Vec<(usize, char)> = message.char_indices().collect();
    for (i, &(at, c)) in chars.iter().enumerate() {
        match c {
            REF_OPEN | PATH_OPEN => inside = true,
            REF_CLOSE | PATH_CLOSE => inside = false,
            _ if inside => {}
            ':' if colon.is_none() && chars.get(i + 1).map(|&(_, c)| c) == Some(' ') => {
                colon = Some(at);
            }
            '.' | '!' | '?' => {
                let next = chars.get(i + 1).map(|&(_, c)| c);
                let after = chars.get(i + 2).map(|&(_, c)| c);
                if next == Some(' ') && after.is_some_and(|c| c.is_uppercase() || c == '\u{201c}') {
                    first_sentence = Some(at);
                    break;
                }
            }
            _ => {}
        }
    }
    let short_lead = |at: usize| message[..at].chars().count() <= MAX_LEAD;
    match (colon, first_sentence) {
        (Some(at), _) if short_lead(at) => Some((at, at + 2)),
        (_, Some(at)) => {
            // The full stop stays out of the headline; "?" and "!" stay in it.
            let end = if message[at..].starts_with('.') {
                at
            } else {
                at + 1
            };
            Some((end, at + 2))
        }
        _ => None,
    }
}

/// Words that keep their lower case at the start of a sentence: the programs, as gg writes
/// them.
const LOWER_CASE: [&str; 4] = ["git", "gg", "gh", "ssh"];

/// `text` with its first letter in upper case, unless it starts with a program's name (git)
/// or a name set off (a file's, a branch's: as they are).
pub(crate) fn capitalized(text: &str) -> String {
    let first_word = text.split([' ', ',', ':', '.']).next().unwrap_or("");
    if LOWER_CASE.contains(&first_word) {
        return text.to_owned();
    }
    let mut chars = text.chars();
    chars
        .next()
        .map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
}

/// The notice `message` says (see the module comment). A message with no headline marked is
/// shaped all the same (see [`plain_split`]); a headline's closing full stop goes.
pub fn notice(message: &str) -> Notice {
    let (title, note) = split(message);
    let title = match title.strip_suffix('.') {
        Some(t) if !t.ends_with('.') => t.to_owned(),
        _ => title,
    };
    Notice {
        title: parts(&title),
        note: parts(&note),
    }
}

/// The text of `parts`, names as they are.
#[cfg(test)]
fn text_of(parts: &[Part]) -> String {
    parts
        .iter()
        .map(|p| match p {
            Part::Text(t) | Part::Ref(t) | Part::Path(t) => t.as_str(),
        })
        .collect()
}

impl Notice {
    /// A notice of plain text, with no line under it.
    pub fn text(text: impl Into<String>) -> Self {
        let text = text.into();
        Notice {
            title: if text.is_empty() {
                Vec::new()
            } else {
                vec![Part::Text(text)]
            },
            note: Vec::new(),
        }
    }

    /// The notice as one sentence: the headline, a full stop unless it ends with one (or a
    /// colon), then the line under it. A notice's short forms read as words: " → " as " into ",
    /// " · " as ", "; in its own text only, a name or path is kept as it is.
    pub fn plain(&self) -> String {
        let words = |parts: &[Part]| -> String {
            parts
                .iter()
                .map(|p| match p {
                    Part::Text(t) => t.replace(" \u{2192} ", " into ").replace(" \u{b7} ", ", "),
                    Part::Ref(t) | Part::Path(t) => t.clone(),
                })
                .collect()
        };
        let title = words(&self.title);
        let note = words(&self.note);
        if note.is_empty() {
            return title;
        }
        let stop = if title.ends_with(['.', ':', '!', '?']) {
            " "
        } else {
            ". "
        };
        let end = if note.ends_with(['.', '!', '?', ')', '\u{201d}']) {
            ""
        } else {
            "."
        };
        format!("{title}{stop}{note}{end}")
    }
}

/// The line of git's output `output` that says most about why it failed, for a notice: the
/// first `fatal:` or `error:` line (without that word, its first letter in upper case), else
/// the first line that is not a hint, as it is; cut at [`MAX_QUOTED`] characters. `None` when
/// there is no line.
pub(crate) fn telling_line(output: &str) -> Option<String> {
    let lines: Vec<&str> = output
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("hint:"))
        .collect();
    // git's own error, its label taken off, starts a sentence; anything else (a hook's
    // line) is quoted as it is.
    let labelled = lines.iter().find_map(|l| {
        l.strip_prefix("fatal:")
            .or_else(|| l.strip_prefix("error:"))
            .map(str::trim)
    });
    let mut out = match labelled {
        Some(line) => capitalized(&unmarked(line)),
        None => unmarked(lines.first()?),
    };
    if out.is_empty() {
        return None;
    }
    out = short_hashes(&out);
    if out.chars().count() > MAX_QUOTED {
        out = out.chars().take(MAX_QUOTED - 1).collect::<String>();
        out = format!("{}\u{2026}", out.trim_end());
    }
    Some(out)
}

/// `line` with each full commit hash in it (40 or 64 hex digits, a word of its own) cut to its
/// first 8, as gg shows hashes: a quoted line then wraps between words, not inside a hash.
fn short_hashes(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        let full = matches!(word.len(), 40 | 64) && word.chars().all(|c| c.is_ascii_hexdigit());
        out.push_str(if full { &word[..8] } else { word });
        word.clear();
    };
    for c in line.chars() {
        if c.is_ascii_alphanumeric() {
            word.push(c);
        } else {
            flush(&mut word, &mut out);
            out.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_reads_into_a_headline_and_a_line_with_its_names() {
        let message = said(
            "Pull request #990 opened",
            format!(
                "{} \u{2192} {} \u{b7} pushed to origin",
                name("shk/x"),
                name("main")
            ),
        );
        assert_eq!(
            notice(&message),
            Notice {
                title: vec![Part::Text("Pull request #990 opened".into())],
                note: vec![
                    Part::Ref("shk/x".into()),
                    Part::Text(" \u{2192} ".into()),
                    Part::Ref("main".into()),
                    Part::Text(" \u{b7} pushed to origin".into()),
                ],
            }
        );
        assert_eq!(
            notice(&message).plain(),
            "Pull request #990 opened. shk/x into main, pushed to origin."
        );
    }

    /// A message written as sentences, with no headline marked, has the shape all the same.
    #[test]
    fn plain_sentences_are_shaped() {
        let n = notice("Nothing was committed: the message is empty. Write one.");
        assert_eq!(text_of(&n.title), "Nothing was committed");
        assert_eq!(text_of(&n.note), "The message is empty. Write one.");
        let n = notice("It changed since you confirmed. Nothing was deleted.");
        assert_eq!(text_of(&n.title), "It changed since you confirmed");
        assert_eq!(text_of(&n.note), "Nothing was deleted.");
        // One sentence: a headline alone, its full stop gone.
        let n = notice("Copied the hash.");
        assert_eq!(text_of(&n.title), "Copied the hash");
        assert!(n.note.is_empty());
        // Never split inside a name, nor at a colon that is not after "Nothing was".
        let n = notice(&format!("Deleted {}. Done.", name("a. B")));
        assert_eq!(
            n.title,
            vec![Part::Text("Deleted ".into()), Part::Ref("a. B".into())]
        );
        let n = notice("Nothing was created: git still lists it.");
        assert_eq!(text_of(&n.note), "git still lists it.");
        let n = notice(&format!(
            "Nothing was committed: {} has conflicts.",
            path("notes")
        ));
        assert_eq!(n.note[0], Part::Path("notes".into()));
        let n = notice("Removed the gg command: ~/bin/gg.");
        assert_eq!(text_of(&n.title), "Removed the gg command");
        assert_eq!(text_of(&n.note), "~/bin/gg.");
        // A long lead is a sentence with a colon in it; a colon in a later sentence is not
        // looked at.
        let long = "Some files have no conflict markers to go by and so on and so forth: a, b.";
        assert_eq!(text_of(&notice(long).title), long.trim_end_matches('.'));
        let n = notice("Pick one. Then: go.");
        assert_eq!(text_of(&n.title), "Pick one");
        // A colon with no space after it (a time, a drive) is no break.
        assert!(notice("Fetched at 14:05 today").note.is_empty());
        // Not a sentence's end: a lower-case word follows.
        assert!(notice("Version 2. of it").note.is_empty());
    }

    #[test]
    fn paths_and_plain_text() {
        let message = format!("Deleted the worktree at {}", path("/w/x"));
        assert_eq!(
            notice(&message).title,
            vec![
                Part::Text("Deleted the worktree at ".into()),
                Part::Path("/w/x".into())
            ]
        );
        assert!(notice(&message).note.is_empty());
        assert_eq!(notice("Stopped.").plain(), "Stopped");
        assert_eq!(notice("").title, Vec::<Part>::new());
    }

    /// A name can never end its own mark early, nor start a line under the headline.
    #[test]
    fn marks_in_a_name_are_taken_out() {
        let odd = format!("a{REF_CLOSE}b{NOTE}c{PATH_OPEN}");
        let message = format!("Deleted branch {}", name(&odd));
        assert_eq!(
            notice(&message).title,
            vec![
                Part::Text("Deleted branch ".into()),
                Part::Ref("abc".into())
            ]
        );
    }

    #[test]
    fn notes_are_added_to() {
        let one = add_note("Merged", "Changes kept.");
        assert_eq!(notice(&one).plain(), "Merged. Changes kept.");
        let two = add_note(&one, "More.");
        assert_eq!(note_of(&two), "Changes kept. More.");
        assert_eq!(title_of(&two), "Merged");
        // A headline that ends a sentence, a line that ends in a quote.
        assert_eq!(
            notice(&said("Done.", "It said \u{201c}x\u{201d}")).plain(),
            "Done. It said \u{201c}x\u{201d}"
        );
        assert_eq!(notice(&said("Why:", "this")).plain(), "Why: this.");
        // A message of plain sentences keeps its short headline: what is added goes under it.
        let refused = add_note(
            "Nothing was deleted: /w contains another repository. Move it first.",
            "The main worktree is active now.",
        );
        assert_eq!(title_of(&refused), "Nothing was deleted");
        assert_eq!(
            note_of(&refused),
            "/w contains another repository. Move it first. The main worktree is active now."
        );
        let one = add_note("Deleted the worktree", "More.");
        assert_eq!(
            (title_of(&one), note_of(&one)),
            ("Deleted the worktree", "More.")
        );
    }

    /// The short forms read as words in a notice's own text; a name or path that has them in
    /// it is kept as it is.
    #[test]
    fn a_name_is_kept_as_it_is_in_the_sentence() {
        let message = said(
            "Nothing was committed",
            format!(
                "{} \u{2192} {} \u{b7} {} is inside a link.",
                name("a"),
                name("b"),
                path("x \u{2192} y \u{b7} z.txt")
            ),
        );
        assert_eq!(
            notice(&message).plain(),
            "Nothing was committed. a into b, x \u{2192} y \u{b7} z.txt is inside a link."
        );
    }

    #[test]
    fn the_telling_line_is_gits_error_without_its_label() {
        let out =
            "Updating abc..def\nhint: try this\nfatal: refusing to merge unrelated histories\n";
        assert_eq!(
            telling_line(out).as_deref(),
            Some("Refusing to merge unrelated histories")
        );
        assert_eq!(
            telling_line("\n  hint: x\nmerge: nope - not something we can merge\n").as_deref(),
            Some("merge: nope - not something we can merge")
        );
        assert_eq!(telling_line(" \n hint: only\n"), None);
        let hash = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(
            telling_line(&format!("merge: {hash} - not something we can merge")).as_deref(),
            Some("merge: 01234567 - not something we can merge")
        );
        // Not a word of its own, or not 40 digits: as it is.
        assert_eq!(
            short_hashes(&format!("x{hash} abc12345")),
            format!("x{hash} abc12345")
        );
        let long = format!("error: {}", "x".repeat(400));
        let line = telling_line(&long).unwrap();
        assert_eq!(line.chars().count(), MAX_QUOTED);
        assert!(line.ends_with('\u{2026}'));
    }
}
