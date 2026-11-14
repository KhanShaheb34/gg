// What a context menu holds for a target, and what picking one of its items does: copy items
// and the commit, tag and branch actions work here; stash actions are wired in with their
// dialogs (Task 9). What an item acts on is taken from the target as it was when the menu
// opened (hashes and names, never row positions: the graph may have changed underneath since).

import CheckoutRemoteDialog from "../components/dialogs/CheckoutRemoteDialog.svelte";
import CherryPickDialog from "../components/dialogs/CherryPickDialog.svelte";
import CreateBranchDialog from "../components/dialogs/CreateBranchDialog.svelte";
import CreateTagDialog from "../components/dialogs/CreateTagDialog.svelte";
import DeleteBranchDialog from "../components/dialogs/DeleteBranchDialog.svelte";
import DeleteTagDialog from "../components/dialogs/DeleteTagDialog.svelte";
import FetchIntoLocalDialog from "../components/dialogs/FetchIntoLocalDialog.svelte";
import MergeDialog from "../components/dialogs/MergeDialog.svelte";
import PushDialog from "../components/dialogs/PushDialog.svelte";
import PushTagDialog from "../components/dialogs/PushTagDialog.svelte";
import RebaseDialog from "../components/dialogs/RebaseDialog.svelte";
import RenameBranchDialog from "../components/dialogs/RenameBranchDialog.svelte";
import ResetDialog from "../components/dialogs/ResetDialog.svelte";
import RevertDialog from "../components/dialogs/RevertDialog.svelte";
import { api } from "../ipc";
import { toast } from "../state/app.svelte";
import { runOp } from "../state/ops.svelte";
import { showDialog as show } from "../state/overlays.svelte";
import type { CommitRow, OpRequest, RefLabel, RepoSnapshot } from "../types";
import { branchMenu } from "./branchMenu";
import { checkOutBranch } from "./checkout";
import { remoteOf } from "./common";
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
  if (label?.kind === "branch" && pickBranch(id, label, snap)) return;
  if (label?.kind === "remote" && pickRemote(id, label.name, snap)) return;
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

/**
 * A double click on `label` of `row`: a local branch's label checks it out, as its menu's
 * "Check out" does. When that item cannot apply, the reason is said (nothing, for the
 * branch already checked out).
 */
export function checkOutLabel(label: RefLabel, row: CommitRow, snap: RepoSnapshot): void {
  if (label.kind !== "branch" || label.current) return;
  const checkout = itemsFor({ row, label }, snap).find((it) => "id" in it && it.id === "checkout");
  if (checkout && "disabledReason" in checkout && checkout.disabledReason) {
    toast("info", checkout.disabledReason);
    return;
  }
  pick("checkout", { row, label }, snap);
}

/** Runs `request` from a menu item; runOp reports how it went. */
function run(repoId: number, request: OpRequest): void {
  runOp(repoId, request).catch(() => {}); // reported by runOp
}

/** Commit item `id` on `row`; false if `id` is not one. */
function pickCommit(id: string, row: CommitRow, snap: RepoSnapshot): boolean {
  const repoId = snap.repoId;
  const { hash, subject, parents } = row;
  const branch = snap.head.branch;
  switch (id) {
    case "checkout":
      run(repoId, { op: "checkout_commit", hash });
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

/** Local branch item `id` on `label`; false if `id` is not one. */
function pickBranch(id: string, label: RefLabel, snap: RepoSnapshot): boolean {
  const repoId = snap.repoId;
  const branch = label.name;
  const into = snap.head.branch;
  switch (id) {
    case "checkout":
      checkOutBranch(repoId, branch, label);
      return true;
    case "rename":
      show(RenameBranchDialog, { repoId, branch, branches: snap.branches });
      return true;
    case "delete":
      show(DeleteBranchDialog, { repoId, branch, upstream: label.upstream });
      return true;
    case "merge":
      if (into !== null) show(MergeDialog, { repoId, target: { kind: "branch" as const, name: branch }, into });
      return true;
    case "rebase":
      if (into !== null) show(RebaseDialog, { repoId, onto: branch, branch: into });
      return true;
    case "push":
      show(PushDialog, { repoId, branch, upstream: label.upstream, remotes: snap.remotes });
      return true;
    case "pull":
      run(repoId, { op: "pull", branch, mode: "default" });
      return true;
    default:
      return false;
  }
}

/** Remote branch item `id` on remote branch `name` ("origin/x"); false if `id` is not one. */
function pickRemote(id: string, name: string, snap: RepoSnapshot): boolean {
  const repoId = snap.repoId;
  const { remotes, branches } = snap;
  switch (id) {
    case "checkout":
      show(CheckoutRemoteDialog, { repoId, remoteBranch: name, remotes, branches });
      return true;
    case "fetch_into_local":
      show(FetchIntoLocalDialog, { repoId, remoteBranch: name, remotes, branches });
      return true;
    case "merge":
      if (snap.head.branch !== null) {
        show(MergeDialog, { repoId, target: { kind: "remote" as const, name }, into: snap.head.branch });
      }
      return true;
    case "delete": {
      // The core asks to confirm, saying what would be lost: no dialog of its own.
      const remote = remoteOf(name, remotes);
      if (remote === null) toast("error", `No remote of this repository has ${name}`);
      else run(repoId, { op: "delete_remote_branch", remote, branch: name.slice(remote.length + 1) });
      return true;
    }
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
