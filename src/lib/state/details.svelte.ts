// The details of the selected commit, for the details panel: asked of the core as the
// selection moves, the newest selection's answer only ever shown, recent answers remembered.

import { api } from "../ipc";
import type { CommitDetails, CommitRow } from "../types";

/** Commits whose details are remembered per repository, at most. */
export const DETAILS_CACHE_SIZE = 50;
/**
 * Changed files remembered per repository, at most, across all remembered commits (about
 * 10 MB): the least recently shown go first. The newest is kept even when it alone has more.
 */
export const DETAILS_CACHE_FILES = 60_000;
/**
 * A selection made this soon after the previous one (arrow keys held down, say) waits this
 * long for the next before the core is asked: only where the selection stops is loaded.
 */
export const DETAILS_DEBOUNCE_MS = 80;

/** A full commit name (40 or 64 hex digits); the uncommitted row's is not one. */
export function isCommitHash(hash: string | null): hash is string {
  return hash !== null && (hash.length === 40 || hash.length === 64) && /^[0-9a-f]+$/i.test(hash);
}

export class CommitDetailsLoader {
  private readonly repoId: number;
  /** The commit shown: the selection, when it is a commit. */
  hash: string | null = $state(null);
  /** The graph's row of `hash`, when known: shown while the details load. */
  row: CommitRow | null = $state.raw(null);
  /** The details of `hash`, once loaded. */
  details: CommitDetails | null = $state.raw(null);
  /** Why the details of `hash` could not be loaded. */
  error: string | null = $state(null);

  /** Answers by hash, least recently shown first. */
  private cache = new Map<string, CommitDetails>();
  /** Changed files of all remembered commits. */
  private cachedFiles = 0;
  private inFlight = new Set<string>();
  private timer: ReturnType<typeof setTimeout> | null = null;
  private lastShown = -Infinity;
  private disposed = false;

  constructor(repoId: number) {
    this.repoId = repoId;
  }

  /** The details of `hash` are on their way. */
  get loading(): boolean {
    return this.hash !== null && this.details === null && this.error === null;
  }

  /**
   * Shows commit `hash` (null, or the uncommitted row: nothing). `row` is its row in the
   * graph, shown until the details arrive.
   */
  show(hash: string | null, row?: CommitRow | null): void {
    if (!isCommitHash(hash)) hash = null;
    if (hash === this.hash) {
      if (row && row.hash === hash && this.row?.hash !== hash) this.row = row;
      return;
    }
    const now = performance.now();
    const rapid = now - this.lastShown < DETAILS_DEBOUNCE_MS;
    this.lastShown = now;
    this.cancelTimer();
    this.hash = hash;
    this.row = row && row.hash === hash ? row : null;
    this.error = null;
    this.details = hash === null ? null : this.remembered(hash);
    if (hash === null || this.details !== null) return;
    if (rapid) this.timer = setTimeout(() => this.load(hash), DETAILS_DEBOUNCE_MS);
    else this.load(hash);
  }

  /** Asks again for the details that could not be loaded. */
  retry(): void {
    if (this.hash === null || this.error === null) return;
    this.error = null;
    this.load(this.hash);
  }

  dispose(): void {
    this.disposed = true;
    this.cancelTimer();
    this.cache.clear();
    this.cachedFiles = 0;
  }

  private cancelTimer(): void {
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = null;
  }

  /** The remembered details of `hash`, now the most recently shown; or null. */
  private remembered(hash: string): CommitDetails | null {
    const hit = this.cache.get(hash);
    if (!hit) return null;
    this.cache.delete(hash);
    this.cache.set(hash, hit);
    return hit;
  }

  private remember(hash: string, details: CommitDetails): void {
    this.forget(hash);
    this.cache.set(hash, details);
    this.cachedFiles += details.files.length;
    while (this.cache.size > 1 && (this.cache.size > DETAILS_CACHE_SIZE || this.cachedFiles > DETAILS_CACHE_FILES)) {
      this.forget(this.cache.keys().next().value!);
    }
  }

  private forget(hash: string): void {
    const old = this.cache.get(hash);
    if (!old) return;
    this.cache.delete(hash);
    this.cachedFiles -= old.files.length;
  }

  private async load(hash: string): Promise<void> {
    this.timer = null;
    // An answer already on its way is shown when it comes, if `hash` is still wanted then.
    if (this.disposed || this.inFlight.has(hash)) return;
    this.inFlight.add(hash);
    try {
      const details = await api.commitDetails(this.repoId, hash);
      if (this.disposed) return;
      this.remember(hash, details);
      if (this.hash === hash) this.details = details;
    } catch (e) {
      if (!this.disposed && this.hash === hash) this.error = e instanceof Error ? e.message : String(e);
    } finally {
      this.inFlight.delete(hash);
    }
  }
}
