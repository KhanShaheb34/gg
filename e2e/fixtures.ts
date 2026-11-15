// Playwright fixtures for end-to-end tests against the real core.
//
// Each test gets its own gg-devserver (fresh config dir, free port) and its own temp folder
// of fixture repositories made with real git. The page reaches that devserver through
// `window.__GG_DEV_URL__`, set before any app script runs.

import { type ChildProcess, execFileSync, spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test as base, type Page } from "@playwright/test";
import type { OpOutcome, OpRequest } from "../src/lib/types";

const ROOT = path.resolve(import.meta.dirname, "..");
const DEVSERVER = path.join(ROOT, "target", "debug", "gg-devserver");
/** The vite dev server the page is loaded from (playwright.config.ts `baseURL`). */
const VITE_ORIGIN = "http://127.0.0.1:1420";

/** Runs git with `args` in the repository; returns its stdout. */
export type Git = (...args: string[]) => string;
/** Writes `content` to `file`, relative to the repository. */
export type Write = (file: string, content: string) => void;
/** A fixture repository's script: gets `git` and `write` (both run in it) and its folder. */
export type RepoScript = (git: Git, write: Write, dir: string) => void;

/** An empty file used as git's global config, so the user's own config never applies. */
const EMPTY_CONFIG = (() => {
  const dir = mkdtempSync(path.join(tmpdir(), "gg-e2e-gitconfig-"));
  const file = path.join(dir, "config");
  writeFileSync(file, "");
  process.on("exit", () => rmSync(dir, { recursive: true, force: true }));
  return file;
})();

/** The environment every fixture git (and the devserver) runs with: no user or system config. */
function isolatedEnv(): NodeJS.ProcessEnv {
  return {
    ...process.env,
    GIT_CONFIG_GLOBAL: EMPTY_CONFIG,
    GIT_CONFIG_NOSYSTEM: "1",
    GIT_TERMINAL_PROMPT: "0",
    LC_ALL: "C",
  };
}

/** Commit times start here and advance one hour per git call, so histories are reproducible. */
const EPOCH = Date.UTC(2026, 0, 5, 9, 0, 0) / 1000;

/** Each repository's clock: how many git calls (hours since EPOCH) it has had. */
const clocks = new Map<string, number>();

/** Moves `dir`'s clock on to `tick` hours after EPOCH, if it is not there yet. */
function advanceClock(dir: string, tick: number): void {
  clocks.set(dir, Math.max(clocks.get(dir) ?? 0, tick));
}

/** Who makes the commits of a fixture git: Ada, unless said otherwise. */
type Identity = { name: string; email: string };
const ADA: Identity = { name: "Ada Lovelace", email: "ada@example.com" };
const GRACE: Identity = { name: "Grace Hopper", email: "grace@example.com" };

function gitIn(dir: string, who: Identity = ADA): Git {
  return (...args) => {
    const tick = clocks.get(dir) ?? 0;
    clocks.set(dir, tick + 1);
    const when = `${EPOCH + tick * 3600} +0000`;
    return execFileSync("git", args, {
      cwd: dir,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
      env: {
        ...isolatedEnv(),
        GIT_AUTHOR_NAME: who.name,
        GIT_AUTHOR_EMAIL: who.email,
        GIT_COMMITTER_NAME: who.name,
        GIT_COMMITTER_EMAIL: who.email,
        GIT_AUTHOR_DATE: when,
        GIT_COMMITTER_DATE: when,
      },
    });
  };
}

/**
 * Creates the folder `name` in `parent` (a new temp folder by default), runs `script` in it
 * and returns the folder's path. The script gets `git` (run in the folder), `write` and the
 * folder.
 */
export function makeRepo(
  name: string,
  script: RepoScript,
  parent: string = mkdtempSync(path.join(tmpdir(), "gg-e2e-")),
): string {
  const dir = path.join(parent, name);
  mkdirSync(dir, { recursive: true });
  const write: Write = (file, content) => {
    const target = path.join(dir, file);
    mkdirSync(path.dirname(target), { recursive: true });
    writeFileSync(target, content);
  };
  script(gitIn(dir), write, dir);
  return dir;
}

/** The second clone beside fixture repository `dir` that pushes to its remote (`routine`). */
export function otherClone(dir: string): string {
  return path.join(path.dirname(dir), `${path.basename(dir)}-other`);
}

