// Context menus: what the builders (commitMenu, branchMenu, tagMenu, stashMenu) produce and
// what ContextMenu.svelte shows. Builders are pure functions of what the row, its label and
// the snapshot already hold, so a menu opens without asking the core anything.

import type { CommitRow, RefLabel } from "../types";

/**
 * One entry of a menu. `id` is stable (`"checkout"`, `"reset.hard"`…): tests and the
 * dispatcher go by it. An item with `disabledReason` is shown but cannot be picked; the
 * reason says why. `danger`: it destroys something (styled as such). `submenu`: picking it
 * opens these items.
 */
export type MenuItem =
  | { id: string; label: string; disabledReason?: string; danger?: boolean; submenu?: MenuItem[] }
  | { separator: true };

/** What a menu was opened on: a row (commit, stash, uncommitted changes), or a label of it. */
export type MenuTarget = { row: CommitRow; label: RefLabel | null };
