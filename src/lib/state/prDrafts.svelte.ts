// The drafts the user writes to pull requests from the Pull request tab (replies in review
// threads, comments on the conversation) and how resolving each thread goes: one repository's,
// kept in its side panel's state (PanelState.prCompose) while the app runs, in memory only, so
// they survive switching the side panel's tabs and selecting other commits. Sending them is
// prCompose.svelte.ts's.

export type Draft = {
  /** What is typed. */
  text: string;
  /** It is being sent (and the details loaded again after). */
  sending: boolean;
  /** Why the last send failed, in one line; null when it did not. */
  error: string | null;
  /** The command log's first record of the failed send (0: none). */
  logFrom: number;
};

export type Resolving = {
  /** The thread is being resolved or unresolved. */
  pending: boolean;
  error: string | null;
  logFrom: number;
};

/** The thread a draft is for (null: the conversation), in pull request `number`. */
const keyOf = (number: number, thread: string | null) => `${number}\n${thread ?? ""}`;

/** One repository's drafts and the threads being resolved in it. */
export class PrCompose {
  // Plain maps of reactive entries: an entry is made where it is first read (in a card's
  // markup), and never replaced, so the maps themselves need not be followed.
  private drafts = new Map<string, Draft>();
  private resolves = new Map<string, Resolving>();

  /** The draft of `thread` (null: the conversation) of pull request `number`, made on first use. */
  draft(number: number, thread: string | null): Draft {
    const key = keyOf(number, thread);
    const found = this.drafts.get(key);
    if (found) return found;
    const made = $state<Draft>({ text: "", sending: false, error: null, logFrom: 0 });
    this.drafts.set(key, made);
    return made;
  }

  /** How resolving `thread` of pull request `number` goes, made on first use. */
  resolving(number: number, thread: string): Resolving {
    const key = keyOf(number, thread);
    const found = this.resolves.get(key);
    if (found) return found;
    const made = $state<Resolving>({ pending: false, error: null, logFrom: 0 });
    this.resolves.set(key, made);
    return made;
  }
}
