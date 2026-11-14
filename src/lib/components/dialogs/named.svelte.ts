// What a dialog that names something (create branch or tag, rename, check out a remote
// branch as…) does with its request: it stays open until the core has answered, so that a
// name the core refuses shows under the field, the text kept, until the name is changed.
// While the request is out it cannot be cancelled (it would run all the same); when it waits
// behind other operations, the dialog says which one it waits for.

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
  /** Operations ahead of the request when it was sent (running and queued). */
  private ahead = 0;
  /** Operations seen running since it was sent. */
  private seen = new Set<number>();

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
   * The title of the operation the request waits behind, while it waits; else null. Each
   * operation seen running since the request went out is one of those ahead of it, until as
   * many as there were have run: the one running then is the request's own.
   */
  get waitingFor(): string | null {
    const op = this.running();
    if (!this.pending || op === null) return null;
    this.seen.add(op.id);
    return this.seen.size <= this.ahead ? op.title : null;
  }

  /** Sends `request`, made with `name`; closes the dialog unless the core refused the name. */
  async submit(name: string, request: OpRequest): Promise<void> {
    const op = this.running();
    this.ahead = op === null ? 0 : 1 + op.queued;
    this.seen = new Set(op === null ? [] : [op.id]);
    this.pending = true;
    const message = await runNamed(this.close, () => this.open, this.repoId, request);
    this.pending = false;
    if (message !== null) this.refused = { name, message };
  }

  private running(): RepoSnapshot["op"] {
    return findRepoView(this.repoId)?.snapshot.op ?? null;
  }
}
