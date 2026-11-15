// Captures the app screenshots the site uses, at 1× and 2× device pixels, into
// site/.shots/1x and site/.shots/2x (git-ignored). Run from anywhere:
//   node site/scripts/shots.mjs
//
// It runs the app's own screenshot spec (e2e/screens.spec.ts), copied into site/.shots/spec
// with three changes: the device scale factor comes from GG_SHOTS_SCALE, the shots go to
// site/.shots/<scale>x, and the fixtures are imported from e2e/. Only the tests that take the
// shots the page uses run. Nothing outside site/ is written except cargo's target/ folder in
// this checkout (git-ignored), which gg-devserver is built into.
import { spawnSync } from "node:child_process";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";

const root = path.resolve(import.meta.dirname, "..", "..");
const shots = path.join(root, "site", ".shots");
const specDir = path.join(shots, "spec");

// The tests whose shots the page uses (see screens.sh for which shot and crop).
const grep = [
  "(^| )graph, (light|dark)$", // graph-branchy
  "small window, focus and hover", // small-search
  "menus, dialogs and operations, 1280x800", // menu-branch
  "branch dialogs and the staging routine, 1280x800", // notice-merge-push
  "conflict banner and stash dialogs, 900x560", // small-banner-merge-partly
  "command log, 900x560", // small-log-open
  "remaining menus, confirmations and notices, 1280x800", // dialog-force-push
].join("|");

const source = readFileSync(path.join(root, "e2e", "screens.spec.ts"), "utf8");
const edits = [
  [
    'const OUT = path.resolve(import.meta.dirname, "..", "docs", "perf", "screens");',
    `const OUT = path.join(${JSON.stringify(shots)}, process.env.GG_SHOTS_SCALE + "x");`,
  ],
  ["deviceScaleFactor: 1 }", "deviceScaleFactor: Number(process.env.GG_SHOTS_SCALE) }"],
  ['from "./fixtures"', `from ${JSON.stringify(path.join(root, "e2e", "fixtures.ts"))}`],
];
let spec = source;
for (const [from, to] of edits) {
  if (!spec.includes(from)) throw new Error(`e2e/screens.spec.ts changed: cannot find ${from}`);
  spec = spec.replace(from, to);
}
rmSync(specDir, { recursive: true, force: true });
mkdirSync(specDir, { recursive: true });
writeFileSync(path.join(specDir, "screens.spec.ts"), spec);

for (const scale of ["1", "2"]) {
  mkdirSync(path.join(shots, `${scale}x`), { recursive: true });
  const run = spawnSync(
    "pnpm",
    ["exec", "playwright", "test", "-c", path.join(root, "site", "scripts", "shots.config.ts"), "--grep", grep],
    {
      cwd: root,
      stdio: "inherit",
      env: {
        ...process.env,
        GG_SCREENS: "1",
        GG_SHOTS_SCALE: scale,
        // This checkout's own target folder (where the fixtures look for gg-devserver).
        CARGO_TARGET_DIR: path.join(root, "target"),
        CARGO_INCREMENTAL: "0",
      },
    },
  );
  if (run.status !== 0) process.exit(run.status ?? 1);
}
