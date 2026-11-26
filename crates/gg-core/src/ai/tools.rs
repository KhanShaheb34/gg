//! One adapter per command-line AI tool: its name and where to get it, how its version reads,
//! the command line a prompt is run with, where its answer is, and what its failures mean.
//! Adding a tool is one more [`Adapter`] in [`ADAPTERS`] (and a variant of
//! [`AiTool`](crate::dto::AiTool)).
//!
//! Every command line is fixed words, the model (checked: [`check_model`]) and a path gg made;
//! the prompt, with the diff in it, goes on standard input. The diff is not to be trusted (a
//! repository's text may ask the model to do things), so neither tool can run a command or
//! read a file: Claude Code runs with no tools (`--tools ""`), no MCP servers and no settings
//! files (so none of their hooks); Codex with its shell and image viewer off, no web search,
//! no MCP servers, in its read-only sandbox. Each keeps no session and runs in a temporary
//! folder of its own, never in the repository.

use std::path::{Path, PathBuf};

use crate::dto::{AiFailureKind, AiTool};

/// The command line for one prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    /// The arguments after the program.
    pub args: Vec<String>,
    /// What it reads on standard input: the prompt.
    pub stdin: Vec<u8>,
    /// The file it writes its answer to, when it does (else its standard output is the answer).
    pub answer_file: Option<PathBuf>,
}

/// How a run of a tool ended, for [`Adapter::classify`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Exit {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// A command-line AI tool.
pub trait Adapter: Sync {
    fn tool(&self) -> AiTool;
    /// Its name for people: "Claude Code".
    fn name(&self) -> &'static str;
    /// Its program's name (`claude`), as it is looked up and as the command log shows it.
    fn program(&self) -> &'static str;
    /// Who it sends prompts to: "Anthropic".
    fn provider(&self) -> &'static str;
    /// Its official install page.
    fn install_url(&self) -> &'static str;
    /// What to run in a terminal to sign in.
    fn sign_in(&self) -> &'static str;
    /// gg's default model for it ("": the tool's own).
    fn default_model(&self) -> &'static str;
    /// Models offered in the settings (any other can be typed).
    fn models(&self) -> &'static [&'static str];
    /// Its version in what `<program> --version` printed.
    fn version_of(&self, printed: &str) -> Option<String>;
    /// The command line that writes an answer to `prompt` with `model` ("": the tool's own),
    /// in `scratch` (a folder of gg's for this run).
    fn invocation(&self, model: &str, prompt: &str, scratch: &Path) -> Invocation;
    /// Its answer in what it printed on stdout (when it wrote none to a file).
    fn answer_of(&self, stdout: &str) -> String {
        stdout.to_owned()
    }
    /// The command line to run again after `run` failed with `exit` because of a setting this
    /// version of the tool does not take, when that setting can be left out.
    fn retry(&self, _run: &Invocation, _exit: &Exit) -> Option<Invocation> {
        None
    }
    /// A plain sentence for a failure gg recognises and the user can act on (before
    /// [`Adapter::classify`]).
    fn advice(&self, _exit: &Exit) -> Option<String> {
        None
    }
    /// What a run that did not answer means, when it can tell: not signed in, a limit, a model
    /// it does not know; with the tool's own line about it. `None`: a failure of another kind.
    fn classify(&self, exit: &Exit) -> Option<(AiFailureKind, String)>;
    /// The tool's own line about a failure, for the message.
    fn failure_line(&self, exit: &Exit) -> Option<String>;
}

/// The adapters, in the order "Automatic" tries them.
pub static ADAPTERS: [&dyn Adapter; 2] = [&Claude, &Codex];

/// The adapter of `tool`.
pub fn adapter(tool: AiTool) -> &'static dyn Adapter {
    match tool {
        AiTool::Claude => &Claude,
        AiTool::Codex => &Codex,
    }
}

/// Whether `model` can be put on a command line: a name as models are named (letters,
/// digits and `.-_:/@[]`, at most 100), never starting with `-` or `@`, which a tool could
/// read as an option or a file.
pub fn check_model(model: &str) -> Result<(), String> {
    if model.is_empty() {
        return Ok(());
    }
    if model.len() > 100 {
        return Err("A model name is at most 100 characters.".to_owned());
    }
    let first = model.chars().next().unwrap_or(' ');
    if !first.is_ascii_alphanumeric() {
        return Err("A model name starts with a letter or a digit.".to_owned());
    }
    if let Some(bad) = model
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || "._-:/@[]".contains(*c)))
    {
        return Err(format!(
            "A model name has letters, digits and . - _ : / @ [ ] only, not \u{201c}{bad}\u{201d}."
        ));
    }
    Ok(())
}

