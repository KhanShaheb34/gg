// What a context menu holds for a target, and what picking one of its items does. Copy items
// work here; the other actions are wired in as their dialogs are built (Tasks 7–9).

import { api } from "../ipc";
import { toast } from "../state/app.svelte";
import type { RepoSnapshot } from "../types";
import { branchMenu } from "./branchMenu";
import { commitMenu } from "./commitMenu";
import { stashMenu, uncommittedMenu } from "./stashMenu";
import { tagMenu } from "./tagMenu";
import type { MenuItem, MenuTarget } from "./types";

/** The menu for `target`: its label's, or its row's. */
export function itemsFor(target: MenuTarget, snap: RepoSnapshot): MenuItem[] {
  const { row, label } = target;
  if (label?.kind === "branch" || label?.kind === "remote") return branchMenu(label, row, snap);
  if (label?.kind === "tag") return tagMenu(label, snap);
  if (row.kind === "stash" || label?.kind === "stash") return stashMenu(row, snap);
  if (row.kind === "uncommitted") return uncommittedMenu(snap);
  return commitMenu(row, snap);
}

const KIND_NAMES = { branch: "branch name", remote: "branch name", tag: "tag name", stash: "stash name" } as const;

/** Does what item `id` of `target`'s menu stands for. */
export function pick(id: string, target: MenuTarget, snap: RepoSnapshot): void {
  const { row, label } = target;
  switch (id) {
    case "copy.hash":
      void copy(row.hash, "hash");
      return;
    case "copy.name":
      if (label) void copy(label.name, KIND_NAMES[label.kind]);
      return;
    case "copy.message":
      void copyMessage(snap.repoId, row.hash);
      return;
    default:
      toast("info", "This action is not available yet");
  }
}

async function copy(text: string, what: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    toast("error", `Could not copy the ${what}`);
    return;
  }
  toast("info", `Copied the ${what}`);
}

/** Copies the commit's whole message (the row has only its subject). */
async function copyMessage(repoId: number, hash: string): Promise<void> {
  let message: string;
  try {
    message = (await api.commitDetails(repoId, hash)).message;
  } catch (e) {
    toast("error", `Could not copy the message: ${e instanceof Error ? e.message : String(e)}`);
    return;
  }
  await copy(message, "message");
}
