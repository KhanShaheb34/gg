// What a context menu holds for a target, and what picking one of its items does: copy items
// and the commit and tag actions work here; branch and stash actions are wired in as their
// dialogs are built (Tasks 8–9). What an item acts on is taken from the target as it was
// when the menu opened (hashes and names, never row positions: the graph may have changed
// underneath since).

import type { Component } from "svelte";
import CherryPickDialog from "../components/dialogs/CherryPickDialog.svelte";
import CreateBranchDialog from "../components/dialogs/CreateBranchDialog.svelte";
import CreateTagDialog from "../components/dialogs/CreateTagDialog.svelte";
import DeleteTagDialog from "../components/dialogs/DeleteTagDialog.svelte";
import PushTagDialog from "../components/dialogs/PushTagDialog.svelte";
import ResetDialog from "../components/dialogs/ResetDialog.svelte";
import RevertDialog from "../components/dialogs/RevertDialog.svelte";
import { api } from "../ipc";
import { toast } from "../state/app.svelte";
import { runOp } from "../state/ops.svelte";
import { closeDialog, openDialog } from "../state/overlays.svelte";
import type { CommitRow, RepoSnapshot } from "../types";
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
  // Labels share some ids with commits ("checkout", "push", "delete"): theirs go first.
  if (label?.kind === "tag" && pickTag(id, label.name, snap)) return;
  if (label === null && row.kind === "commit" && pickCommit(id, row, snap)) return;
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

/** Opens `component` as a dialog with `props` and a `close` that closes it. */
function show<P extends Record<string, unknown>>(
  component: Component<P & { close: () => void }>,
  props: P,
): void {
  const id = openDialog(component, { ...props, close: () => closeDialog(id) });
}

/** Commit item `id` on `row`; false if `id` is not one. */
function pickCommit(id: string, row: CommitRow, snap: RepoSnapshot): boolean {
  const repoId = snap.repoId;
  const { hash, subject, parents } = row;
  const branch = snap.head.branch;
  switch (id) {
    case "checkout":
      runOp(repoId, { op: "checkout_commit", hash }).catch(() => {}); // reported by runOp
      return true;
    case "branch.create":
      show(CreateBranchDialog, { repoId, hash, subject, branches: snap.branches });
      return true;
    case "tag.create":
      show(CreateTagDialog, { repoId, hash, subject });
      return true;
    case "cherry_pick":
      show(CherryPickDialog, { repoId, hash, subject, parents, branch });
      return true;
    case "revert":
      show(RevertDialog, { repoId, hash, subject, parents, branch });
      return true;
    case "reset.soft":
    case "reset.mixed":
    case "reset.hard":
      show(ResetDialog, { repoId, hash, subject, branch, mode: id.slice("reset.".length) as "soft" | "mixed" | "hard" });
      return true;
    default:
      return false;
  }
}

/** Tag item `id` on tag `name`; false if `id` is not one. */
function pickTag(id: string, name: string, snap: RepoSnapshot): boolean {
  const repoId = snap.repoId;
  switch (id) {
    case "push":
      show(PushTagDialog, { repoId, name, remotes: snap.remotes });
      return true;
    case "delete":
      show(DeleteTagDialog, { repoId, name, remotes: snap.remotes });
      return true;
    default:
      return false;
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