/** Subject of commit `i` (1 = oldest) of the `big` fixture. */
export const bigSubject = (i: number) => `Big commit ${i}`;
/** Number of commits in the `big` fixture. */
export const BIG_COMMITS = 3000;

/** A `git fast-import` stream of `count` linear commits on `main`, one hour apart. */
function linearHistory(count: number): string {
  const parts: string[] = [];
  const data = (text: string) => `data ${Buffer.byteLength(text)}\n${text}\n`;
  for (let i = 1; i <= count; i++) {
    const who = `Ada Lovelace <ada@example.com> ${EPOCH + i * 3600} +0000`;
    parts.push(
      "commit refs/heads/main\n",
      `mark :${i}\n`,
      `author ${who}\n`,
      `committer ${who}\n`,
      data(bigSubject(i)),
      i > 1 ? `from :${i - 1}\n` : "",
      "M 644 inline log.txt\n",
      data(`line ${i}`),
    );
  }
  return parts.join("");
}

/** Named fixture repositories. */
const REPOS: Record<string, RepoScript> = {
  /**
   * 30 commits: `main` with tag `v1`, `feature/login` merged into it, `fix/typo` unmerged;
   * a bare `origin` (next to the repository) whose `main` is 2 commits behind.
   */
  small(git, write) {
    git("init", "-q", "-b", "main");
    const commit = (file: string, line: string, subject: string) => {
      write(file, `${line}\n`);
      git("add", "-A");
      git("commit", "-q", "-m", subject);
    };
    for (let i = 1; i <= 12; i++) commit(`src/part${i}.txt`, `part ${i}`, `Add part ${i}`);
    git("tag", "v1", "HEAD~4");
    git("switch", "-q", "-c", "feature/login");
    for (let i = 1; i <= 6; i++) commit(`login/step${i}.txt`, `step ${i}`, `Login step ${i}`);
    git("switch", "-q", "-c", "fix/typo", "main");
    for (let i = 1; i <= 4; i++) commit(`docs/typo${i}.md`, `fixed ${i}`, `Fix typo ${i}`);
    git("switch", "-q", "main");
    for (let i = 13; i <= 17; i++) commit(`src/part${i}.txt`, `part ${i}`, `Add part ${i}`);
    git("merge", "-q", "--no-ff", "-m", "Merge branch 'feature/login'", "feature/login");
    git("init", "-q", "--bare", "-b", "main", "../small-origin.git");
    git("remote", "add", "origin", "../small-origin.git");
    git("push", "-q", "-u", "origin", "main", "fix/typo");
    for (let i = 18; i <= 19; i++) commit(`src/part${i}.txt`, `part ${i}`, `Add part ${i}`);
  },
  /**
   * For the details panel, oldest first: "Initial import"; "Rework peaches routes", which adds
   * (a text and a binary file), modifies and renames files and has a message body; "Side
   * feature" on `side` and "Main work" on `main`, merged by "Merge branch 'side'"; then
   * "Generate the module scaffolding", 64 new files and a long message.
   */
  details(git, write) {
    git("init", "-q", "-b", "main");
    write("app/peaches/a.py", "def a():\n    return 1\n");
    write("app/routes/r.py", "ROUTES = []\n");
    write("docs/A.md", "# A\n");
    write("notes/old-name.txt", Array.from({ length: 20 }, (_, i) => `note ${i + 1}\n`).join(""));
    git("add", "-A");
    git("commit", "-q", "-m", "Initial import");

    write("app/peaches/b.py", "def b():\n    return 2\n");
    write("app/routes/r.py", "ROUTES = ['/peaches']\nPREFIX = '/api'\n");
    write("assets/logo.bin", "\u0000\u0001\u0002binary\u0000data\u0000");
    git("mv", "notes/old-name.txt", "docs/new-name.txt");
    git("add", "-A");
    git(
      "commit",
      "-q",
      "-m",
      "Rework peaches routes\n\nMove the old notes into docs and add b.py.\n    Indented line keeps   its spacing.",
    );

    git("switch", "-q", "-c", "side");
    write("side/feature.txt", "feature\n");
    write("side/more.txt", "more\n");
    git("add", "-A");
    git("commit", "-q", "-m", "Side feature");
    git("switch", "-q", "main");
    write("main.txt", "main\n");
    git("add", "-A");
    git("commit", "-q", "-m", "Main work");
    git("merge", "-q", "--no-ff", "-m", "Merge branch 'side'", "side");

    for (let i = 1; i <= 64; i++) {
      const lines = Array.from({ length: (i % 7) + 1 }, (_, k) => `export const v${k} = ${i * k};\n`).join("");
      write(`packages/module-${String((i % 8) + 1).padStart(2, "0")}/src/file-${i}.ts`, lines);
    }
    const body = Array.from(
      { length: 24 },
      (_, i) => `- Step ${i + 1}: wire module ${(i % 8) + 1} into the build and check its exports.`,
    ).join("\n");
    git("add", "-A");
    git(
      "commit",
      "-q",
      "-m",
      `Generate the module scaffolding\n\nEvery package gets a src folder with typed exports, so the\nbuild can check them before bundling.\n\n${body}\n\nSee https://example.com/docs/modules for the layout.`,
    );
  },
  /**
   * For cherry-picks, oldest first: "Initial commit" (greeting.txt "hello"); on `topic`, "Add
   * topic notes" (topic.txt) and "Greet from topic" (greeting.txt changed); on `integration`,
   * "Prepare integration" and "Merge branch 'feature' into integration", which brings in
   * "Build the feature" (feature.txt) from `feature`; on `main` (checked out), "Greet from
   * main", which changes greeting.txt otherwise: picking "Greet from topic" conflicts. The
   * repository has an identity for the commits the app makes.
   */
  picks(git, write) {
    git("init", "-q", "-b", "main");
    git("config", "user.name", "Ada Lovelace");
    git("config", "user.email", "ada@example.com");
    const commit = (files: Record<string, string>, subject: string) => {
      for (const [file, content] of Object.entries(files)) write(file, content);
      git("add", "-A");
      git("commit", "-q", "-m", subject);
    };
    commit({ "greeting.txt": "hello\n", "README.md": "picks\n" }, "Initial commit");
    git("switch", "-q", "-c", "topic");
    commit({ "topic.txt": "topic notes\n" }, "Add topic notes");
    commit({ "greeting.txt": "hello from topic\n" }, "Greet from topic");
    git("switch", "-q", "-c", "integration", "main");
    commit({ "integration.txt": "base\n" }, "Prepare integration");
    git("switch", "-q", "-c", "feature", "main");
    commit({ "feature.txt": "feature\n" }, "Build the feature");
    git("switch", "-q", "integration");
    git("merge", "-q", "--no-ff", "-m", "Merge branch 'feature' into integration", "feature");
    git("switch", "-q", "main");
    commit({ "greeting.txt": "hello from main\n" }, "Greet from main");
  },
  /**
   * Conflicts (spec §5.6). 40 commits of history first ("Earlier work 1".."40", so the graph
   * scrolls), then "Base" with `app.txt`, `style.css` and `src/module-01.ts`..`12.ts`.
   * From Base: `feature/x` changes `app.txt` and `style.css` ("Rework the feature");
   * `wide` changes every module ("Rename across modules"); `topic` has five commits, "Topic
   * one".."Topic five", of which two and four change `app.txt` and `style.css`. `main` then
   * changes all of those files ("Rework on main"): merging `feature/x` into main conflicts in
   * 2 files, `wide` in 12, and rebasing `topic` onto main stops at steps 2 and 4. `main` is
   * checked out; the repository has an identity for the commits git makes.
   */
  conflicts(git, write) {
    git("init", "-q", "-b", "main");
    git("config", "user.name", "Ada Lovelace");
    git("config", "user.email", "ada@example.com");
    for (let i = 1; i <= 40; i++) {
      write("history.txt", `step ${i}\n`);
      git("add", "-A");
      git("commit", "-q", "-m", `Earlier work ${i}`);
    }
    const modules = Array.from({ length: 12 }, (_, i) => `src/module-${String(i + 1).padStart(2, "0")}.ts`);
    const commit = (files: Record<string, string>, subject: string) => {
      for (const [file, content] of Object.entries(files)) write(file, content);
      git("add", "-A");
      git("commit", "-q", "-m", subject);
    };
    commit(
      { "app.txt": "app base\n", "style.css": "body {}\n", ...Object.fromEntries(modules.map((m) => [m, "base\n"])) },
      "Base",
    );
    git("switch", "-q", "-c", "feature/x");
    commit({ "app.txt": "app from feature\n", "style.css": "body { color: teal; }\n" }, "Rework the feature");
    git("switch", "-q", "-c", "wide", "main");
    commit(Object.fromEntries(modules.map((m) => [m, "renamed on wide\n"])), "Rename across modules");
    git("switch", "-q", "-c", "topic", "main");
    commit({ "topic-1.txt": "one\n" }, "Topic one");
    commit({ "app.txt": "app from topic\n" }, "Topic two");
    commit({ "topic-3.txt": "three\n" }, "Topic three");
    commit({ "style.css": "body { color: plum; }\n" }, "Topic four");
    commit({ "topic-5.txt": "five\n" }, "Topic five");
    git("switch", "-q", "main");
    commit(
      {
        "app.txt": "app from main\n",
        "style.css": "body { color: navy; }\n",
        ...Object.fromEntries(modules.map((m) => [m, "changed on main\n"])),
      },
      "Rework on main",
    );
  },
  /**
   * The staging routine (spec §5.3), with a bare `origin` beside it (`routine-origin.git`)
   * and a second clone of that (`routine-other`, see `otherClone`) that pushes to it, to move
   * the remote. Oldest first: "Initial commit" on `main`; "Prepare staging" on `staging`;
   * "Add login form" and "Validate login" on `feature/login` (from `staging`); all three
   * pushed with upstreams. Then the other clone pushes "Hotfix one" and "Hotfix two" to
   * `staging`, and this repository fetches: `staging` is 2 behind `origin/staging`, not
   * ahead. `main` is checked out. The other clone's commits are Grace Hopper's; this one's,
   * and those git makes here, Ada Lovelace's.
   */
  routine(git, write, dir) {
    git("init", "-q", "-b", "main");
    git("config", "user.name", "Ada Lovelace");
    git("config", "user.email", "ada@example.com");
    const commit = (file: string, subject: string) => {
      write(file, `${subject}\n`);
      git("add", "-A");
      git("commit", "-q", "-m", subject);
    };
    commit("README.md", "Initial commit");
    git("switch", "-q", "-c", "staging");
    commit("staging.txt", "Prepare staging");
    git("switch", "-q", "-c", "feature/login");
    commit("login/form.txt", "Add login form");
    commit("login/validate.txt", "Validate login");
    git("switch", "-q", "main");
    git("init", "-q", "--bare", "-b", "main", "../routine-origin.git");
    git("remote", "add", "origin", "../routine-origin.git");
    git("push", "-q", "-u", "origin", "main", "staging", "feature/login");

    const other = otherClone(dir);
    git("clone", "-q", "../routine-origin.git", other);
    // Someone else's commits.
    const there = gitIn(other, GRACE);
    advanceClock(other, clocks.get(dir) ?? 0);
    there("switch", "-q", "staging");
    for (const subject of ["Hotfix one", "Hotfix two"]) {
      writeFileSync(path.join(other, `${subject.replace(" ", "-")}.txt`), `${subject}\n`);
      there("add", "-A");
      there("commit", "-q", "-m", subject);
    }
    there("push", "-q", "origin", "staging");
    advanceClock(dir, clocks.get(other) ?? 0);
    git("fetch", "-q", "origin");
  },
  /** A shallow clone (depth 2) of three commits, "One" to "Three": "Two" is where its history is cut. */
  shallow(git, _write, dir) {
    const source = makeRepo(
      "shallow-source",
      (src, write) => {
        src("init", "-q", "-b", "main");
        for (const subject of ["One", "Two", "Three"]) {
          write("f.txt", `${subject}\n`);
          src("add", "-A");
          src("commit", "-q", "-m", subject);
        }
      },
      path.dirname(dir),
    );
    git("init", "-q", "-b", "main");
    git("fetch", "-q", "--depth", "2", `file://${source}`, "main");
    git("reset", "-q", "--hard", "FETCH_HEAD");
  },
  /** A repository without commits. */
  empty(git) {
    git("init", "-q", "-b", "main");
  },
  /** 3,000 linear commits on `main` ("Big commit 1" is the oldest), made with fast-import. */
  big(git, _write, dir) {
    git("init", "-q", "-b", "main");
    execFileSync("git", ["fast-import", "--quiet"], {
      cwd: dir,
      input: linearHistory(BIG_COMMITS),
      stdio: ["pipe", "ignore", "pipe"],
      env: isolatedEnv(),
    });
    advanceClock(dir, BIG_COMMITS + 1); // later commits come after the imported ones
    git("reset", "-q", "--hard");
  },
  /**
   * 600 linear commits ("Big commit 1" the oldest) with labels: a tag on every 7th commit, a
   * branch on every 11th, and five labels on commit 300.
   */
  labelled(git, _write, dir) {
    git("init", "-q", "-b", "main");
    let refs = "";
    for (let i = 1; i <= 600; i++) {
      if (i % 7 === 0) refs += `reset refs/tags/t${i}\nfrom :${i}\n\n`;
      if (i % 11 === 0) refs += `reset refs/heads/b${i}\nfrom :${i}\n\n`;
    }
    for (const name of ["x1", "x2", "x3", "x4", "x5"]) refs += `reset refs/tags/${name}\nfrom :300\n\n`;
    execFileSync("git", ["fast-import", "--quiet"], {
      cwd: dir,
      input: linearHistory(600) + refs,
      stdio: ["pipe", "ignore", "pipe"],
      env: isolatedEnv(),
    });
    advanceClock(dir, 601);
    git("reset", "-q", "--hard");
  },
  /**
   * 300 linear commits on `main` ("Big commit 1" the oldest) and two stashes, both newer than
   * every commit: `stash@{1}` "On main: Keep the parser draft" on `main` with an untracked
   * file, and `stash@{0}` "WIP on (no branch): …" on commit 150, which no branch has after
   * it (its base is on `main`, so only the stash's own row is extra).
   */
  stashes(git, write, dir) {
    git("init", "-q", "-b", "main");
    execFileSync("git", ["fast-import", "--quiet"], {
      cwd: dir,
      input: linearHistory(300),
      stdio: ["pipe", "ignore", "pipe"],
      env: isolatedEnv(),
    });
    advanceClock(dir, 301);
    git("reset", "-q", "--hard");
    write("log.txt", "draft\n");
    write("draft.txt", "untracked draft\n");
    git("stash", "push", "-q", "--include-untracked", "--message=Keep the parser draft");
    git("checkout", "-q", "--detach", "main~150");
    write("log.txt", "older idea\n");
    git("stash", "push", "-q");
    git("checkout", "-q", "main");
  },
  /**
   * 48 commits on `main`, `develop`, `feature/search`, `feature/ui`, `release/1.0` and a merged
   * `hotfix/crash` that now exists only on the remote, with merges between them; tags `v0.1`,
   * `v0.2`, `v1.0`; a bare `origin` whose `develop` is at the local one, and whose `main` and
   * `feature/ui` are behind the local ones.
   */
  branchy(git, write) {
    git("init", "-q", "-b", "main");
    let n = 0;
    const commit = (subject: string) => {
      write(`files/f${++n}.txt`, `${subject}\n`);
      git("add", "-A");
      git("commit", "-q", "-m", subject);
    };
    const merge = (branch: string) => git("merge", "-q", "--no-ff", "-m", `Merge branch '${branch}'`, branch);
    for (const s of ["Initial layout", "Add README", "Set up CI", "Add config loader", "Parse arguments", "Add logging"])
      commit(s);
    git("tag", "v0.1", "HEAD~2");
    git("switch", "-q", "-c", "develop");
    for (const s of ["Introduce plugin API", "Plugin registry", "Load plugins lazily", "Document plugin hooks"]) commit(s);
    git("switch", "-q", "-c", "feature/search");
    for (const s of ["Search index skeleton", "Tokenize subjects", "Rank results"]) commit(s);
    git("switch", "-q", "main");
    for (const s of ["Bump dependencies", "Fix flaky test"]) commit(s);
    git("switch", "-q", "-c", "hotfix/crash");
    for (const s of ["Guard against empty config", "Add crash regression test"]) commit(s);
    git("switch", "-q", "feature/search");
    for (const s of ["Highlight matches", "Search keyboard shortcuts"]) commit(s);
    git("switch", "-q", "main");
    merge("hotfix/crash");
    git("tag", "v0.2");
    git("switch", "-q", "develop");
    commit("Plugin settings page");
    git("switch", "-q", "-c", "feature/ui");
    for (const s of ["New sidebar", "Theme tokens", "Dark theme"]) commit(s);
    git("switch", "-q", "develop");
    merge("feature/search");
    git("switch", "-q", "feature/ui");
    for (const s of ["Toolbar icons", "Compact rows"]) commit(s);
    git("switch", "-q", "develop");
    for (const s of ["Cache plugin manifests", "Retry failed plugin loads"]) commit(s);
    git("switch", "-q", "main");
    commit("Update licence year");
    git("switch", "-q", "develop");
    merge("main");
    git("switch", "-q", "feature/ui");
    for (const s of ["Animate panel transitions", "Keyboard focus rings", "Accessible labels"]) commit(s);
    git("switch", "-q", "develop");
    merge("feature/ui");
    git("switch", "-q", "-c", "release/1.0");
    for (const s of ["Release notes for 1.0", "Bump version to 1.0.0"]) commit(s);
    git("switch", "-q", "main");
    merge("release/1.0");
    git("tag", "v1.0");
    git("switch", "-q", "develop");
    merge("main");
    for (const s of ["Start 1.1 cycle", "Plugin sandboxing", "Sandbox file access"]) commit(s);
    git("switch", "-q", "feature/ui");
    for (const s of ["Resizable columns", "Column persistence"]) commit(s);
    git("switch", "-q", "main");
    commit("Hotfix: config path on Windows");
    git("init", "-q", "--bare", "-b", "main", "../branchy-origin.git");
    git("remote", "add", "origin", "../branchy-origin.git");
    git("push", "-q", "-u", "origin", "main", "develop", "feature/ui", "hotfix/crash");
    git("branch", "-q", "-D", "hotfix/crash");
    commit("Tidy imports");
    git("switch", "-q", "feature/ui");
    for (const s of ["Sticky header", "Row hover states"]) commit(s);
    git("switch", "-q", "main");
  },
};

