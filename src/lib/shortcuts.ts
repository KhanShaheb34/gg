// The app's keyboard shortcuts, as one table: what each does, the keys for it, the group it
// is listed in on the Keyboard shortcuts sheet, and (for the window's own shortcuts) how a
// key press is matched. The window's handler (App.svelte) and the sheet both read this table,
// so the sheet cannot list a key that does nothing or miss one that does. Cmd is the command
// modifier on macOS, Ctrl elsewhere. Rows without a matcher are handled where they apply (the
// graph, the commit message, a panel) and are listed here so the sheet shows them too.

export type Shortcut =
  | { kind: "open" }
  | { kind: "close" }
  | { kind: "cycle"; delta: 1 | -1 }
  | { kind: "jump"; index: number }
  | { kind: "search" }
  | { kind: "fetch" }
  | { kind: "refresh" }
  | { kind: "regions"; delta: 1 | -1 }
  | { kind: "log" }
  | { kind: "sidePanel" }
  | { kind: "commit" }
  | { kind: "settings" }
  | { kind: "help" }
  | { kind: "worktree"; index: number };

/** Every kind of shortcut the window's handler takes (a test checks each has a row). */
export const SHORTCUT_KINDS = [
  "open",
  "close",
  "cycle",
  "jump",
  "search",
  "fetch",
  "refresh",
  "regions",
  "log",
  "sidePanel",
  "commit",
  "settings",
  "help",
  "worktree",
] as const satisfies readonly Shortcut["kind"][];

type Keys = Pick<KeyboardEvent, "key" | "metaKey" | "ctrlKey" | "shiftKey" | "altKey"> &
  Partial<Pick<KeyboardEvent, "isComposing" | "code">>;

/** What a matcher is told besides the keys. */
type Context = {
  /** The command modifier alone is down (Cmd on macOS, Ctrl elsewhere), with no Alt. */
  command: boolean;
  isMac: boolean;
  /** The focus is in a text field: plain keys are its own. */
  typing: boolean;
};

export type ShortcutGroup = "Repositories" | "Graph" | "Actions" | "Panels" | "Worktrees";
export const SHORTCUT_GROUPS: readonly ShortcutGroup[] = ["Repositories", "Graph", "Actions", "Panels", "Worktrees"];

/**
 * A row of the table. `keys`: the keys as the sheet shows them, one alternative per entry,
 * each a list of tokens: "Mod" (⌘ / Ctrl), "Shift", "Alt" (⌥ / Alt), "Ctrl" (Control on every
 * platform), or a key's name. `macKeys` replaces them on macOS where they differ. `match`:
 * the shortcut a key press triggers, for the window's own; absent for keys handled where they
 * apply (`where` says where).
 */
export type ShortcutRow = {
  group: ShortcutGroup;
  does: string;
  keys: string[][];
  macKeys?: string[][];
  where?: string;
  /** The kind of shortcut `match` gives (the window's own rows). */
  kind?: Shortcut["kind"];
  match?: (e: Keys, c: Context) => Shortcut | null;
};

const key = (e: Keys) => e.key.toLowerCase();
/** The digit of `e` (1..9), by the key's position, so Shift or Option giving another character does not matter. */
const digit = (e: Keys): number | null => {
  const m = /^Digit([1-9])$/.exec(e.code ?? "") ?? /^([1-9])$/.exec(e.key);
  return m ? Number(m[1]) : null;
};

