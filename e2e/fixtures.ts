// Playwright fixtures for end-to-end tests against the real core.
//
// Each test gets its own gg-devserver (fresh config dir, free port) and its own temp folder
// of fixture repositories made with real git. The page reaches that devserver through
// `window.__GG_DEV_URL__`, set before any app script runs.

import { type ChildProcess, execFileSync, spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test as base, type Locator, type Page } from "@playwright/test";
import type { OpOutcome, OpRequest } from "../src/lib/types";

const ROOT = path.resolve(import.meta.dirname, "..");
const DEVSERVER = path.join(ROOT, "target", "debug", "gg-devserver");
/** The vite dev server the page is loaded from (playwright.config.ts `baseURL`). */
const VITE_ORIGIN = `http://127.0.0.1:${process.env.GG_E2E_PORT ?? 1420}`;

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
  // No GitHub host, token or repository of the user's reaches anything a test runs.
  const env = Object.fromEntries(
    Object.entries(process.env).filter(([name]) => !name.startsWith("GH_") && !name.startsWith("GITHUB_")),
  );
  return {
    ...env,
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
   * Conflicts. 40 commits of history first ("Earlier work 1".."40", so the graph
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
   * The staging routine, with a bare `origin` beside it (`routine-origin.git`)
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
  /**
   * Pull requests: `origin` is https://github.com/octo/cat.git for the app, which git's
   * `insteadOf` sends to a bare repository beside it (`prs-origin.git`), so pushes stay on disk.
   * `main` ("Initial commit", "Set up the parser"); `feature/login` ("Add login form",
   * "Validate the login form") and `feature/search-indexing-across-every-workspace-package`
   * ("Index the search") pushed with upstreams; `docs/readme` ("Explain the parser") pushed;
   * `fix/typo` ("Fix a typo in the README") not pushed. A worktree on `feature/login` beside it
   * (`prs.worktrees/login`). `main` is checked out.
   */
  prs(git, write, dir) {
    git("init", "-q", "-b", "main");
    const commit = (file: string, subject: string) => {
      write(file, `${subject}\n`);
      git("add", "-A");
      git("commit", "-q", "-m", subject);
    };
    commit("README.md", "Initial commit");
    commit("parser.ts", "Set up the parser");
    const branch = (name: string, subjects: string[]) => {
      git("switch", "-q", "-c", name, "main");
      subjects.forEach((subject, i) => commit(`${name.replaceAll("/", "-")}-${i}.txt`, subject));
    };
    branch("feature/login", ["Add login form", "Validate the login form"]);
    branch("feature/search-indexing-across-every-workspace-package", ["Index the search"]);
    branch("docs/readme", ["Explain the parser"]);
    branch("fix/typo", ["Fix a typo in the README"]);
    git("switch", "-q", "main");
    const bare = path.join(path.dirname(dir), "prs-origin.git");
    git("init", "-q", "--bare", "-b", "main", bare);
    git("remote", "add", "origin", "https://github.com/octo/cat.git");
    git("config", `url.${bare}.insteadOf`, "https://github.com/octo/cat.git");
    git("push", "-q", "-u", "origin", "main", "feature/login", "feature/search-indexing-across-every-workspace-package", "docs/readme");
    git("worktree", "add", "-q", path.join(path.dirname(dir), "prs.worktrees", "login"), "feature/login");
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
  /**
   * The commit panel's: `main` level with a bare `origin` (`commit-origin.git`), its upstream,
   * and changes of every kind in the working tree: `src/app.ts` modified, `old.txt` deleted,
   * `notes/plan.md` renamed to `notes/roadmap.md` (staged), `a file with spaces.txt` modified,
   * `todo.md` new (untracked), and a file deep in long folder names modified. A linked worktree
   * `topic` (in `commit.worktrees/topic`, branch `topic`, no upstream) with one new file. An
   * identity for the commits the app makes.
   */
  commit(git, write, dir) {
    git("init", "-q", "-b", "main");
    git("config", "user.name", "Ada Lovelace");
    git("config", "user.email", "ada@example.com");
    const deep = "packages/payments-service/src/components/checkout/summary/LineItemPriceBreakdown.tsx";
    write("src/app.ts", "export const app = 1;\n");
    write("old.txt", "old\n");
    write("notes/plan.md", "# Plan\n\nShip it.\n");
    write("a file with spaces.txt", "spaces\n");
    write(deep, "export {};\n");
    git("add", "-A");
    git("commit", "-q", "-m", "Start the app");
    const origin = path.join(path.dirname(dir), "commit-origin.git");
    git("init", "-q", "--bare", "-b", "main", origin);
    git("remote", "add", "origin", origin);
    git("push", "-q", "-u", "origin", "main");
    git("worktree", "add", "-q", "-b", "topic", path.join(path.dirname(dir), "commit.worktrees", "topic"));
    writeFileSync(path.join(path.dirname(dir), "commit.worktrees", "topic", "topic.txt"), "topic\n");
    write("src/app.ts", "export const app = 2;\n");
    rmSync(path.join(dir, "old.txt"));
    git("mv", "notes/plan.md", "notes/roadmap.md");
    write("a file with spaces.txt", "spaces, changed\n");
    write("todo.md", "- write the docs\n");
    write(deep, "export const total = 0;\n");
  },
  /**
   * The Commit tab's "Committed on branch": on `feature/report` (pushed, tracking
   * origin/feature/report), clean, two commits after it left `main`: `src/report.ts` modified
   * (+2 −1), `docs/old-guide.md` renamed to `docs/guide.md` (one line added), `legacy.txt`
   * deleted, `assets/logo.bin` (binary) and `src/new.ts` added. `main` moved on after it left
   * (`main-only.txt`), and origin's HEAD names `main` (`branchfiles-origin.git`). `bulk` (no
   * upstream) adds 520 files to `main`. An identity for the commits the app makes.
   */
  branchfiles(git, write, dir) {
    git("init", "-q", "-b", "main");
    git("config", "user.name", "Ada Lovelace");
    git("config", "user.email", "ada@example.com");
    write("src/report.ts", "export const a = 1;\nexport const b = 2;\nexport const c = 3;\n");
    write("docs/old-guide.md", "# Guide\n\nOne.\nTwo.\nThree.\nFour.\n");
    write("legacy.txt", "legacy\n");
    git("add", "-A");
    git("commit", "-q", "-m", "Start the report");
    const origin = path.join(path.dirname(dir), "branchfiles-origin.git");
    git("init", "-q", "--bare", "-b", "main", origin);
    git("remote", "add", "origin", origin);
    git("push", "-q", "-u", "origin", "main");
    git("switch", "-q", "-c", "bulk");
    for (let i = 0; i < 520; i++) write(`bulk/f${String(i).padStart(3, "0")}.txt`, `${i}\n`);
    git("add", "-A");
    git("commit", "-q", "-m", "Add 520 files");
    git("switch", "-q", "-c", "feature/report", "main");
    write("src/report.ts", "export const a = 1;\nexport const b = 20;\nexport const c = 3;\nexport const d = 4;\n");
    git("mv", "docs/old-guide.md", "docs/guide.md");
    write("docs/guide.md", "# Guide\n\nOne.\nTwo.\nThree.\nFour.\nFive.\n");
    git("rm", "-q", "legacy.txt");
    git("add", "-A");
    git("commit", "-q", "-m", "Rework the report");
    mkdirSync(path.join(dir, "assets"), { recursive: true });
    writeFileSync(path.join(dir, "assets", "logo.bin"), Buffer.from([0, 159, 146, 150, 0, 1, 2, 3]));
    write("src/new.ts", "export {};\n");
    git("add", "-A");
    git("commit", "-q", "-m", "Add a logo and a module");
    git("push", "-q", "-u", "origin", "feature/report");
    git("switch", "-q", "main");
    write("main-only.txt", "main\n");
    git("add", "-A");
    git("commit", "-q", "-m", "Main moves on");
    git("push", "-q", "origin", "main");
    git("switch", "-q", "feature/report");
    git("remote", "set-head", "origin", "main");
  },
  /**
   * Worktrees in every state, beside a bare `origin` (`worktrees-origin.git`); the linked ones
   * in `worktrees.worktrees/` (the default location pattern). `main` (the main worktree): clean,
   * level with origin/main, a `.gitignore` for `build/`. `feature/login`: 2 ahead and 1 behind
   * origin/feature/login, one changed and one new file. `fix/typo`: no upstream, 1 commit on no
   * remote. `detached`: HEAD detached at main~2. `release`: locked ("on a USB drive").
   * `old-spike`: its folder deleted (prunable). `docs`: clean but for ignored files in
   * `build/`. `hotfix`: a merge stopped on a conflict in `app.txt`. The repository has an
   * identity for the commits the app makes.
   */
  worktrees(git, _write, dir) {
    git("init", "-q", "-b", "main");
    git("config", "user.name", "Ada Lovelace");
    git("config", "user.email", "ada@example.com");
    const commit = (file: string, content: string, subject: string, at: Git = git, base = dir) => {
      const target = path.join(base, file);
      mkdirSync(path.dirname(target), { recursive: true });
      writeFileSync(target, content);
      at("add", "-A");
      at("commit", "-q", "-m", subject);
    };
    commit(".gitignore", "build/\n", "Ignore build output");
    commit("app.txt", "app\n", "Start the app");
    commit("README.md", "worktrees\n", "Add a readme");
    commit("docs/guide.md", "guide\n", "Write the guide");
    // Absolute: the linked worktrees push from their own folders.
    const origin = path.join(path.dirname(dir), "worktrees-origin.git");
    git("init", "-q", "--bare", "-b", "main", origin);
    git("remote", "add", "origin", origin);
    git("branch", "-q", "feature/login");
    git("branch", "-q", "fix/typo");
    git("branch", "-q", "release");
    git("branch", "-q", "old-spike");
    git("branch", "-q", "docs");
    git("branch", "-q", "hotfix");
    git("push", "-q", "-u", "origin", "main", "feature/login");
    const base = path.join(path.dirname(dir), "worktrees.worktrees");
    const add = (folder: string, ...args: string[]) => {
      const at = path.join(base, folder);
      git("worktree", "add", "-q", at, ...args);
      return at;
    };

    const login = add("feature-login", "feature/login");
    const inLogin = gitIn(login);
    advanceClock(login, clocks.get(dir) ?? 0);
    commit("login/remote.txt", "theirs\n", "Login on the remote", inLogin, login);
    inLogin("push", "-q", "origin", "feature/login");
    inLogin("reset", "-q", "--hard", "HEAD~1");
    commit("login/form.txt", "form\n", "Add the login form", inLogin, login);
    commit("login/check.txt", "check\n", "Check the password", inLogin, login);
    inLogin("fetch", "-q", "origin");
    writeFileSync(path.join(login, "app.txt"), "app with login\n");
    writeFileSync(path.join(login, "login/notes.txt"), "notes\n");

    const typo = add("fix-typo", "fix/typo");
    const inTypo = gitIn(typo);
    advanceClock(typo, clocks.get(login) ?? 0);
    commit("README.md", "worktrees, fixed\n", "Fix a typo in the readme", inTypo, typo);

    add("detached", "--detach", "main~2");
    add("release", "release");
    git("worktree", "lock", "--reason", "on a USB drive", path.join(base, "release"));
    rmSync(add("old-spike", "old-spike"), { recursive: true, force: true });

    const docs = add("docs", "docs");
    mkdirSync(path.join(docs, "build"), { recursive: true });
    writeFileSync(path.join(docs, "build", "site.html"), "<p>built</p>\n");

    const hotfix = add("hotfix", "hotfix");
    const inHotfix = gitIn(hotfix);
    advanceClock(hotfix, clocks.get(typo) ?? 0);
    git("switch", "-q", "-c", "patch");
    commit("app.txt", "app, patched\n", "Patch the app");
    git("switch", "-q", "main");
    commit("app.txt", "app, hotfixed\n", "Hotfix the app", inHotfix, hotfix);
    try {
      inHotfix("merge", "-q", "patch");
    } catch {
      // Stopped on the conflict in app.txt, as wanted.
    }
    advanceClock(dir, clocks.get(hotfix) ?? 0);
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
function launchRecord(configDir: string): string {
  return path.join(path.dirname(configDir), "launches.jsonl");
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
 * A recording stand-in for `gh` (and `ssh` beside it) in `<home>/tools`, the folder the
 * devserver finds and runs them in (`--tool-path`): the real ones are never run, nothing
 * reaches GitHub. Each `gh` call's arguments are recorded; it answers with what the test set
 * for that kind of call (`pr-list`, `pr-list-head` with `--head`, `repo-view`, `pr-create`,
 * `auth-token`, `pr-view`, `api-graphql`; a pull request's own `pr-view-<n>` and
 * `api-graphql-<n>` first, when set; the Pull request tab's writes: `gql-reply`, `gql-resolve`,
 * `gql-unresolve` (`gh api graphql` with that mutation) and `pr-comment`; the Pull requests
 * page's query, `gql-inbox`, or `gql-inbox-<host>` for `GH_HOST` `<host>`, its dots as `_`),
 * else prints nothing.
 * A call that exits 0 then puts the answers set for it with `then` in place (what GitHub
 * answers once it has the write). `ssh -G` says the host it is asked about is itself.
 */
const FAKE_GH = `#!/bin/sh
PATH=/usr/bin:/bin
export PATH
d="$(cd "$(dirname "$0")/.." && pwd)/gh"
i=1
while ! mkdir "$d/calls/$i" 2>/dev/null; do i=$((i+1)); done
for a in "$@"; do printf '%s\\0' "$a"; done > "$d/calls/$i/argv"
cat > "$d/calls/$i/stdin"
key="$1-$2"
case " $* " in *" --head "*) if [ "$key" = pr-list ]; then key=pr-list-head; fi;; esac
if [ "$key" = api-graphql ]; then
  for a in "$@"; do case "$a" in *unresolveReviewThread*) key=gql-unresolve;; *resolveReviewThread*) key=gql-resolve;; *GgInboxDetails*) key=gql-inbox-details;; *GgInbox*) key=gql-inbox;; *GgRepos*) key=gql-repos;; esac; done
  if grep -q addPullRequestReviewThreadReply "$d/calls/$i/stdin"; then key=gql-reply; fi
fi
case "$key" in gql-inbox*) if [ -n "$GH_HOST" ]; then
  h="$(printf '%s' "$GH_HOST" | tr . _)"
  if [ -f "$d/replies/$key-$h.code" ]; then key="$key-$h"; fi
fi;; esac
if [ "$1" = api ] && [ "$2" = --hostname ]; then case "$5" in repos/*/files*) key=api-files; n="$(printf '%s' "$5" | sed 's#.*/pulls/\\([0-9]*\\)/files.*#\\1#')"; if [ -f "$d/replies/api-files-$n.code" ]; then key="api-files-$n"; fi;; esac; fi
n=""
if [ "$key" = pr-view ]; then n="$3"; fi
if [ "$key" = api-graphql ]; then for a in "$@"; do case "$a" in n=*) n="\${a#n=}";; esac; done; fi
if [ -n "$n" ] && { [ -f "$d/replies/$key-$n.out" ] || [ -f "$d/replies/$key-$n.code" ]; }; then key="$key-$n"; fi
r="$d/replies/$key"
if [ -f "$r.sleep" ]; then sleep "$(cat "$r.sleep")"; fi
if [ -f "$r.err" ]; then cat "$r.err" >&2; fi
if [ -f "$r.out" ]; then cat "$r.out"; fi
code=0
if [ -f "$r.code" ]; then code="$(cat "$r.code")"; fi
if [ "$code" = 0 ] && [ -d "$r.then" ]; then cp "$r.then"/* "$d/replies/"; fi
exit "$code"
`;
const FAKE_SSH = `#!/bin/sh
PATH=/usr/bin:/bin
export PATH
for a in "$@"; do host="$a"; done
printf 'hostname %s\\n' "$host"
`;

/** Writes the fake `gh` and `ssh` into `home`; returns the `PATH` the devserver runs them on. */
function fakeTools(home: string): string {
  const tools = path.join(home, "tools");
  for (const dir of [tools, path.join(home, "gh", "calls"), path.join(home, "gh", "replies")]) mkdirSync(dir, { recursive: true });
  writeFileSync(path.join(tools, "gh"), FAKE_GH, { mode: 0o755 });
  writeFileSync(path.join(tools, "ssh"), FAKE_SSH, { mode: 0o755 });
  // The fakes set their own PATH; nothing else is found here (the core takes gh and ssh from
  // the first folder only).
  return tools;
}

/** The folder the devserver's stand-in for the updater answers from (`--fake-updates`). */
function updatesDir(home: string): string {
  const dir = path.join(home, "updates");
  mkdirSync(dir, { recursive: true });
  return dir;
}

/**
 * The stand-in for gg's updater (gg-devserver's `--fake-updates`): what the update source
 * offers, what fails, why this copy cannot update itself, and the restarts asked for. Read
 * by the devserver at each check, so a test may change it between checks.
 */
export type FakeUpdates = {
  /** The running version (default: the core's). */
  running(version: string): void;
  /** The update source offers `version` (null: nothing newer). */
  offer(version: string | null): void;
  /** The check fails with `kind` (`offline`, `notFound`, `badManifest`, …) and `detail`. */
  checkFails(kind: string, detail: string): void;
  /** The download fails, or (null) works again. */
  download(fail: { kind: string; detail: string } | null): void;
  /** Downloads stop halfway until `openDownloadGate`. */
  holdDownloads(): void;
  openDownloadGate(): void;
  /** Why this copy cannot update itself (`translocated`, `diskImage`, `notWritable <folder>`, …); null: it can. */
  blocker(reason: string | null): void;
  /** The restarts asked for (the running version each), oldest first. */
  restarts(): string[];
};

function fakeUpdates(home: string): FakeUpdates {
  const dir = updatesDir(home);
  const put = (name: string, text: string) => writeFileSync(path.join(dir, name), text);
  return {
    running: (version) => put("version", version),
    offer: (version) => put("check.json", JSON.stringify({ offer: version })),
    checkFails: (kind, detail) => put("check.json", JSON.stringify({ fail: kind, detail })),
    download(fail) {
      if (fail) put("download.json", JSON.stringify({ fail: fail.kind, detail: fail.detail }));
      else rmSync(path.join(dir, "download.json"), { force: true });
    },
    holdDownloads: () => put("download-gate", ""),
    openDownloadGate: () => rmSync(path.join(dir, "download-gate"), { force: true }),
    blocker(reason) {
      if (reason) put("blocker", reason);
      else rmSync(path.join(dir, "blocker"), { force: true });
    },
    restarts() {
      const file = path.join(dir, "restarts");
      return existsSync(file) ? readFileSync(file, "utf8").split("\n").filter((l) => l !== "") : [];
    },
  };
}

/** The stand-in for the AI tools, built with the devserver (`crates/gg-devserver/src/bin/gg-fake-ai.rs`). */
const FAKE_AI = path.join(ROOT, "target", "debug", "gg-fake-ai");

/**
 * The stand-in for the command-line AI tools (`claude`, `codex`) of a test: none is installed
 * until `install`; then the devserver finds it in the tool folder and runs it, never a real
 * tool. Each call's arguments and stdin are recorded; it answers with what the test set.
 */
export type FakeAi = {
  install(name: "claude" | "codex"): void;
  uninstall(name: "claude" | "codex"): void;
  /** Answers each call with `stdout` (and `stderr`), exiting with `code`, after `sleepMs`. */
  reply(name: "claude" | "codex", code: number, stdout: string, stderr?: string, sleepMs?: number): void;
  /** Each call's arguments and what it read on stdin, oldest first. */
  calls(name: "claude" | "codex"): { argv: string[]; stdin: string; done: boolean; pid: number }[];
  /**
   * Waits until call `i` (0 = first) of `name` has ended, however it ended (it answered, or was
   * ended), and says whether it answered: a call ended by Cancel never does.
   */
  ended(name: "claude" | "codex", i: number): Promise<{ answered: boolean }>;
};

function fakeAi(home: string): FakeAi {
  const tools = path.join(home, "tools");
  const state = (name: string) => {
    const dir = path.join(tools, `${name}.fake`);
    mkdirSync(dir, { recursive: true });
    return dir;
  };
  return {
    install(name) {
      execFileSync("cp", [FAKE_AI, path.join(tools, name)]);
    },
    uninstall(name) {
      rmSync(path.join(tools, name), { force: true });
    },
    reply(name, code, stdout, stderr = "", sleepMs = 0) {
      const dir = state(name);
      writeFileSync(path.join(dir, "reply.out"), stdout);
      writeFileSync(path.join(dir, "reply.err"), stderr);
      writeFileSync(path.join(dir, "reply.code"), String(code));
      writeFileSync(path.join(dir, "reply.sleep"), String(sleepMs));
    },
    async ended(name, i) {
      const alive = (pid: number) => {
        try {
          process.kill(pid, 0);
          return true;
        } catch {
          return false;
        }
      };
      const deadline = Date.now() + 15_000;
      for (;;) {
        const call = this.calls(name)[i];
        if (call && !alive(call.pid)) return { answered: this.calls(name)[i].done };
        if (Date.now() > deadline) throw new Error(`${name} call ${i} did not end`);
        await new Promise((r) => setTimeout(r, 50));
      }
    },
    calls(name) {
      const out: { argv: string[]; stdin: string; done: boolean; pid: number }[] = [];
      for (let i = 1; existsSync(path.join(state(name), "calls", String(i), "argv")); i++) {
        const c = path.join(state(name), "calls", String(i));
        const argv = readFileSync(path.join(c, "argv"), "utf8").split("\0");
        argv.pop();
        out.push({
          argv,
          stdin: readFileSync(path.join(c, "stdin"), "utf8"),
          done: existsSync(path.join(c, "done")),
          pid: Number(readFileSync(path.join(c, "pid"), "utf8")),
        });
      }
      return out;
    },
  };
}

/** The fake `gh` of a test (see FAKE_GH). */
export type FakeGh = {
  /** Answers calls of `key` (`pr-list`, `pr-list-head`, `repo-view`, `pr-create`, `auth-token`). */
  reply(key: string, code: number, stdout: string, stderr?: string): void;
  /** Makes calls of `key` wait `secs` seconds before answering. */
  slow(key: string, secs: number): void;
  /** Calls of `key` answer at once again. */
  fast(key: string): void;
  /** Once a call of `key` exits 0, calls of each key of `then` answer (exit 0) with its text. */
  then(key: string, then: Record<string, string>): void;
  /** Each call's arguments so far, oldest first. */
  calls(): string[][];
  /** What the call `i` (0 = first) read on stdin. */
  stdin(i: number): string;
  /** Takes `gh` away: the devserver finds none, as when it is not installed. */
  uninstall(): void;
  /** Puts `gh` back, as when the user installs it. */
  install(): void;
};

function fakeGh(home: string): FakeGh {
  const dir = path.join(home, "gh");
  const callDir = (i: number) => path.join(dir, "calls", String(i + 1));
  return {
    reply(key, code, stdout, stderr = "") {
      const base = path.join(dir, "replies", key);
      writeFileSync(`${base}.out`, stdout);
      writeFileSync(`${base}.err`, stderr);
      writeFileSync(`${base}.code`, String(code));
    },
    slow(key, secs) {
      writeFileSync(path.join(dir, "replies", `${key}.sleep`), String(secs));
    },
    fast(key) {
      rmSync(path.join(dir, "replies", `${key}.sleep`), { force: true });
    },
    then(key, then) {
      const into = path.join(dir, "replies", `${key}.then`);
      rmSync(into, { recursive: true, force: true });
      mkdirSync(into, { recursive: true });
      for (const [k, stdout] of Object.entries(then)) {
        writeFileSync(path.join(into, `${k}.out`), stdout);
        writeFileSync(path.join(into, `${k}.err`), "");
        writeFileSync(path.join(into, `${k}.code`), "0");
      }
    },
    calls() {
      const out: string[][] = [];
      for (let i = 0; existsSync(path.join(callDir(i), "argv")); i++) {
        out.push(readFileSync(path.join(callDir(i), "argv"), "utf8").split("\0").filter((a) => a !== ""));
      }
      return out;
    },
    stdin(i) {
      return readFileSync(path.join(callDir(i), "stdin"), "utf8");
    },
    uninstall() {
      rmSync(path.join(home, "tools", "gh"));
    },
    install() {
      writeFileSync(path.join(home, "tools", "gh"), FAKE_GH, { mode: 0o755 });
    },
  };
}

/**
 * A stand-in for the login shell the devserver asks what `gg` is (`--cli-shell`): a POSIX sh
 * script with a bare `PATH` and no `ZDOTDIR` that runs `setup` (an alias, say) and then the
 * command it is given. Named `name`: `zsh` or `bash` make the devserver take it for that shell
 * (in `<home>/shell`), and so offer to turn off its `gg` in the test's home folder. The user's
 * own shell and its files are never run.
 */
function fakeShell(home: string, setup: string, name = "fake-shell"): string {
  const dir = name === "fake-shell" ? home : path.join(home, "shell");
  mkdirSync(dir, { recursive: true });
  const file = path.join(dir, name);
  writeFileSync(file, `#!/bin/sh\nunset ZDOTDIR\nPATH=/usr/bin:/bin\n${setup}\neval "$2"\n`, { mode: 0o755 });
  return file;
}

/**
 * Starts a devserver whose home folder is `home` (so paths under it show as `~/…`); the
 * debug build unless `binary` names another. The debug build records launches (editor, terminal, file manager, browser) instead
 * of starting an editor (see `GG.launches`). The `gg` command is looked for in
 * `cliFolders(home, extraCliFolders)` only, and the stand-in shell, set up with `shellSetup`, is what is asked
 * what `gg` is.
 */
export function startDevserver(
  configDir: string,
  home: string,
  binary = DEVSERVER,
  shellSetup = "",
  extraCliFolders = 0,
  shellName = "fake-shell",
  extraArgs: readonly string[] = [],
): Promise<Devserver> {
  const cliPath = cliFolders(home, extraCliFolders).join(path.delimiter);
  const shell = fakeShell(home, shellSetup, shellName);
  // No ZDOTDIR of the user's reaches the devserver or its stand-in shell.
  const env = isolatedEnv();
  delete env.ZDOTDIR;
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
      ...(binary === DEVSERVER ? ["--record-launches", launchRecord(configDir)] : []),
      "--cli-path",
      cliPath,
      "--cli-shell",
      shell,
      // gh and ssh: the fakes only (see FAKE_GH).
      "--tool-path",
      fakeTools(home),
      // Updating gg: the stand-in only, answering from files (see FakeUpdates); nothing is downloaded.
      ...(binary === DEVSERVER ? ["--fake-updates", updatesDir(home)] : []),
      // The Pull requests page: no floor between its loads, and clones from local folders
      // (`<home>/remotes/<owner>/<name>.git`), never the network.
      ...(binary === DEVSERVER ? ["--inbox-every-ms", "0", "--clone-source", path.join(home, "remotes")] : []),
      ...extraArgs,
    ],
    // Its own process group, so stopping it also stops whatever it started. Its stdin stays
    // open while this process lives: if the runner dies (Ctrl-C), the pipe closes and the
    // server quits, though it is detached.
    { stdio: ["pipe", "pipe", "pipe"], env: { ...env, HOME: home, USERPROFILE: home }, detached: true },
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
type Unplaced<R> = R extends { expectWorktree: unknown }
  ? Omit<R, "expectWorktree"> & Partial<Pick<R, "expectWorktree">>
  : R;
/**
 * A request as a test writes it: one on the current branch may leave out `expectHead`, and one
 * that runs in a worktree `expectWorktree`, which `runOp` fills in with HEAD and the active
 * worktree as the active tab shows them when the request is sent, as a menu would.
 */
export type Asked = Unplaced<Unheaded<OpRequest>>;

/** Runs `request` through the app's own runOp (the module the page loaded), in the active tab. */
export function runOp(page: Page, request: Asked): Promise<OpOutcome> {
  return page.evaluate(async (req) => {
    const ops = await import("/src/lib/state/ops.svelte.ts");
    const { app } = await import("/src/lib/state/app.svelte.ts");
    const { findRepoView } = await import("/src/lib/state/repo.svelte.ts");
    const id = app.activeRepoId!;
    const snap = findRepoView(id)!.snapshot;
    const onHead = ["reset", "merge", "rebase", "cherry_pick", "revert", "pull", "stash_apply", "stash_pop", "continue", "abort", "skip"];
    const elsewhere = ["push", "push_tag", "delete_tag", "delete_remote_branch", "fetch_into_local"];
    const headed = onHead.includes(req.op) && !("expectHead" in req) ? { ...req, expectHead: snap.head } : req;
    const full =
      !elsewhere.includes(req.op) && !("expectWorktree" in headed)
        ? { ...headed, expectWorktree: snap.activeWorktree }
        : headed;
    return ops.runOp(id, full as OpRequest);
  }, request);
}

export function closeButton(page: Page, name: string) {
  return page.getByRole("tab", { name, exact: true }).locator("xpath=..").locator(".close");
}

/**
 * How tall the status bar at the window's foot is, px. A test that measures the graph or the
 * panels under it makes its window this much taller, so they keep the room they were written for.
 */
export const STATUS_BAR_H = 22;

/** The toolbar's View popover (open or not). */
export function viewPopover(page: Page): Locator {
  return page.getByRole("dialog", { name: "View", exact: true });
}

/** Opens the toolbar's View popover, unless it is open, and gives its checkbox `name`. */
export async function viewOption(page: Page, name: "Remote branches" | "Tags" | "Stashes"): Promise<Locator> {
  if (!(await viewPopover(page).isVisible())) await page.getByTestId("view-options").click();
  return viewPopover(page).getByRole("checkbox", { name, exact: true });
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
  /** The launches asked for so far (editor, terminal, file manager, browser: program and arguments each), oldest first; none is started. */
  launches(): string[][];
  /** The fake `gh` the devserver runs (the real one never runs). */
  gh: FakeGh;
  /** The stand-ins for the AI tools (none installed until a test says so; a real one never runs). */
  ai: FakeAi;
  /** The stand-in for gg's updater. */
  updates: FakeUpdates;
  /** The app quit and started again: a new devserver on the same config folder, the page loaded afresh. */
  relaunch(): Promise<void>;
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

export const test = base.extend<{ gg: GG; ggShellSetup: string; ggShellName: string; ggCliFolders: number; ggDevserverArgs: string[] }>({
  /** What the stand-in login shell defines before it is asked what `gg` is (`alias gg=…`). */
  ggShellSetup: ["", { option: true }],
  /** The stand-in login shell's name: `zsh` or `bash` to be taken for that shell. */
  ggShellName: ["fake-shell", { option: true }],
  /** Folders the `gg` command may go in besides `~/.local/bin` and `~/bin`. */
  ggCliFolders: [0, { option: true }],
  /** More of the devserver's flags (`--pr-details-every-ms`, say). */
  ggDevserverArgs: [[], { option: true }],
  gg: async ({ page, ggShellSetup, ggShellName, ggCliFolders, ggDevserverArgs }, use) => {
    // realpath: macOS's temp folder is behind a symlink, and the core reports real paths.
    const root = realpathSync(mkdtempSync(path.join(tmpdir(), "gg-e2e-")));
    let server: Devserver | null = null;
    try {
      server = await startDevserver(path.join(root, "config"), root, DEVSERVER, ggShellSetup, ggCliFolders, ggShellName, ggDevserverArgs);
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

      const launches = (): string[][] => {
        const file = launchRecord(path.join(root, "config"));
        if (!existsSync(file)) return [];
        return readFileSync(file, "utf8")
          .split("\n")
          .filter((l) => l !== "")
          .map((l) => JSON.parse(l) as string[]);
      };

      const relaunch = async () => {
        if (server) await stopDevserver(server);
        server = null;
        server = await startDevserver(path.join(root, "config"), root, DEVSERVER, ggShellSetup, ggCliFolders, ggShellName, ggDevserverArgs);
        // Run after the first one, so this address wins.
        await page.addInitScript((url) => {
          (window as unknown as { __GG_DEV_URL__: string }).__GG_DEV_URL__ = url;
        }, server.url);
        await page.reload();
      };

      await use({ page, home: root, repoPath, folder, git, gitAt, launches, gh: fakeGh(root), ai: fakeAi(root), updates: fakeUpdates(root), relaunch });
    } finally {
      if (server) await stopDevserver(server);
      // A tool the devserver started may still be writing what it was asked (a fake gh's record
      // of a call), in a process group of its own: retried until it is done.
      rmSync(root, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
    }
  },
});

export { expect } from "@playwright/test";
