// What a dialog that names something (create branch or tag, rename, check out a remote
// branch as…) does with its request: it stays open until the core has answered, so that a
// name the core refuses shows under the field, the text kept, until the name is changed.
// While the request is out it cannot be cancelled (it would run all the same); while the
// operation that was running when it was sent still runs, the dialog says it waits for it.

import { onDestroy } from "svelte";
import { findRepoView } from "../../state/repo.svelte";
import type { OpRequest, RepoSnapshot } from "../../types";
import { runNamed } from "./run";

export class NamedSubmit {
  /** The request is out. */
  pending = $state(false);
  /** What the core said of the name last sent. */
  private refused: { name: string; message: string } | null = $state(null);
  private open = true;
  /** The operation running when the request was sent, if any: the one it waits behind. */
  private behind: { id: number; title: string } | null = $state(null);

  /** Made while the dialog is set up: `close` closes it. */
  constructor(
    private readonly repoId: number,
    private readonly close: () => void,
  ) {
    onDestroy(() => (this.open = false));
  }

  /** Why `name` cannot be used: what the core said of it, while it is the name sent; else `own`. */
  problem(name: string, own: string | null): string | null {
    return this.refused !== null && this.refused.name === name ? this.refused.message : own;
  }

  /**
   * The title of the operation the request waits behind, while that one runs: the one that
   * was running when the request was sent. Any other running since may be the request's own,
   * so it is not named (the toolbar's busy pill counts the rest).
   */
  get waitingFor(): string | null {
    const op = this.running();
    return this.pending && op !== null && this.behind !== null && op.id === this.behind.id ? op.title : null;
  }

  /** Sends `request`, made with `name`; closes the dialog unless the core refused the name. */
  async submit(name: string, request: OpRequest): Promise<void> {
    const op = this.running();
    this.behind = op === null ? null : { id: op.id, title: op.title };
    this.pending = true;
    const message = await runNamed(this.close, () => this.open, this.repoId, request);
    this.pending = false;
    if (message !== null) this.refused = { name, message };
  }

  private running(): RepoSnapshot["op"] {
    return findRepoView(this.repoId)?.snapshot.op ?? null;
  }
}