/** A running gg-devserver. */
type Devserver = { url: string; process: ChildProcess };

/** Kills `child` and everything it started (a login shell, git): its whole process group. */
function killGroup(child: ChildProcess, signal: NodeJS.Signals = "SIGTERM"): void {
  if (child.pid === undefined) return;
  try {
    process.kill(-child.pid, signal);
  } catch {
    child.kill(signal); // the group is gone; the child may not be
  }
}

/** Where the devserver of config folder `configDir` records editor launches (it never starts one). */
function editorRecord(configDir: string): string {
  return path.join(path.dirname(configDir), "editor-launches.jsonl");
}

/**
 * Where the `gg` command may go in a test: `~/.local/bin` and `~/bin` of the test's home
 * folder, and `extra` more (`~/tools/bin1`…), made here; the `PATH` the devserver looks for it on (`--cli-path`). Never a folder
 * of the user's own `PATH`.
 */
export function cliFolders(home: string, extra = 0): string[] {
  const dirs = [path.join(home, ".local", "bin"), path.join(home, "bin")];
  for (let i = 1; i <= extra; i++) dirs.push(path.join(home, "tools", `bin${i}`));
  for (const dir of dirs) mkdirSync(dir, { recursive: true });
  return dirs;
}

/**
 * A stand-in for the login shell the devserver asks what `gg` is (`--cli-shell`): a POSIX sh
 * script with a bare `PATH` that runs `setup` (an alias, say) and then the command it is
 * given. The user's own shell and its files are never run.
 */
