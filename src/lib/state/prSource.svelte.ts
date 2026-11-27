// Where a pull request shown in full (PullRequestView: its state, checks and comments, with
// replies and resolving) comes from and goes to: an open repository's side panel (its Pull
// request tab), or the Pull requests page (a repository named by its host, owner and name,
// open in gg or not). The view reads and writes only through this, so it is the same view in
// both places.

import { openExternal } from "./editor";
import { repoTarget, type PrTarget } from "./prCompose.svelte";
import { loadDetails } from "./prDetails.svelte";
import type { PrCompose } from "./prDrafts.svelte";
import type { RepoView } from "./repo.svelte";
import { showLog } from "./app.svelte";
import { panelState } from "./worktrees.svelte";

export type PrSource = {
  /** What its pull requests' details are kept under (prDetails.svelte.ts). */
  key: string;
  /** Loads pull request `number`'s details (`background`: a poll). */
  load(number: number, background?: boolean): Promise<void>;
  /** Where pull request `number`'s writes go. */
  target(number: number): PrTarget;
  /** The drafts of replies and comments. */
  compose: PrCompose;
  /** Shows the command log from record `from`. */
  showLog(from: number): void;
  /** Opens a pull request's page (on its GitHub host only), or any https link, in the browser. */
  openPage(url: string): void;
  openLink(url: string): void;
  /** Changes when the details should be loaded again: the user refreshed, the list reloaded. */
  readonly refresh: string;
  /** Changes after a push of the pull request's branch (loaded again, and once more later). */
  readonly pushed: number;
};

const sources = new WeakMap<RepoView, PrSource>();

/**
 * The pull requests of open repository `view`, as its side panel's Pull request tab shows them:
 * one object per view, which reads the view only when asked (a new snapshot changes nothing).
 */
export function repoPrSource(view: RepoView): PrSource {
  const known = sources.get(view);
  if (known) return known;
  const repoId = view.repoId;
  const root = () => view.snapshot.rootPath;
  const panel = () => panelState(repoId);
  const made: PrSource = {
    get key() {
      return root();
    },
    load: (number, background = false) => loadDetails(repoId, root(), number, background),
    target: (number) => repoTarget(repoId, root(), number),
    get compose() {
      return panel().prCompose;
    },
    showLog: (from) => showLog(repoId, from),
    openPage: (url) => void openExternal(repoId, { target: "url", url }),
    openLink: (url) => void openExternal(repoId, { target: "link", url }),
    get refresh() {
      return `${panel().prRefresh}:${view.snapshot.prs.loadedAtMs}`;
    },
    get pushed() {
      return panel().prPushed;
    },
  };
  sources.set(view, made);
  return made;
}