/// The first line of `text` that says something, trimmed.
fn first_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_owned)
}

fn has_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| text.contains(n))
}

// --- Claude Code --------------------------------------------------------------------------

/// Claude Code: `claude -p` with no tools, no MCP servers, no settings files, no slash
/// commands and no session kept; a short system prompt of gg's in place of Claude Code's own
/// (which describes its tools and the folder it runs in); the prompt on stdin, the answer on
/// stdout.
pub struct Claude;

/// The system prompt Claude Code is run with.
const CLAUDE_SYSTEM: &str = "You write commit messages and pull request descriptions from the changes you are given. Follow the instructions in the message exactly and answer with the requested text only.";

impl Adapter for Claude {
    fn tool(&self) -> AiTool {
        AiTool::Claude
    }
    fn name(&self) -> &'static str {
        "Claude Code"
    }
    fn program(&self) -> &'static str {
        "claude"
    }
    fn provider(&self) -> &'static str {
        "Anthropic"
    }
    fn install_url(&self) -> &'static str {
        "https://code.claude.com/docs/en/setup"
    }
    fn sign_in(&self) -> &'static str {
        "claude"
    }
    fn default_model(&self) -> &'static str {
        "haiku"
    }
    fn models(&self) -> &'static [&'static str] {
        &["haiku", "sonnet", "opus"]
    }
    fn version_of(&self, printed: &str) -> Option<String> {
        // "2.1.332 (Claude Code)"
        let word = printed.split_whitespace().next()?;
        word.starts_with(|c: char| c.is_ascii_digit())
            .then(|| word.to_owned())
    }
    fn invocation(&self, model: &str, prompt: &str, _scratch: &Path) -> Invocation {
        let mut args: Vec<String> = ["-p", "--output-format", "text"]
            .map(str::to_owned)
            .to_vec();
        if !model.is_empty() {
            args.extend(["--model".to_owned(), model.to_owned()]);
        }
        args.extend(
            [
                "--tools",
                "",
                "--no-session-persistence",
                "--strict-mcp-config",
                "--setting-sources",
                "",
                "--disable-slash-commands",
                "--system-prompt",
                CLAUDE_SYSTEM,
            ]
            .map(str::to_owned),
        );
        Invocation {
            args,
            stdin: prompt.as_bytes().to_vec(),
            answer_file: None,
        }
    }
    fn classify(&self, exit: &Exit) -> Option<(AiFailureKind, String)> {
        // Claude Code says what went wrong on stdout in print mode.
        let all = format!("{}\n{}", exit.stdout, exit.stderr).to_lowercase();
        let line = self.failure_line(exit).unwrap_or_default();
        if has_any(
            &all,
            &[
                "not logged in",
                "please run /login",
                "invalid api key",
                "oauth token has expired",
                "oauth token revoked",
                "authentication_error",
            ],
        ) {
            return Some((AiFailureKind::SignedOut, line));
        }
        if has_any(
            &all,
            &[
                "usage limit",
                "hit your limit",
                "rate limit",
                "rate_limit",
                "credit balance is too low",
            ],
        ) {
            return Some((AiFailureKind::Limited, line));
        }
        if all.contains("issue with the selected model") {
            return Some((AiFailureKind::BadModel, line));
        }
        None
    }
    fn failure_line(&self, exit: &Exit) -> Option<String> {
        first_line(&exit.stdout).or_else(|| first_line(&exit.stderr))
    }
}

// --- Codex --------------------------------------------------------------------------------

/// Codex: `codex exec` in its read-only sandbox, ephemeral (no session kept), outside any
/// repository (`--skip-git-repo-check`), without the user's `config.toml`
/// (`--ignore-user-config`: its MCP servers, hooks and profiles; the sign-in is still Codex's
/// own), with low reasoning effort for speed; the prompt on stdin (`-`), the answer written to
/// a file of gg's (`--output-last-message`), and what happens as JSON events on stdout
/// (`--json`: Codex then repeats nothing of the prompt, and its errors are events).
pub struct Codex;