function fakeShell(home: string, setup: string): string {
  const file = path.join(home, "fake-shell");
  writeFileSync(file, `#!/bin/sh\nPATH=/usr/bin:/bin\n${setup}\neval "$2"\n`, { mode: 0o755 });
  return file;
}

/**
 * Starts a devserver whose home folder is `home` (so paths under it show as `~/…`); the
 * debug build unless `binary` names another. The debug build records editor launches instead
 * of starting an editor (see `GG.editorLaunches`). The `gg` command is looked for in
 * `cliFolders(home, extraCliFolders)` only, and the stand-in shell, set up with `shellSetup`, is what is asked
 * what `gg` is.
 */
export function startDevserver(
  configDir: string,
  home: string,
  binary = DEVSERVER,
  shellSetup = "",
  extraCliFolders = 0,
): Promise<Devserver> {
  const cliPath = cliFolders(home, extraCliFolders).join(path.delimiter);
  const shell = fakeShell(home, shellSetup);
  const child = spawn(
    binary,
    [
      "--port",
      "0",
      "--config-dir",
      configDir,
      "--allow-origin",
      VITE_ORIGIN,
      "--exit-on-stdin-eof",
      // The perf runs' release build (built on its own) need not know it.
      ...(binary === DEVSERVER ? ["--record-editor-launches", editorRecord(configDir)] : []),
      "--cli-path",
      cliPath,
      "--cli-shell",
      shell,
    ],
    // Its own process group, so stopping it also stops whatever it started. Its stdin stays
    // open while this process lives: if the runner dies (Ctrl-C), the pipe closes and the
    // server quits, though it is detached.
    { stdio: ["pipe", "pipe", "pipe"], env: { ...isolatedEnv(), HOME: home, USERPROFILE: home }, detached: true },
  );
  return new Promise((resolve, reject) => {
    let out = "";
    let err = "";
    const t0 = Date.now();
    const timer = setTimeout(() => {
      // What the server (and anything it started: login shell, git) is doing, for the report.
      let ps = "";
      try {
        ps = execFileSync("ps", ["-A", "-o", "pid,ppid,etime,stat,command"], { encoding: "utf8" })
          .split("\n")
          .filter((l) => l.includes(String(child.pid)) || /gg-devserver|zsh|xcrun|git /.test(l))
          .join("\n");
      } catch {}
      fail(new Error(`gg-devserver did not start within 20 s (pid ${child.pid}, out ${JSON.stringify(out)})\n${ps}`));
    }, 20_000);
    const detach = () => {
      child.stdout!.removeAllListeners("data");
      child.stderr!.removeAllListeners("data");
      child.removeListener("error", fail);
      child.removeListener("exit", exited);
    };
    const fail = (e: Error) => {
      clearTimeout(timer);
      detach();
      killGroup(child);
      reject(new Error(`${e.message}\n${err}`));
    };
    const exited = (code: number | null) => fail(new Error(`gg-devserver exited with ${code}`));
    // Start-up times, for looking into slow starts: GG_STARTLOG=<file> pnpm e2e
    if (process.env.GG_STARTLOG) {
      child.stdout!.once("data", () => {
        writeFileSync(process.env.GG_STARTLOG!, `${new Date().toISOString()} ${Date.now() - t0}\n`, { flag: "a" });
      });
    }
    child.stderr!.on("data", (d: Buffer) => (err += d.toString()));
    child.stdout!.on("data", (d: Buffer) => {
      out += d.toString();
      const match = /listening on (http:\/\/\S+)/.exec(out);
      if (match) {
        clearTimeout(timer);
        detach();
        // Its output is no longer read; it must not fill the pipes and block.
        child.stdout!.resume();
        child.stderr!.resume();
        resolve({ url: match[1], process: child });
      }
    });
    child.on("error", fail);
    child.on("exit", exited);
  });
}

