//! "Generate with AI" end to end in the core, against the recording stand-in for the AI tools
//! (`gg-fake-ai`, copied into a tool `PATH` as `claude` and `codex`): what is found, what each
//! tool is run with and given, what its answers and failures become, cancelling and the time
//! limit. No real AI tool runs, nothing reaches the network. Runs on every platform.

use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gg_core::dto::{AiFailureKind, AiOutcome, AiTask, AiTool, AiToolChoice, Settings};
use gg_core::git::runner::{CommandLog, GitRunner};
use gg_core::repo::RepoManager;
use gg_core::testutil::{RecordingSink, TestRepo};

const FAKE: &str = env!("CARGO_BIN_EXE_gg-fake-ai");

/// The tool folder: copies of the stand-in named as the tools.
struct Tools {
    dir: tempfile::TempDir,
}

/// One recorded call of a stand-in.
#[derive(Debug)]
struct Call {
    argv: Vec<String>,
    stdin: String,
    cwd: PathBuf,
    env: String,
    done: bool,
}

impl Tools {
    fn new(names: &[&str]) -> Self {
        let tools = Self {
            dir: tempfile::Builder::new()
                .prefix("gg-fake-ai-")
                .tempdir()
                .unwrap(),
        };
        for name in names {
            tools.install(name);
        }
        tools
    }

    fn exe(&self, name: &str) -> PathBuf {
        self.dir
            .path()
            .join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
    }

