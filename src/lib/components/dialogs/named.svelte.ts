// What a dialog that names something (create branch or tag, rename, check out a remote
// branch as…) does with its request: it stays open until the core has answered, so that a
// name the core refuses shows under the field, the text kept, until the name is changed.

import { onDestroy } from "svelte";
import type { OpRequest } from "../../types";
import { runNamed } from "./run";

export class NamedSubmit {
  /** The request is out. */
  pending = $state(false);
  /** What the core said of the name last sent. */
  private refused: { name: string; message: string } | null = $state(null);
  private open = true;

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

  /** Sends `request`, made with `name`; closes the dialog unless the core refused the name. */
  async submit(name: string, request: OpRequest): Promise<void> {
    this.pending = true;
    const message = await runNamed(this.close, () => this.open, this.repoId, request);
    this.pending = false;
    if (message !== null) this.refused = { name, message };
  }
}