/** Stops the devserver and what it started; SIGKILL if it has not gone after 5 s. */
export async function stopDevserver(server: Devserver): Promise<void> {
  const child = server.process;
  if (child.exitCode !== null || child.signalCode !== null) {
    killGroup(child); // anything it left behind
    return;
  }
  const exited = new Promise<void>((resolve) => child.once("exit", () => resolve()));
  killGroup(child);
  const timer = setTimeout(() => killGroup(child, "SIGKILL"), 5_000);
  await exited;
  clearTimeout(timer);
}

/** The close button of the tab named `name` (hidden from assistive technology; see TabBar). */
type Unheaded<R> = R extends { expectHead: unknown } ? Omit<R, "expectHead"> & Partial<Pick<R, "expectHead">> : R;
/**
 * A request as a test writes it: one on the current branch may leave out `expectHead`, which
 * `runOp` fills in with HEAD as the active tab shows it when the request is sent, as a menu
 * would (R39).
 */
export type Asked = Unheaded<OpRequest>;

/** Runs `request` through the app's own runOp (the module the page loaded), in the active tab. */
export function runOp(page: Page, request: Asked): Promise<OpOutcome> {
  return page.evaluate(async (req) => {
    const ops = await import("/src/lib/state/ops.svelte.ts");
    const { app } = await import("/src/lib/state/app.svelte.ts");
    const { findRepoView } = await import("/src/lib/state/repo.svelte.ts");
    const id = app.activeRepoId!;
    const onHead = ["reset", "merge", "rebase", "cherry_pick", "revert", "pull", "continue", "abort", "skip"];
    const full = onHead.includes(req.op) && !("expectHead" in req) ? { ...req, expectHead: findRepoView(id)!.snapshot.head } : req;
    return ops.runOp(id, full as OpRequest);
  }, request);
}