impl Adapter for Codex {
    fn tool(&self) -> AiTool {
        AiTool::Codex
    }
    fn name(&self) -> &'static str {
        "Codex"
    }
    fn program(&self) -> &'static str {
        "codex"
    }
    fn provider(&self) -> &'static str {
        "OpenAI"
    }
    fn install_url(&self) -> &'static str {
        "https://developers.openai.com/codex/cli"
    }
    fn sign_in(&self) -> &'static str {
        "codex login"
    }
    fn default_model(&self) -> &'static str {
        ""
    }
    fn models(&self) -> &'static [&'static str] {
        &["gpt-5.4-mini", "gpt-5.5", "gpt-5.6-terra"]
    }
    fn version_of(&self, printed: &str) -> Option<String> {
        // "codex-cli 0.163.0"
        let word = printed.split_whitespace().last()?;
        word.starts_with(|c: char| c.is_ascii_digit())
            .then(|| word.to_owned())
    }
    fn invocation(&self, model: &str, prompt: &str, scratch: &Path) -> Invocation {
        let answer = scratch.join("answer.txt");
        let mut args: Vec<String> = [
            "exec",
            "--ephemeral",
            "--skip-git-repo-check",
            "--ignore-user-config",
            "--json",
            "--sandbox",
            "read-only",
            "--color",
            "never",
        ]
        .map(str::to_owned)
        .to_vec();
        if !model.is_empty() {
            args.extend(["--model".to_owned(), model.to_owned()]);
        }
        for feature in CODEX_NO_COMMANDS.iter().chain(&CODEX_ALSO_OFF) {
            args.extend(["--disable".to_owned(), (*feature).to_owned()]);
        }
        for config in CODEX_CONFIG {
            args.extend(["-c".to_owned(), config.to_owned()]);
        }
        args.extend([
            "--output-last-message".to_owned(),
            answer.to_string_lossy().into_owned(),
            "-".to_owned(),
        ]);
        Invocation {
            args,
            stdin: prompt.as_bytes().to_vec(),
            answer_file: Some(answer),
        }
    }
    fn retry(&self, run: &Invocation, exit: &Exit) -> Option<Invocation> {
        // A feature this Codex does not know (older, or one it dropped) stops it before it
        // starts: run again without it, unless it is one that keeps commands off.
        let name = unknown_feature(&exit.stderr)?;
        if CODEX_NO_COMMANDS.contains(&name.as_str()) {
            return None;
        }
        let at = run
            .args
            .windows(2)
            .position(|w| w[0] == "--disable" && w[1] == name)?;
        let mut again = run.clone();
        again.args.drain(at..at + 2);
        Some(again)
    }
    fn advice(&self, exit: &Exit) -> Option<String> {
        let name = unknown_feature(&exit.stderr)
            .filter(|n| CODEX_NO_COMMANDS.contains(&n.as_str()))
            .or_else(|| unknown_option(&exit.stderr))?;
        Some(format!(
            "This Codex cannot be run as gg runs it, with its commands and your MCP servers off (it does not know \u{201c}{name}\u{201d}), so gg does not run it. Update Codex (codex update), then try again."
        ))
    }
    fn answer_of(&self, stdout: &str) -> String {
        // The last agent message among the events.
        codex_events(stdout)
            .iter()
            .rev()
            .filter(|e| e["type"] == "item.completed" && e["item"]["type"] == "agent_message")
            .find_map(|e| e["item"]["text"].as_str())
            .unwrap_or_default()
            .to_owned()
    }
    fn classify(&self, exit: &Exit) -> Option<(AiFailureKind, String)> {
        // Its error events and its own error lines; nothing of the prompt is in them.
        let errors = codex_errors(exit);
        let all = errors.join("\n").to_lowercase();
        let line = self.failure_line(exit).unwrap_or_default();
        if has_any(
            &all,
            &[
                "401 unauthorized",
                "not logged in",
                "codex login",
                "token_expired",
                "refresh token",
                "please log in",
            ],
        ) {
            return Some((AiFailureKind::SignedOut, line));
        }
        if has_any(
            &all,
            &[
                "usage limit",
                "429 too many requests",
                "rate limit",
                "quota",
            ],
        ) {
            return Some((AiFailureKind::Limited, line));
        }
        let model_said = errors.iter().any(|e| {
            let e = e.to_lowercase();
            e.contains("model_not_found")
                || (e.contains("model")
                    && has_any(
                        &e,
                        &[
                            "does not exist",
                            "not found",
                            "not supported",
                            "unknown model",
                            "invalid model",
                            "not a valid model",
                        ],
                    ))
        });
        if model_said {
            return Some((AiFailureKind::BadModel, line));
        }
        None
    }
    fn failure_line(&self, exit: &Exit) -> Option<String> {
        // Its last error, without the retries announced on the way.
        codex_errors(exit)
            .into_iter()
            .rfind(|e| !e.starts_with("Reconnecting"))
    }
}