export const SHORTCUT_TABLE: readonly ShortcutRow[] = [
  // --- Repositories ---------------------------------------------------------------------
  {
    group: "Repositories",
    does: "Open a repository",
    keys: [["Mod", "O"]],
    kind: "open",
    match: (e, c) => (c.command && !e.shiftKey && key(e) === "o" ? { kind: "open" } : null),
  },
  {
    group: "Repositories",
    does: "Close the tab (the window, with none open)",
    keys: [["Mod", "W"]],
    kind: "close",
    match: (e, c) => (c.command && !e.shiftKey && key(e) === "w" ? { kind: "close" } : null),
  },
  {
    group: "Repositories",
    does: "Next tab, previous tab",
    keys: [
      ["Ctrl", "Tab"],
      ["Ctrl", "Shift", "Tab"],
    ],
    // On every platform.
    kind: "cycle",
    match: (e) => (e.key === "Tab" && e.ctrlKey && !e.metaKey && !e.altKey ? { kind: "cycle", delta: e.shiftKey ? -1 : 1 } : null),
  },
  {
    group: "Repositories",
    does: "Go to tab 1 to 8; the last tab",
    keys: [
      ["Mod", "1…8"],
      ["Mod", "9"],
    ],
    kind: "jump",
    match: (e, c) => {
      if (!c.command || e.shiftKey || !/^[1-9]$/.test(e.key)) return null;
      return { kind: "jump", index: e.key === "9" ? -1 : Number(e.key) - 1 };
    },
  },
  {
    group: "Repositories",
    does: "Settings",
    keys: [["Mod", ","]],
    kind: "settings",
    match: (e, c) => (c.command && !e.shiftKey && e.key === "," ? { kind: "settings" } : null),
  },
  {
    group: "Repositories",
    does: "Keyboard shortcuts (this sheet)",
    // F1 and ⌘/ also while typing, and where ? needs AltGr (Task 10 review minor 13).
    keys: [["?"], ["F1"]],
    macKeys: [["?"], ["Mod", "/"]],
    where: "? when no text field has the focus",
    kind: "help",
    match: (e, c) => {
      const plain = !e.metaKey && !e.ctrlKey && !e.altKey;
      if (!c.typing && e.key === "?" && plain) return { kind: "help" };
      if (!c.isMac && e.key === "F1" && plain && !e.shiftKey) return { kind: "help" };
      if (c.isMac && c.command && e.key === "/") return { kind: "help" };
      return null;
    },
  },

  // --- Graph ----------------------------------------------------------------------------
  {
    group: "Graph",
    does: "Search commits",
    keys: [["Mod", "F"]],
    kind: "search",
    match: (e, c) => (c.command && !e.shiftKey && key(e) === "f" ? { kind: "search" } : null),
  },
  { group: "Graph", does: "Next match, previous match", keys: [["Enter"], ["Shift", "Enter"]], where: "in the search field" },
  { group: "Graph", does: "Select the next or previous commit", keys: [["↓"], ["↑"]], where: "in the graph" },
  { group: "Graph", does: "A page down or up", keys: [["Page Down"], ["Page Up"]], where: "in the graph" },
  { group: "Graph", does: "The first or last commit", keys: [["Home"], ["End"]], where: "in the graph" },
  { group: "Graph", does: "The selected commit's menu", keys: [["Shift", "F10"], ["Menu"]], where: "in the graph" },
  { group: "Graph", does: "Clear the selection; close the search", keys: [["Esc"]], where: "in the graph, in the search field" },

  // --- Actions --------------------------------------------------------------------------
  {
    group: "Actions",
    does: "Fetch from all remotes",
    keys: [["Mod", "Shift", "F"]],
    kind: "fetch",
    match: (e, c) => (c.command && e.shiftKey && key(e) === "f" ? { kind: "fetch" } : null),
  },
  {
    group: "Actions",
    does: "Refresh (and the pull requests)",
    keys: [["Mod", "R"]],
    kind: "refresh",
    match: (e, c) => (c.command && !e.shiftKey && key(e) === "r" ? { kind: "refresh" } : null),
  },
  {
    group: "Actions",
    does: "Commit: the commit panel, its message focused",
    keys: [["Mod", "K"]],
    kind: "commit",
    match: (e, c) => (c.command && !e.shiftKey && key(e) === "k" ? { kind: "commit" } : null),
  },
  { group: "Actions", does: "Commit what is ticked", keys: [["Mod", "Enter"]], where: "in the commit message" },
  { group: "Actions", does: "Tick or untick a file; tick all", keys: [["Space"], ["Mod", "A"]], where: "in the commit panel's list" },
  { group: "Actions", does: "Confirm a dialog; cancel it", keys: [["Enter"], ["Esc"]], where: "in a dialog" },

  // --- Panels ---------------------------------------------------------------------------
  {
    group: "Panels",
    does: "Show or hide the side panel (worktrees)",
    keys: [["Mod", "B"]],
    kind: "sidePanel",
    match: (e, c) => (c.command && !e.shiftKey && key(e) === "b" ? { kind: "sidePanel" } : null),
  },
  {
    group: "Panels",
    does: "Show or hide the command log",
    keys: [["Mod", "Shift", "L"]],
    kind: "log",
    match: (e, c) => (c.command && e.shiftKey && key(e) === "l" ? { kind: "log" } : null),
  },
  {
    group: "Panels",
    does: "Next region (graph, newest notice, side panel); the region before",
    keys: [["F6"], ["Shift", "F6"]],
    // On every platform.
    kind: "regions",
    match: (e) => (e.key === "F6" && !e.metaKey && !e.ctrlKey && !e.altKey ? { kind: "regions", delta: e.shiftKey ? -1 : 1 } : null),
  },
  { group: "Panels", does: "Close the details, the log, the commit panel", keys: [["Esc"]], where: "in that panel" },

  // --- Worktrees ------------------------------------------------------------------------
  {
    group: "Worktrees",
    does: "Make worktree 1 to 9 active, as the side panel lists them",
    // ⌘⇧3, 4 and 5 are macOS's own (screenshots): ⌘⌥ there.
    keys: [["Ctrl", "Shift", "1…9"]],
    macKeys: [["Mod", "Alt", "1…9"]],
    kind: "worktree",
    match: (e, c) => {
      const n = digit(e);
      if (n === null) return null;
      const keysDown = c.isMac
        ? e.metaKey && e.altKey && !e.ctrlKey && !e.shiftKey
        : e.ctrlKey && e.shiftKey && !e.metaKey && !e.altKey;
      return keysDown ? { kind: "worktree", index: n - 1 } : null;
    },
  },
];