export function closeButton(page: Page, name: string) {
  return page.getByRole("tab", { name, exact: true }).locator("xpath=..").locator(".close");
}

export type GG = {
  page: Page;
  /** The test's home folder (`HOME` of its devserver); the `gg` command may go in its `.local/bin` and `bin`. */
  home: string;
  /** Path of fixture repository `name` (`small`, `empty`, `big`, `branchy`, `details`, `stashes`…), created on first use. */
  repoPath(name: string): string;
  /** Path of a plain (non-repository) folder `name`, created on first use. */
  folder(name: string): string;
  /** Runs git in fixture repository `name`; returns stdout. */
  git(name: string, ...args: string[]): string;
  /** Runs git in folder `dir` (a clone beside a fixture, say); returns stdout. */
  gitAt(dir: string, ...args: string[]): string;
  /** The editor launches asked for so far (program and arguments each), oldest first; none is started. */
  editorLaunches(): string[][];
};

/**
 * Builds the debug gg-devserver and starts it once, so that tests only ever start an
 * up-to-date binary that has run before. Called once per run, from e2e/global-setup.ts,
 * before any worker starts.
 */
export async function buildDevserver(): Promise<void> {
  execFileSync("cargo", ["build", "-q", "-p", "gg-devserver"], {
    cwd: ROOT,
    stdio: ["ignore", "ignore", "inherit"],
    env: { ...process.env, CARGO_INCREMENTAL: "0" },
  });
  const root = realpathSync(mkdtempSync(path.join(tmpdir(), "gg-e2e-warmup-")));
  try {
    await stopDevserver(await startDevserver(path.join(root, "config"), root));
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

export const test = base.extend<{ gg: GG; ggShellSetup: string; ggCliFolders: number }>({
  /** What the stand-in login shell defines before it is asked what `gg` is (`alias gg=…`). */
  ggShellSetup: ["", { option: true }],
  /** Folders the `gg` command may go in besides `~/.local/bin` and `~/bin`. */
  ggCliFolders: [0, { option: true }],
  gg: async ({ page, ggShellSetup, ggCliFolders }, use) => {
    // realpath: macOS's temp folder is behind a symlink, and the core reports real paths.
    const root = realpathSync(mkdtempSync(path.join(tmpdir(), "gg-e2e-")));
    let server: Devserver | null = null;
    try {
      server = await startDevserver(path.join(root, "config"), root, DEVSERVER, ggShellSetup, ggCliFolders);
      await page.addInitScript((url) => {
        (window as unknown as { __GG_DEV_URL__: string }).__GG_DEV_URL__ = url;
      }, server.url);

      const repos = path.join(root, "repos");
      const made = new Map<string, string>();
      const repoPath = (name: string) => {
        let dir = made.get(name);
        if (!dir) {
          const script = REPOS[name];
          if (!script) throw new Error(`no fixture repository named ${name}`);
          dir = makeRepo(name, script, repos);
          made.set(name, dir);
        }
        return dir;
      };
      const folder = (name: string) => {
        const dir = path.join(root, "folders", name);
        mkdirSync(dir, { recursive: true });
        return dir;
      };
      const gits = new Map<string, Git>();
      const git = (name: string, ...args: string[]) => {
        let run = gits.get(name);
        if (!run) gits.set(name, (run = gitIn(repoPath(name))));
        return run(...args);
      };

      const gitAt = (dir: string, ...args: string[]) => {
        let run = gits.get(dir);
        if (!run) gits.set(dir, (run = gitIn(dir)));
        return run(...args);
      };

      const editorLaunches = (): string[][] => {
        const file = editorRecord(path.join(root, "config"));
        if (!existsSync(file)) return [];
        return readFileSync(file, "utf8")
          .split("\n")
          .filter((l) => l !== "")
          .map((l) => JSON.parse(l) as string[]);
      };

      await use({ page, home: root, repoPath, folder, git, gitAt, editorLaunches });
    } finally {
      if (server) await stopDevserver(server);
      rmSync(root, { recursive: true, force: true });
    }
  },
});

export { expect } from "@playwright/test";
