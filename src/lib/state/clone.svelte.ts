// Cloning, from the app's side: the Clone dialog (state kept here: the user's repositories as
// gh last listed them, for the whole session), the + menu, where clones go, and going to a
// repository gg already has instead of cloning it again.

import { tick } from "svelte";
import { cloneLocation } from "../clone";
import CloneDialog from "../components/dialogs/CloneDialog.svelte";
import NewTabMenu from "../components/NewTabMenu.svelte";
import { api } from "../ipc";
import type { KnownRepo, RepoList } from "../types";
import { activate, app, openPath } from "./app.svelte";
import { closeInbox } from "./inbox.svelte";
import { overlays, showDialog } from "./overlays.svelte";

/** The user's repositories as gh last listed them this session, and whether a list is being asked for. */
export const repoList = $state({ list: null as RepoList | null, loading: false });

let asking: Promise<void> | null = null;

/**
 * Asks gh for the user's repositories, unless a list is kept (`again`: ask anyway). A request
 * while one is out waits for it.
 */
export function loadRepos(again = false): Promise<void> {
  if (asking) return asking;
  if (repoList.list && repoList.list.problem === null && !again) return Promise.resolve();
  repoList.loading = true;
  asking = api
    .cloneRepos()
    .then(
      (list) => {
        repoList.list = list;
      },
      (e: unknown) => {
        repoList.list = {
          host: "github.com",
          login: null,
          repos: [],
          total: 0,
          problem: "failed",
          message: e instanceof Error ? e.message : String(e),
          detail: null,
        };
      },
    )
    .finally(() => {
      repoList.loading = false;
      asking = null;
    });
  return asking;
}

/** Where clones go now: the folder the settings name, else the platform's. */
export function clonesGoTo(): string {
  return cloneLocation(app.settings.cloneDir, app.cloneDirDefault);
}

/** How the Clone dialog opens: on a way (a link, typed or not; the user's repositories), or for one repository (a pull request's). */
export type CloneStart = { way?: "link" | "mine"; link?: string; repo?: { host: string; owner: string; name: string } };

/** Opens the Clone dialog (once: a second ask while it is open does nothing). */
export function openClone(start: CloneStart = {}): void {
  if (overlays.dialogs.some((d) => d.component === CloneDialog)) return;
  showDialog(CloneDialog, { start });
}

/** Opens the + menu (once): open a folder, a recent repository, or clone. */
export function openNewTab(): void {
  if (overlays.dialogs.some((d) => d.component === NewTabMenu)) return;
  showDialog(NewTabMenu, {});
}

/**
 * Runs `then` once the + menu has closed and the focus is back where it was, so a dialog it
 * opens returns the focus there too.
 */
export async function afterMenu(close: () => void, then: () => void | Promise<void>): Promise<void> {
  close();
  await tick();
  await then();
}

/** Goes to a repository gg has: its tab, or opened from its folder (a recent one). The Pull requests page goes. */
export async function goToKnown(known: KnownRepo): Promise<void> {
  if (app.inboxOpen) closeInbox();
  if (known.repoId !== null && app.tabs.some((t) => t.repoId === known.repoId)) {
    activate(known.repoId);
    return;
  }
  await openPath(known.path);
}