    fn state(&self, name: &str) -> PathBuf {
        let dir = self.dir.path().join(format!("{name}.fake"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn install(&self, name: &str) {
        fs::copy(FAKE, self.exe(name)).unwrap();
    }

    fn uninstall(&self, name: &str) {
        fs::remove_file(self.exe(name)).unwrap();
    }

    fn path(&self) -> OsString {
        self.dir.path().as_os_str().to_owned()
    }

    fn set(&self, name: &str, file: &str, contents: &str) {
        fs::write(self.state(name).join(file), contents).unwrap();
    }

    fn reply(&self, name: &str, code: i32, out: &str, err: &str) {
        self.set(name, "reply.code", &code.to_string());
        self.set(name, "reply.out", out);
        self.set(name, "reply.err", err);
    }

    fn calls(&self, name: &str) -> Vec<Call> {
        let mut out = Vec::new();
        for n in 1.. {
            let c = self.state(name).join("calls").join(n.to_string());
            if !c.is_dir() {
                break;
            }
            let text = |f: &str| fs::read_to_string(c.join(f)).unwrap_or_default();
            out.push(Call {
                argv: text("argv")
                    .split('\0')
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
                    .split_last()
                    .map(|(_, rest)| rest.to_vec())
                    .unwrap_or_default(),
                stdin: text("stdin"),
                cwd: PathBuf::from(text("cwd")),
                env: text("env"),
                done: c.join("done").exists(),
            });
        }
        out
    }

    /// Waits until `name` has been started `n` times.
    async fn wait_started(&self, name: &str, n: usize) {
        for _ in 0..400 {
            let started = (1..=n).all(|i| {
                self.state(name)
                    .join("calls")
                    .join(i.to_string())
                    .join("started")
                    .exists()
            });
            if started {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("{name} was not started");
    }
}

struct Setup {
    m: Arc<RepoManager>,
    log: Arc<CommandLog>,
    tools: Tools,
    repo: TestRepo,
    id: u32,
    /// The repository's folder as the core names it.
    root: String,
}

async fn setup(names: &[&str]) -> Setup {
    let log = CommandLog::new();
    let runner = GitRunner::detect(log.clone()).await.expect("git");
    let m = RepoManager::new(runner, Arc::new(RecordingSink::default()));
    let tools = Tools::new(names);
    m.set_tool_path(Some(tools.path()));
    let repo = TestRepo::new();
    repo.write("README.md", "# demo\n");
    repo.commit_all("Start");
    let snap = m.open(repo.path()).await.expect("open");
    Setup {
        m,
        log,
        tools,
        repo,
        id: snap.repo_id,
        root: snap.root_path,
    }
}

fn settings(tool: AiToolChoice) -> Settings {
    Settings {
        ai_tool: tool,
        ..Settings::default()
    }
}

impl Setup {
    fn worktree(&self) -> String {
        self.root.clone()
    }

    fn commit_task(&self, paths: &[&str]) -> AiTask {
        AiTask::Commit {
            path: self.worktree(),
            paths: paths.iter().map(|p| (*p).to_owned()).collect(),
        }
    }

    async fn generate(&self, task: AiTask, settings: &Settings) -> AiOutcome {
        tokio::time::timeout(
            Duration::from_secs(60),
            self.m.ai_generate(self.id, "req-1", task, settings),
        )
        .await
        .expect("answers in time")
        .expect("no error")
    }
}

fn failure(outcome: &AiOutcome) -> (AiFailureKind, String) {
    match outcome {
        AiOutcome::Failed { kind, message, .. } => (*kind, message.clone()),
        other => panic!("expected a failure, got {other:?}"),
    }
}

#[tokio::test]
async fn tools_are_found_in_gg_s_order() {
    let s = setup(&[]).await;
    let auto = settings(AiToolChoice::Auto);
    let none = s.m.ai_tools(&auto, false, false).await;
    assert_eq!(none.chosen, None);
    assert!(none.tools.iter().all(|t| !t.installed));
    assert_eq!(none.tools[0].name, "Claude Code");
    assert_eq!(
        none.tools[0].install_url,
        "https://code.claude.com/docs/en/setup"
    );

    s.tools.install("codex");
    let one = s.m.ai_tools(&auto, true, false).await;
    assert_eq!(one.chosen, Some(AiTool::Codex));
    assert_eq!(one.tools[1].version.as_deref(), Some("0.0.1"));
    assert_eq!(one.tools[0].version, None);
    // Chosen in the settings but not there: none runs.
    let claude =
        s.m.ai_tools(&settings(AiToolChoice::Claude), false, false)
            .await;
    assert_eq!(claude.chosen, None);

    s.tools.install("claude");
    s.tools.set("claude", "version", "2.1.332 (Claude Code)\n");
    let both = s.m.ai_tools(&auto, true, false).await;
    assert_eq!(both.chosen, Some(AiTool::Claude));
    assert_eq!(both.tools[0].version.as_deref(), Some("2.1.332"));
    let codex =
        s.m.ai_tools(&settings(AiToolChoice::Codex), false, false)
            .await;
    assert_eq!(codex.chosen, Some(AiTool::Codex));

    // Gone again: Check again sees it.
    s.tools.uninstall("claude");
    let after = s.m.ai_tools(&auto, false, true).await;
    assert_eq!(after.chosen, Some(AiTool::Codex));
}

#[tokio::test]
async fn no_tool_says_so() {
    let s = setup(&[]).await;
    s.repo.write("a.txt", "a\n");
    let out = s
        .generate(s.commit_task(&["a.txt"]), &settings(AiToolChoice::Auto))
        .await;
    assert_eq!(
        failure(&out),
        (
            AiFailureKind::NoTool,
            "Neither Claude Code nor Codex was found on this computer.".into()
        )
    );
    let out = s
        .generate(s.commit_task(&["a.txt"]), &settings(AiToolChoice::Codex))
        .await;
    assert_eq!(
        failure(&out),
        (
            AiFailureKind::NoTool,
            "Codex was not found on this computer.".into()
        )
    );
}

/// The message is written from what a commit of the ticked files would record, and only
/// that: the prompt on stdin, the tool in a folder of its own outside the repository.
#[tokio::test]
async fn a_commit_message_is_written_from_the_ticked_changes() {
    let s = setup(&["claude"]).await;
    s.repo.write(
        "README.md",
        "# demo\n\nA fast history viewer with $(rm -rf ~) and `ticks`.\n",
    );
    s.repo.write("notes.txt", "not ticked\n");
    s.repo.write("img.bin", [0u8, 1, 2, 0, 255, 254]);
    s.repo.write("latin1.txt", [b'c', b'a', b'f', 0xe9, b'\n']);
    s.repo.write("src/new.rs", "fn main() {}\n");
    s.tools.reply(
        "claude",
        0,
        "```\nAdd a tagline to the README.\n\nSo readers know what it is.\n```\n",
        "",
    );
    let out = s
        .generate(
            s.commit_task(&["README.md", "img.bin", "latin1.txt", "src/new.rs"]),
            &settings(AiToolChoice::Auto),
        )
        .await;
    let AiOutcome::Done {
        tool,
        model,
        message,
        log_from,
        ..
    } = out
    else {
        panic!("{out:?}")
    };
    assert_eq!(tool, AiTool::Claude);
    assert_eq!(model, "haiku");
    assert_eq!(
        message.as_deref(),
        Some("Add a tagline to the README\n\nSo readers know what it is.")
    );

    let calls = s.tools.calls("claude");
    assert_eq!(calls.len(), 1);
    let call = &calls[0];
    assert_eq!(
        &call.argv[..5],
        ["-p", "--output-format", "text", "--model", "haiku"]
    );
    assert!(call.argv.windows(2).any(|w| w == ["--tools", ""]));
    // The git it might run is as protected as gg's own.
    assert!(call.env.contains("GIT_TERMINAL_PROMPT=0"), "{}", call.env);
    let stdin = &call.stdin;
    assert!(stdin.starts_with("You are generating a single git commit message."));
    assert!(
        stdin.contains("+A fast history viewer with $(rm -rf ~) and `ticks`."),
        "{stdin}"
    );
    assert!(
        stdin.contains("Binary files /dev/null and b/img.bin differ"),
        "{stdin}"
    );
    assert!(stdin.contains("+caf\u{fffd}"), "{stdin}");
    assert!(
        stdin.contains("+++ b/src/new.rs\n@@ -0,0 +1,1 @@\n+fn main() {}"),
        "{stdin}"
    );
    assert!(stdin.contains("M\tREADME.md"));
    assert!(
        !stdin.contains("not ticked"),
        "an unticked file is not sent"
    );
    // Run outside the repository, in a folder that is gone afterwards.
    let repo = fs::canonicalize(s.repo.path()).unwrap();
    assert!(!call.cwd.starts_with(&repo), "{:?}", call.cwd);
    assert!(!call.cwd.exists(), "the scratch folder is removed");
    // Nothing in the repository changed: the file still untracked, nothing staged.
    assert_eq!(s.repo.git(&["diff", "--cached", "--name-only"]), "");

    // The log names the generation, never holds the prompt.
    let records = s.log.since(log_from - 1);
    let claude = records.iter().find(|r| r.program == "claude").unwrap();
    assert_eq!(
        claude.op_title.as_deref(),
        Some("Write a commit message with Claude Code (haiku)")
    );
    for r in &records {
        let all = format!("{:?}{}{}", r.args, r.stdout, r.stderr);
        assert!(!all.contains("rm -rf"), "{r:?}");
    }
}

#[tokio::test]
async fn codex_answers_in_its_file_and_its_stderr_is_not_logged() {
    let s = setup(&["codex"]).await;
    // A line of the diff that looks like Codex's own warnings: it is still never logged.
    s.repo.write(
        "README.md",
        "# demo\nWarning: secret-ish line\nERROR: secret-ish too\n",
    );
    s.tools.reply("codex", 0, "progress noise\n", "");
    s.tools.set("codex", "reply.last", "Describe the demo\n");
    s.tools.set("codex", "reply.echo", "");
    let mut with_model = settings(AiToolChoice::Auto);
    with_model
        .ai_models
        .insert("codex".into(), "gpt-5.4-mini".into());
    let out = s.generate(s.commit_task(&["README.md"]), &with_model).await;
    let AiOutcome::Done {
        message, log_from, ..
    } = out
    else {
        panic!("{out:?}")
    };
    assert_eq!(message.as_deref(), Some("Describe the demo"));
    let call = &s.tools.calls("codex")[0];
    assert_eq!(
        &call.argv[..5],
        [
            "exec",
            "--ephemeral",
            "--skip-git-repo-check",
            "--ignore-user-config",
            "--json"
        ]
    );
    assert!(
        call.argv
            .windows(2)
            .any(|w| w == ["--sandbox", "read-only"])
    );
    assert!(
        call.argv
            .windows(2)
            .any(|w| w == ["--model", "gpt-5.4-mini"])
    );
    assert_eq!(call.argv.last().map(String::as_str), Some("-"));
    let record = s
        .log
        .since(log_from - 1)
        .into_iter()
        .find(|r| r.program == "codex")
        .unwrap();
    assert!(!record.stderr.contains("secret-ish"), "{}", record.stderr);
    assert!(record.stderr.contains("model: fake"));
}

#[tokio::test]
async fn failures_are_told_apart() {
    let s = setup(&["claude"]).await;
    s.repo.write("README.md", "# changed\n");
    let auto = settings(AiToolChoice::Auto);
    let task = || s.commit_task(&["README.md"]);
    let cases: [(i32, &str, &str, AiFailureKind, &str); 5] = [
        (
            1,
            "Not logged in · Please run /login\n",
            "",
            AiFailureKind::SignedOut,
            "Claude Code is not signed in. Run claude in a terminal to sign in, then try again.",
        ),
        (
            1,
            "You've hit your limit · resets 3pm\n",
            "",
            AiFailureKind::Limited,
            "Claude Code has reached a limit: You've hit your limit · resets 3pm.",
        ),
        (
            1,
            "There's an issue with the selected model (haiku). It may not exist or you may not have access to it.\n",
            "",
            AiFailureKind::BadModel,
            "Claude Code does not know the model \u{201c}haiku\u{201d}, or your account cannot use it. Choose another in Settings \u{2192} AI.",
        ),
        (
            2,
            "",
            "panic: something odd\n",
            AiFailureKind::Failed,
            "Claude Code failed: panic: something odd.",
        ),
        (
            0,
            "  \n",
            "",
            AiFailureKind::Empty,
            "Claude Code returned an empty message.",
        ),
    ];
    for (code, out, err, kind, message) in cases {
        s.tools.reply("claude", code, out, err);
        let got = s.generate(task(), &auto).await;
        assert_eq!(failure(&got), (kind, message.to_owned()), "{out}{err}");
    }
    // A failure's detail is the tool's own words.
    s.tools.reply("claude", 3, "", "first\nsecond\n");
    let AiOutcome::Failed { detail, .. } = s.generate(task(), &auto).await else {
        panic!()
    };
    assert_eq!(detail, "first\nfirst\nsecond");
}

#[tokio::test]
async fn nothing_to_describe() {
    let s = setup(&["claude"]).await;
    let auto = settings(AiToolChoice::Auto);
    let out = s.generate(s.commit_task(&[]), &auto).await;
    assert_eq!(failure(&out).0, AiFailureKind::Nothing);
    // Ticked, but unchanged since (committed in a terminal).
    let out = s.generate(s.commit_task(&["README.md"]), &auto).await;
    assert_eq!(failure(&out).0, AiFailureKind::Nothing);
    assert!(s.tools.calls("claude").is_empty(), "the tool never ran");
}

#[tokio::test]
async fn a_huge_diff_is_cut_to_the_budget() {
    let s = setup(&["claude"]).await;
    s.repo
        .write("big.txt", "a line that repeats\n".repeat(60_000));
    s.repo.write("small.txt", "the change that matters\n");
    s.tools.reply("claude", 0, "Add files\n", "");
    let out = s
        .generate(
            s.commit_task(&["big.txt", "small.txt"]),
            &settings(AiToolChoice::Auto),
        )
        .await;
    assert!(matches!(out, AiOutcome::Done { .. }), "{out:?}");
    let stdin = &s.tools.calls("claude")[0].stdin;
    assert!(stdin.len() < 260_000, "{}", stdin.len());
    assert!(stdin.contains("+the change that matters"));
    assert!(stdin.contains("truncated"));
}

#[tokio::test]
async fn cancel_ends_the_tool() {
    let s = setup(&["claude"]).await;
    s.repo.write("README.md", "# changed\n");
    s.tools.reply("claude", 0, "Too late\n", "");
    s.tools.set("claude", "reply.sleep", "30000");
    let m = s.m.clone();
    let (id, task) = (s.id, s.commit_task(&["README.md"]));
    let running = tokio::spawn(async move {
        m.ai_generate(id, "req-c", task, &settings(AiToolChoice::Auto))
            .await
    });
    s.tools.wait_started("claude", 1).await;
    let started = std::time::Instant::now();
    assert!(s.m.ai_cancel("req-c"));
    let out = tokio::time::timeout(Duration::from_secs(10), running)
        .await
        .expect("ends at once")
        .unwrap()
        .unwrap();
    assert_eq!(failure(&out).0, AiFailureKind::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(5));
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        !s.tools.calls("claude")[0].done,
        "the tool was ended, not left to answer"
    );
}

#[tokio::test]
async fn a_cancel_that_comes_first_wins() {
    let s = setup(&["claude"]).await;
    s.repo.write("README.md", "# changed\n");
    assert!(!s.m.ai_cancel("req-1"));
    let out = s
        .generate(s.commit_task(&["README.md"]), &settings(AiToolChoice::Auto))
        .await;
    assert_eq!(failure(&out).0, AiFailureKind::Cancelled);
    assert!(s.tools.calls("claude").is_empty());
}

#[tokio::test]
async fn a_tool_that_does_not_answer_is_given_up_on() {
    let s = setup(&["claude"]).await;
    s.repo.write("README.md", "# changed\n");
    s.tools.set("claude", "reply.sleep", "30000");
    s.m.set_ai_timeout(Duration::from_millis(800));
    let out = s
        .generate(s.commit_task(&["README.md"]), &settings(AiToolChoice::Auto))
        .await;
    assert_eq!(
        failure(&out),
        (
            AiFailureKind::TimedOut,
            "Claude Code did not answer within 1 s.".into()
        )
    );
}

/// A pull request's title and description from the branch's commits and changes since its
/// base; the base stays the user's.
#[tokio::test]
async fn pull_request_details_are_written_from_the_branch() {
    let s = setup(&["claude"]).await;
    s.repo.git(&["checkout", "-q", "-b", "feature/login"]);
    s.repo.write("login.rs", "fn login() {}\n");
    s.repo.commit_all("Add login");
    s.repo.write("login.rs", "fn login() { check() }\n");
    s.repo.commit_all("Check the password");
    s.tools.reply(
        "claude",
        0,
        "{\"base\":\"develop\",\"title\":\"Add login.\",\"body\":\"## Problem\\nNo login.\\n\\n## Solution\\nAdd it.\",\"draft\":true}",
        "",
    );
    // The repository's first branch is the base (main or master, as git made it).
    let heads = s
        .repo
        .git(&["for-each-ref", "--format=%(refname:short)", "refs/heads/"]);
    let main = heads
        .lines()
        .find(|b| *b != "feature/login")
        .unwrap()
        .to_owned();
    let task = AiTask::Pr {
        branch: "feature/login".into(),
        base: main.clone(),
        base_remote: Some("origin".into()),
        title: "Login".into(),
        body: String::new(),
        draft: false,
    };
    let out = s.generate(task, &settings(AiToolChoice::Auto)).await;
    let AiOutcome::Done {
        title, body, draft, ..
    } = out
    else {
        panic!("{out:?}")
    };
    assert_eq!(title.as_deref(), Some("Add login"));
    assert_eq!(
        body.as_deref(),
        Some("## Problem\nNo login.\n\n## Solution\nAdd it.")
    );
    assert_eq!(draft, Some(true));
    let stdin = &s.tools.calls("claude")[0].stdin;
    assert!(stdin.starts_with("You are generating pull request details."));
    assert!(stdin.contains("Head branch: feature/login"));
    assert!(stdin.contains(&format!("Current base: {main}")));
    assert!(stdin.contains("Current title: Login"));
    assert!(
        stdin.contains("- Check the password\n- Add login"),
        "{stdin}"
    );
    assert!(stdin.contains("A\tlogin.rs"), "{stdin}");
    assert!(stdin.contains("+fn login() { check() }"));

    // An answer that is not the JSON asked for.
    s.tools
        .reply("claude", 0, "Sure! Here is a title: Add login", "");
    let again = AiTask::Pr {
        branch: "feature/login".into(),
        base: main.clone(),
        base_remote: None,
        title: String::new(),
        body: String::new(),
        draft: false,
    };
    let out = s.generate(again, &settings(AiToolChoice::Auto)).await;
    assert_eq!(failure(&out).0, AiFailureKind::Empty);

    // A branch with nothing ahead of its base.
    let level = AiTask::Pr {
        branch: main.clone(),
        base: main.clone(),
        base_remote: None,
        title: String::new(),
        body: String::new(),
        draft: false,
    };
    let out = s.generate(level, &settings(AiToolChoice::Auto)).await;
    assert_eq!(failure(&out).0, AiFailureKind::Nothing);
}

#[tokio::test]
async fn a_program_inside_the_repository_is_never_run() {
    let s = setup(&[]).await;
    // The tool PATH is the repository's own folder, holding a "claude".
    fs::copy(
        FAKE,
        s.repo
            .path()
            .join(format!("claude{}", std::env::consts::EXE_SUFFIX)),
    )
    .unwrap();
    s.m.set_tool_path(Some(s.repo.path().as_os_str().to_owned()));
    s.repo.write("README.md", "# changed\n");
    let out = s
        .generate(s.commit_task(&["README.md"]), &settings(AiToolChoice::Auto))
        .await;
    assert_eq!(failure(&out).0, AiFailureKind::NoTool);
}

/// A binary file is named, never sent, even when the repository's attributes make git print
/// it as text (`diff` set): for a commit and for a pull request.
#[tokio::test]
async fn binary_contents_are_never_sent_whatever_the_attributes_say() {
    let s = setup(&["claude"]).await;
    s.repo.write(".gitattributes", "*.bin diff\n");
    s.repo
        .write("key.bin", b"SECRET-OLD\0\x01\x02\n".as_slice());
    s.repo.commit_all("Add the key");
    s.repo
        .write("key.bin", b"SECRET-NEW\0\x01\x02\n".as_slice());
    s.tools.reply("claude", 0, "Rotate the key\n", "");
    let out = s
        .generate(s.commit_task(&["key.bin"]), &settings(AiToolChoice::Auto))
        .await;
    assert!(matches!(out, AiOutcome::Done { .. }), "{out:?}");
    let stdin = &s.tools.calls("claude")[0].stdin;
    assert!(!stdin.contains("SECRET"), "{stdin}");
    assert!(!stdin.contains('\0'));
    assert!(
        stdin.contains("diff --git a/key.bin b/key.bin\nBinary file: its contents are not sent.")
    );

    // A branch whose commit changes it.
    s.repo.git(&["checkout", "-q", "-b", "rotate"]);
    s.repo.commit_all("Rotate the key");
    let heads = s
        .repo
        .git(&["for-each-ref", "--format=%(refname:short)", "refs/heads/"]);
    let main = heads.lines().find(|b| *b != "rotate").unwrap().to_owned();
    s.tools.reply(
        "claude",
        0,
        "{\"title\":\"Rotate the key\",\"body\":\"x\"}",
        "",
    );
    let pr = AiTask::Pr {
        branch: "rotate".into(),
        base: main,
        base_remote: None,
        title: String::new(),
        body: String::new(),
        draft: false,
    };
    let out = s.generate(pr, &settings(AiToolChoice::Auto)).await;
    assert!(matches!(out, AiOutcome::Done { .. }), "{out:?}");
    let stdin = &s.tools.calls("claude")[1].stdin;
    assert!(!stdin.contains("SECRET"), "{stdin}");
    assert!(stdin.contains("Binary file: its contents are not sent."));
}

/// No clean (or smudge) filter of the repository's runs while gg reads what a commit would
/// record: every filter driver configured is turned off for those reads, and the diff is
/// still the file's own.
#[cfg(unix)]
#[tokio::test]
async fn no_filter_runs_while_gathering() {
    let s = setup(&["claude"]).await;
    // Each run of the filter notes the command line of the git that ran it.
    let marks = s.repo.path().join("..").join("filter-ran");
    s.repo.write(".gitattributes", "*.txt filter=mark\n");
    s.repo.write("a.txt", "one\n");
    s.repo.commit_all("Add a");
    let command = format!("sh -c 'ps -o args= -p $PPID >> {}; cat'", marks.display());
    s.repo.git(&["config", "filter.mark.clean", &command]);
    s.repo.git(&["config", "filter.mark.smudge", &command]);
    s.repo.git(&["config", "filter.mark.required", "true"]);
    s.repo.write("a.txt", "two\n");
    s.tools.reply("claude", 0, "Change a\n", "");
    let out = s
        .generate(s.commit_task(&["a.txt"]), &settings(AiToolChoice::Auto))
        .await;
    assert!(matches!(out, AiOutcome::Done { .. }), "{out:?}");
    // gg's own status reads (its panel's) may run it, as git status in a terminal does; the
    // generation's reads never: each of those turns the filters off on its command line, and
    // its diff is the only `--literal-pathspecs` one.
    let ran = fs::read_to_string(&marks).unwrap_or_default();
    for line in ran.lines() {
        assert!(
            !line.contains("--literal-pathspecs") && !line.contains("filter.mark"),
            "a filter ran for the generation's read: {line}"
        );
    }
    let generation: Vec<_> = s
        .log
        .since(0)
        .into_iter()
        .filter(|r| r.op_title.is_some() && r.program == "git")
        .filter(|r| r.args.iter().any(|a| a == "status" || a == "diff"))
        .collect();
    assert_eq!(generation.len(), 2, "{generation:?}");
    for r in &generation {
        assert!(
            r.args.iter().any(|a| a == "filter.mark.clean="),
            "{:?}",
            r.args
        );
    }
    let stdin = &s.tools.calls("claude")[0].stdin;
    assert!(stdin.contains("-one\n+two"), "{stdin}");
}

/// A tool found inside an open repository (its main folder or a linked worktree outside it)
/// is never run: not for its version, not to generate.
#[tokio::test]
async fn a_tool_inside_any_open_repository_is_never_run_even_for_its_version() {
    let log = CommandLog::new();
    let runner = GitRunner::detect(log.clone()).await.expect("git");
    let m = RepoManager::new(runner, Arc::new(RecordingSink::default()));
    let repo = TestRepo::new_in_subdir("main");
    repo.write("README.md", "# demo\n");
    repo.commit_all("Start");
    let linked = repo.add_worktree("side");
    assert!(
        !linked.starts_with(repo.path()),
        "the worktree is outside the main folder"
    );
    m.open(repo.path()).await.expect("open");
    for folder in [repo.path().to_owned(), linked.clone()] {
        let bin = folder.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let exe = bin.join(format!("claude{}", std::env::consts::EXE_SUFFIX));
        fs::copy(FAKE, &exe).unwrap();
        fs::create_dir_all(bin.join("claude.fake")).unwrap();
        fs::write(bin.join("claude.fake").join("version"), "9.9.9 (planted)\n").unwrap();
        m.set_tool_path(Some(bin.as_os_str().to_owned()));
        let tools = m.ai_tools(&settings(AiToolChoice::Auto), true, false).await;
        assert!(!tools.tools[0].installed, "{}", folder.display());
        assert_eq!(tools.chosen, None);
        assert!(
            !log.since(0).iter().any(|r| r.program == "claude"),
            "never run, not even --version"
        );
    }
}