/// The Codex features that would let the model run commands or read files: its shell
/// (`shell_tool`, and `unified_exec`, the shell it runs now) and `view_image` (reads an image
/// from disk). Turned off for every run; a Codex that does not know one is not run (its
/// read-only sandbox still lets commands read any file, and the diff is not to be trusted:
/// it could ask for a key file to be put in the answer).
pub const CODEX_NO_COMMANDS: [&str; 3] = ["shell_tool", "unified_exec", "view_image"];
/// Codex features also turned off, for less to go wrong: connectors and plugins, other agents,
/// hooks, tool suggestions, goals, the computer, the browser and image generation. One a
/// Codex does not know is left out.
const CODEX_ALSO_OFF: [&str; 10] = [
    "apps",
    "plugins",
    "multi_agent",
    "hooks",
    "tool_suggest",
    "goals",
    "computer_use",
    "browser_use",
    "in_app_browser",
    "image_generation",
];
/// Codex settings for every run: no web search, low reasoning effort (an answer in seconds).
/// (The user's MCP servers are kept out by `--ignore-user-config`: `-c mcp_servers={}` does not
/// remove those a config file names, as seen with Codex 0.163.)
const CODEX_CONFIG: [&str; 2] = ["web_search=disabled", "model_reasoning_effort=low"];

/// The feature Codex said it does not know (`Error: Unknown feature flag: <name>`).
fn unknown_feature(stderr: &str) -> Option<String> {
    stderr.lines().find_map(|l| {
        let (_, name) = l.split_once("Unknown feature flag: ")?;
        let name = name.trim();
        (!name.is_empty()).then(|| name.to_owned())
    })
}

/// The option Codex said it does not know (`error: unexpected argument '--json' found`).
fn unknown_option(stderr: &str) -> Option<String> {
    stderr.lines().find_map(|l| {
        let (_, rest) = l.split_once("unexpected argument '")?;
        let (name, _) = rest.split_once('\'')?;
        (!name.is_empty()).then(|| name.to_owned())
    })
}

/// The JSON events Codex printed on stdout (`--json`), one per line; other lines are skipped.
fn codex_events(stdout: &str) -> Vec<serde_json::Value> {
    stdout
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l.trim()).ok())
        .filter(serde_json::Value::is_object)
        .collect()
}

