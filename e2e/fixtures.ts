// Playwright fixtures for end-to-end tests against the real core.
//
// Each test gets its own gg-devserver (fresh config dir, free port) and its own temp folder
// of fixture repositories made with real git. The page reaches that devserver through
// `window.__GG_DEV_URL__`, set before any app script runs.

import { type ChildProcess, execFileSync, spawn } from "node:child_process";
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test as base, type Page } from "@playwright/test";

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

function gitIn(dir: string): Git {
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
        GIT_AUTHOR_NAME: "Ada Lovelace",
        GIT_AUTHOR_EMAIL: "ada@example.com",
        GIT_COMMITTER_NAME: "Ada Lovelace",
        GIT_COMMITTER_EMAIL: "ada@example.com",
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

/**
 * Starts a devserver whose home folder is `home` (so paths under it show as `~/…`); the
 * debug build unless `binary` names another.
 */
export function startDevserver(configDir: string, home: string, binary = DEVSERVER): Promise<Devserver> {
  const child = spawn(
    binary,
    ["--port", "0", "--config-dir", configDir, "--allow-origin", VITE_ORIGIN],
    { stdio: ["ignore", "pipe", "pipe"], env: { ...isolatedEnv(), HOME: home, USERPROFILE: home } },
  );
  return new Promise((resolve, reject) => {
    let out = "";
    let err = "";
    const timer = setTimeout(() => fail(new Error("gg-devserver did not start within 20 s")), 20_000);
    const fail = (e: Error) => {
      clearTimeout(timer);
      child.kill();
      reject(new Error(`${e.message}\n${err}`));
    };
    child.stderr!.on("data", (d: Buffer) => (err += d.toString()));
    child.stdout!.on("data", (d: Buffer) => {
      out += d.toString();
      const match = /listening on (http:\/\/\S+)/.exec(out);
      if (match) {
        clearTimeout(timer);
        resolve({ url: match[1], process: child });
      }
    });
    child.on("error", fail);
    child.on("exit", (code) => fail(new Error(`gg-devserver exited with ${code}`)));
  });
}

export async function stopDevserver(server: Devserver): Promise<void> {
  const child = server.process;
  if (child.exitCode !== null || child.signalCode !== null) return;
  const exited = new Promise<void>((resolve) => child.once("exit", () => resolve()));
  child.kill();
  await exited;
}

export type GG = {
  page: Page;
  /** Path of fixture repository `name` (`small`, `empty`, `big`, `branchy`), created on first use. */
  repoPath(name: string): string;
  /** Path of a plain (non-repository) folder `name`, created on first use. */
  folder(name: string): string;
  /** Runs git in fixture repository `name`; returns stdout. */
  git(name: string, ...args: string[]): string;
};

export const test = base.extend<{ gg: GG }, { devserverBuilt: void }>({
  devserverBuilt: [
    async ({}, use) => {
      // Once per worker; cargo serializes concurrent builds and skips an up-to-date one.
      execFileSync("cargo", ["build", "-q", "-p", "gg-devserver"], {
        cwd: ROOT,
        stdio: ["ignore", "ignore", "pipe"],
        env: { ...process.env, CARGO_INCREMENTAL: "0" },
      });
      await use();
    },
    { scope: "worker", timeout: 600_000 },
  ],

  gg: async ({ page, devserverBuilt: _ }, use) => {
    // realpath: macOS's temp folder is behind a symlink, and the core reports real paths.
    const root = realpathSync(mkdtempSync(path.join(tmpdir(), "gg-e2e-")));
    const server = await startDevserver(path.join(root, "config"), root);
    try {
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

      await use({ page, repoPath, folder, git });
    } finally {
      await stopDevserver(server);
      rmSync(root, { recursive: true, force: true });
    }
  },
});

export { expect } from "@playwright/test";
