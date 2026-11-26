// What the user writes to a pull request from the Pull request tab: a reply in a review thread,
// a comment on the conversation, a thread resolved or unresolved (pr_reply, pr_comment,
// pr_resolve, which the core runs through gh). The drafts are kept in memory only, per
// repository (in its side panel's state, PanelState.prCompose), pull request and thread, so
// they survive switching the side panel's tabs and selecting other commits; never saved.
//
// Nothing is ever sent but on the user's Send (or ⌘/Ctrl+Enter in the box). Sent, the core
// forgets the pull request's details and the tab loads them again from gh; the draft is
// cleared only once that is done, so the reply shows where the draft was. A failure keeps
// the draft and says why beside it.

import type { OpRequest } from "../types";
import { runOp } from "./ops.svelte";
import { loadDetails } from "./prDetails.svelte";
import type { Draft, PrCompose, Resolving } from "./prDrafts.svelte";

export type { Draft, PrCompose, Resolving } from "./prDrafts.svelte";

/** The message of error `e`, in one line. */
function messageOf(e: unknown): string {
  const text = e instanceof Error ? e.message : typeof e === "object" && e !== null && "message" in e ? String((e as { message: unknown }).message) : String(e);
  return text.split("\n")[0] || "It could not be sent.";
}

/** Where a write goes: the repository (its id and folder) and the pull request. */
export type PrTarget = { repoId: number; root: string; number: number };

/**
 * Runs `request` for `target`; once done, loads the pull request's details again (the core
 * forgot them: gh is asked at once). Resolves to null when done, else why not and the log's
 * first record of it.
 */
async function write(target: PrTarget, request: OpRequest): Promise<{ error: string; logFrom: number } | null> {
  try {
    const outcome = await runOp(target.repoId, request, { ownErrors: ["bad_request"], ownOutcome: true });
    if (outcome.status !== "done") return { error: outcome.message, logFrom: outcome.logFrom };
  } catch (e) {
    return { error: messageOf(e), logFrom: 0 };
  }
  await loadDetails(target.repoId, target.root, target.number);
  return null;
}

/**
 * Sends the draft of `thread` (null: a comment on the conversation) to `target`: what it holds
 * now, unless it is blank or being sent. The draft is cleared once the details show it; a
 * failure keeps it, with why.
 */
export async function sendDraft(compose: PrCompose, target: PrTarget, thread: string | null): Promise<void> {
  const d = compose.draft(target.number, thread);
  const body = d.text;
  if (d.sending || body.trim() === "") return;
  d.sending = true;
  d.error = null;
  d.logFrom = 0;
  const request: OpRequest =
    thread === null ? { op: "pr_comment", number: target.number, body } : { op: "pr_reply", number: target.number, threadId: thread, body };
  try {
    const failed = await write(target, request);
    if (failed) {
      d.error = failed.error;
      d.logFrom = failed.logFrom;
    } else if (d.text === body) {
      d.text = "";
    }
  } finally {
    d.sending = false;
  }
}

/** Resolves (or, `resolved` false, unresolves) `thread` of `target`; a failure is said beside it. */
export async function setResolved(compose: PrCompose, target: PrTarget, thread: string, resolved: boolean): Promise<void> {
  const r = compose.resolving(target.number, thread);
  if (r.pending) return;
  r.pending = true;
  r.error = null;
  r.logFrom = 0;
  try {
    const failed = await write(target, { op: "pr_resolve", number: target.number, threadId: thread, resolved });
    if (failed) {
      r.error = failed.error;
      r.logFrom = failed.logFrom;
    }
  } finally {
    r.pending = false;
  }
}

/** What the comments of one pull request can do, for the cards and the conversation's box. */
export type PrWrites = {
  /** The draft of `thread` (null: the conversation). */
  draft(thread: string | null): Draft;
  /** Sends it (see sendDraft). */
  send(thread: string | null): void;
  /** How resolving `thread` goes. */
  resolving(thread: string): Resolving;
  /** Resolves or unresolves `thread`. */
  setResolved(thread: string, resolved: boolean): void;
  /** Shows the command log from record `from`. */
  showLog(from: number): void;
};

/** The writes of `target`'s comments, with the drafts of `compose`. */
export function writesFor(compose: PrCompose, target: PrTarget, showLog: (from: number) => void): PrWrites {
  return {
    draft: (thread) => compose.draft(target.number, thread),
    send: (thread) => void sendDraft(compose, target, thread),
    resolving: (thread) => compose.resolving(target.number, thread),
    setResolved: (thread, resolved) => void setResolved(compose, target, thread, resolved),
    showLog,
  };
}