/// What Codex said went wrong: the messages of its `error` and `turn.failed` events, then its
/// own error lines on stderr (an option it refused before it started, its log's errors).
fn codex_errors(exit: &Exit) -> Vec<String> {
    let mut errors: Vec<String> = codex_events(&exit.stdout)
        .iter()
        .filter_map(|e| match e["type"].as_str() {
            Some("error") => e["message"].as_str(),
            Some("turn.failed") => e["error"]["message"].as_str(),
            _ => None,
        })
        .map(str::to_owned)
        .collect();
    errors.extend(
        crate::ai::prompt::strip_ansi(&exit.stderr)
            .lines()
            .map(str::trim)
            .filter_map(|l| {
                l.strip_prefix("ERROR: ")
                    .or_else(|| l.strip_prefix("Error: "))
                    .or_else(|| l.strip_prefix("error: "))
            })
            .map(str::to_owned),
    );
    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOSTILE: &str =
        "Fix \"quotes\", `backticks`, $(rm -rf ~) and $HOME\n-leading --dash\n@file\n'single'";

    #[test]
    fn the_prompt_goes_on_stdin_and_never_on_the_command_line() {
        let scratch = Path::new("/tmp/gg-ai-1");
        for a in ADAPTERS {
            let run = a.invocation("haiku", HOSTILE, scratch);
            assert_eq!(run.stdin, HOSTILE.as_bytes(), "{}", a.name());
            for arg in &run.args {
                assert!(!arg.contains("rm -rf"), "{}: {arg}", a.name());
                assert!(!arg.contains("backticks"), "{}: {arg}", a.name());
            }
        }
    }

    #[test]
    fn claude_runs_with_no_tools_and_no_session() {
        let run = Claude.invocation("haiku", "p", Path::new("/s"));
        assert_eq!(
            run.args,
            [
                "-p",
                "--output-format",
                "text",
                "--model",
                "haiku",
                "--tools",
                "",
                "--no-session-persistence",
                "--strict-mcp-config",
                "--setting-sources",
                "",
                "--disable-slash-commands",
                "--system-prompt",
                CLAUDE_SYSTEM,
            ]
        );
        assert_eq!(run.answer_file, None);
        // The tool's own default: no --model.
        let own = Claude.invocation("", "p", Path::new("/s"));
        assert!(!own.args.iter().any(|a| a == "--model"));
    }

    /// Codex runs no command and reads no file: its shell (`shell_tool`, `unified_exec`) and
    /// `view_image` are off (seen with Codex 0.163 against a stand-in model asking for
    /// `cat /etc/hosts`: "unsupported call"), as are web search and the user's MCP servers;
    /// its read-only sandbox stays, so `apply_patch` (which it still offers) writes nothing.
    #[test]
    fn codex_runs_no_commands_read_only_and_writes_its_answer_to_a_file() {
        let scratch = Path::new("/tmp/gg-ai-2");
        let run = Codex.invocation("", "p", scratch);
        let answer = scratch.join("answer.txt");
        let has = |pair: [&str; 2]| run.args.windows(2).any(|w| w == pair);
        for feature in [
            "shell_tool",
            "unified_exec",
            "view_image",
            "apps",
            "plugins",
            "hooks",
        ] {
            assert!(has(["--disable", feature]), "{feature}: {:?}", run.args);
        }
        assert!(has(["--sandbox", "read-only"]));
        assert!(has(["-c", "web_search=disabled"]));
        assert!(has(["-c", "model_reasoning_effort=low"]));
        // The user's config.toml (its MCP servers) is not loaded: `-c mcp_servers={}` does not
        // remove the servers a config names (seen with Codex 0.163: a dummy server started).
        assert!(!run.args.iter().any(|a| a.starts_with("mcp_servers")));
        assert_eq!(
            &run.args[..5],
            [
                "exec",
                "--ephemeral",
                "--skip-git-repo-check",
                "--ignore-user-config",
                "--json"
            ]
        );
        assert_eq!(
            &run.args[run.args.len() - 3..],
            ["--output-last-message", &answer.to_string_lossy(), "-"]
        );
        assert!(!run.args.iter().any(|a| a == "--model"));
        assert_eq!(run.answer_file, Some(answer));
        let with = Codex.invocation("gpt-5.4-mini", "p", scratch);
        let at = with.args.iter().position(|a| a == "--model").unwrap();
        assert_eq!(with.args[at + 1], "gpt-5.4-mini");
    }

    /// A Codex that does not know an optional feature runs without that switch; one that does
    /// not know a switch keeping commands off is not run at all.
    #[test]
    fn codex_without_a_feature_runs_without_it_unless_it_keeps_commands_off() {
        let run = Codex.invocation("", "p", Path::new("/s"));
        let said = |name: &str| exit(1, "", &format!("Error: Unknown feature flag: {name}\n"));
        let again = Codex.retry(&run, &said("goals")).unwrap();
        assert_eq!(again.args.len(), run.args.len() - 2);
        assert!(!again.args.iter().any(|a| a == "goals"));
        assert!(again.args.iter().any(|a| a == "shell_tool"));
        assert_eq!(Codex.advice(&said("goals")), None);
        for essential in CODEX_NO_COMMANDS {
            assert_eq!(Codex.retry(&run, &said(essential)), None);
            assert!(
                Codex
                    .advice(&said(essential))
                    .unwrap()
                    .contains("Update Codex")
            );
        }
        assert_eq!(Codex.retry(&run, &exit(1, "", "ERROR: boom")), None);
        assert_eq!(Claude.retry(&run, &said("goals")), None);
        // A Codex too old for --ignore-user-config (or --json) is not run without it.
        let old = exit(
            2,
            "",
            "error: unexpected argument '--ignore-user-config' found\n",
        );
        assert_eq!(Codex.retry(&run, &old), None);
        let advice = Codex.advice(&old).unwrap();
        assert!(
            advice.contains("\u{201c}--ignore-user-config\u{201d}"),
            "{advice}"
        );
        assert!(advice.contains("Update Codex"));
    }

    /// Codex's answer, when it wrote none to its file, is the last agent message among its
    /// JSON events.
    #[test]
    fn codex_answers_in_its_events() {
        let events = concat!(
            "{\"type\":\"thread.started\",\"thread_id\":\"t\"}\n",
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"First\"}}\n",
            "not json\n",
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"Fix it\"}}\n",
            "{\"type\":\"turn.completed\"}\n",
        );
        assert_eq!(Codex.answer_of(events), "Fix it");
        assert_eq!(Codex.answer_of("plain text"), "");
        assert_eq!(Claude.answer_of("plain text"), "plain text");
    }

    #[test]
    fn model_names_cannot_be_options_or_files() {
        for good in [
            "",
            "haiku",
            "claude-haiku-4-5-20251001",
            "sonnet[1m]",
            "us.anthropic.claude-sonnet-4",
            "gpt-5.4-mini",
            "openrouter/qwen:free",
        ] {
            assert_eq!(check_model(good), Ok(()), "{good}");
        }
        for bad in [
            "-p", "--model", "@file", "a b", "a;b", "$(x)", "a`b", "\"x\"", "x\ny",
        ] {
            assert!(check_model(bad).is_err(), "{bad}");
        }
        assert!(check_model(&"a".repeat(101)).is_err());
    }

    #[test]
    fn versions_are_read() {
        assert_eq!(
            Claude.version_of("2.1.332 (Claude Code)\n").as_deref(),
            Some("2.1.332")
        );
        assert_eq!(
            Codex.version_of("codex-cli 0.163.0\n").as_deref(),
            Some("0.163.0")
        );
        assert_eq!(Claude.version_of("usage: …"), None);
        assert_eq!(Codex.version_of(""), None);
    }

    fn exit(code: i32, stdout: &str, stderr: &str) -> Exit {
        Exit {
            code,
            stdout: stdout.into(),
            stderr: stderr.into(),
        }
    }

    /// What each tool says, as seen from the real tools (Claude Code 2.1, Codex 0.163).
    #[test]
    fn failures_are_told_apart() {
        let kind = |a: &dyn Adapter, e: Exit| a.classify(&e).map(|(k, _)| k);
        assert_eq!(
            kind(&Claude, exit(1, "Not logged in · Please run /login\n", "")),
            Some(AiFailureKind::SignedOut)
        );
        assert_eq!(
            kind(&Claude, exit(1, "Invalid API key · Please run /login", "")),
            Some(AiFailureKind::SignedOut)
        );
        assert_eq!(
            kind(&Claude, exit(1, "You've hit your limit · resets 3pm", "")),
            Some(AiFailureKind::Limited)
        );
        assert_eq!(
            kind(
                &Claude,
                exit(
                    1,
                    "There's an issue with the selected model (x). It may not exist or you may not have access to it.",
                    "Claude Code doesn't recognize the model \"x\""
                )
            ),
            Some(AiFailureKind::BadModel)
        );
        assert_eq!(kind(&Claude, exit(1, "Something else broke", "")), None);
        assert_eq!(
            Claude
                .classify(&exit(1, "\nNot logged in · Please run /login\n", ""))
                .unwrap()
                .1,
            "Not logged in · Please run /login"
        );

        // Codex with --json: its errors are events on stdout (seen with Codex 0.163).
        let signed_out = concat!(
            "{\"type\":\"thread.started\",\"thread_id\":\"t\"}\n",
            "{\"type\":\"error\",\"message\":\"Reconnecting... 5/5\"}\n",
            "{\"type\":\"turn.failed\",\"error\":{\"message\":\"unexpected status 401 Unauthorized: Missing bearer or basic authentication in header, url: https://api.openai.com/v1/responses\"}}\n",
        );
        assert_eq!(
            Codex.classify(&exit(1, signed_out, "")),
            Some((
                AiFailureKind::SignedOut,
                "unexpected status 401 Unauthorized: Missing bearer or basic authentication in header, url: https://api.openai.com/v1/responses".into()
            ))
        );
        assert_eq!(
            kind(
                &Codex,
                exit(
                    1,
                    "{\"type\":\"error\",\"message\":\"You've hit your usage limit. Try again later.\"}\n",
                    ""
                )
            ),
            Some(AiFailureKind::Limited)
        );
        assert_eq!(
            kind(
                &Codex,
                exit(
                    1,
                    "",
                    "ERROR: The model `gpt-x` does not exist or you do not have access to it.\n"
                )
            ),
            Some(AiFailureKind::BadModel)
        );
        // Words of an answer or of another event are not its errors.
        let answer = "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"rate limit, usage limit, not logged in\"}}\n{\"type\":\"error\",\"message\":\"boom\"}\n";
        assert_eq!(kind(&Codex, exit(1, answer, "")), None);
        assert_eq!(
            Codex.failure_line(&exit(1, answer, "")).as_deref(),
            Some("boom")
        );
    }
}
