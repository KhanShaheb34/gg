// The app's keyboard shortcuts. Cmd is the command modifier on macOS, Ctrl elsewhere.
//
//   Cmd+O            open a repository        Cmd+W            close the tab
//   Ctrl+Tab         next tab (Shift: back)   Cmd+1..8, Cmd+9  a tab, the last tab
//   Cmd+F            search commits           Cmd+Shift+F      fetch
//   Cmd+R            refresh                  F6               the newest notice (its first button)

export type Shortcut =
  | { kind: "open" }
  | { kind: "close" }
  | { kind: "cycle"; delta: 1 | -1 }
  | { kind: "jump"; index: number }
  | { kind: "search" }
  | { kind: "fetch" }
  | { kind: "refresh" }
  | { kind: "notices" };

type Keys = Pick<KeyboardEvent, "key" | "metaKey" | "ctrlKey" | "shiftKey" | "altKey"> &
  Partial<Pick<KeyboardEvent, "isComposing">>;

/**
 * The shortcut `e` triggers, or null. Cmd+1..8 are tabs 1 to 8 and Cmd+9 is the last tab
 * (`jump` index -1), as in browsers. Keys that are part of an input method's composition
 * trigger nothing.
 */
export function shortcutFor(e: Keys, isMac: boolean): Shortcut | null {
  if (e.altKey || e.isComposing) return null;
  if (e.key === "F6" && !e.metaKey && !e.ctrlKey && !e.shiftKey) return { kind: "notices" };
  // Ctrl+Tab / Ctrl+Shift+Tab cycle tabs on every platform.
  if (e.key === "Tab" && e.ctrlKey && !e.metaKey) return { kind: "cycle", delta: e.shiftKey ? -1 : 1 };

  const command = isMac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
  if (!command) return null;
  const key = e.key.toLowerCase();
  if (e.shiftKey) return key === "f" ? { kind: "fetch" } : null;
  if (key === "f") return { kind: "search" };
  if (key === "r") return { kind: "refresh" };
  if (key === "o") return { kind: "open" };
  if (key === "w") return { kind: "close" };
  if (key === "9") return { kind: "jump", index: -1 };
  if (/^[1-8]$/.test(key)) return { kind: "jump", index: Number(key) - 1 };
  return null;
}

/** Whether `shortcut` acts again while its keys are held down: only tab cycling does. */
export function repeatable(shortcut: Shortcut): boolean {
  return shortcut.kind === "cycle";
}
