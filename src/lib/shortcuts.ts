// The app's keyboard shortcuts. Cmd is the command modifier on macOS, Ctrl elsewhere.

export type Shortcut =
  | { kind: "open" }
  | { kind: "close" }
  | { kind: "cycle"; delta: 1 | -1 }
  | { kind: "jump"; index: number };

type Keys = Pick<KeyboardEvent, "key" | "metaKey" | "ctrlKey" | "shiftKey" | "altKey">;

/** The shortcut `e` triggers, or null. */
export function shortcutFor(e: Keys, isMac: boolean): Shortcut | null {
  if (e.altKey) return null;
  // Ctrl+Tab / Ctrl+Shift+Tab cycle tabs on every platform.
  if (e.key === "Tab" && e.ctrlKey && !e.metaKey) return { kind: "cycle", delta: e.shiftKey ? -1 : 1 };

  const command = isMac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
  if (!command || e.shiftKey) return null;
  const key = e.key.toLowerCase();
  if (key === "o") return { kind: "open" };
  if (key === "w") return { kind: "close" };
  if (/^[1-9]$/.test(key)) return { kind: "jump", index: Number(key) - 1 };
  return null;
}