/**
 * The shortcut `e` triggers, or null. Keys that are part of an input method's composition
 * trigger nothing; `typing` (the focus is in a text field) leaves plain keys to it.
 */
export function shortcutFor(e: Keys, isMac: boolean, typing = false): Shortcut | null {
  if (e.isComposing) return null;
  const command = (isMac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey) && !e.altKey;
  const context = { command, isMac, typing };
  for (const row of SHORTCUT_TABLE) {
    const found = row.match?.(e, context);
    if (found) return found;
  }
  return null;
}

/** Whether `shortcut` acts again while its keys are held down: only tab cycling does. */
export function repeatable(shortcut: Shortcut): boolean {
  return shortcut.kind === "cycle";
}

/** A key token as `isMac` shows it ("⌘", "⇧", "Ctrl"…). */
export function keyName(token: string, isMac: boolean): string {
  const mac: Record<string, string> = { Mod: "⌘", Shift: "⇧", Alt: "⌥", Ctrl: "⌃", Enter: "↩", Tab: "⇥", Esc: "Esc" };
  const other: Record<string, string> = { Mod: "Ctrl", Alt: "Alt", Ctrl: "Ctrl" };
  return (isMac ? mac[token] : other[token]) ?? token;
}

/** The table as the sheet shows it on `isMac`: its rows by group, each with its keys. */
export function shortcutSheet(isMac: boolean): { group: ShortcutGroup; rows: { does: string; keys: string[][]; where?: string }[] }[] {
  return SHORTCUT_GROUPS.map((group) => ({
    group,
    rows: SHORTCUT_TABLE.filter((r) => r.group === group).map((r) => ({
      does: r.does,
      keys: ((isMac && r.macKeys) || r.keys).map((alt) => alt.map((t) => keyName(t, isMac))),
      where: r.where,
    })),
  }));
}
