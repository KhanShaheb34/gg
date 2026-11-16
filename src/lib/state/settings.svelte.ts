// The settings screen's state: opening it, saving each field as it changes (text after a
// pause), saying "Saved" quietly or the core's own refusal inline, and putting everything back
// to the defaults. The core checks every value (store.rs); what it refuses is never applied,
// and the text typed stays in its field.

import SettingsSheet from "../components/Settings.svelte";
import ShortcutsSheet from "../components/ShortcutsSheet.svelte";
import { api, CoreError } from "../ipc";
import { applyTheme } from "../theme";
import type { Settings } from "../types";
import { app } from "./app.svelte";
import { overlays, showDialog } from "./overlays.svelte";

/** Shows the settings (once: a second ask while it is open does nothing). */
export function openSettings(): void {
  if (overlays.dialogs.some((d) => d.component === SettingsSheet)) return;
  showDialog(SettingsSheet, {});
}

/** Shows the keyboard shortcuts sheet (once). */
export function openShortcuts(): void {
  if (overlays.dialogs.some((d) => d.component === ShortcutsSheet)) return;
  showDialog(ShortcutsSheet, {});
}

/** How long "Saved" stays beside a field, ms. */
export const SAVED_FOR_MS = 1600;
/** How long a text field waits after the last key before it is saved, ms. */
export const TEXT_DELAY_MS = 500;

export type SaveStatus = "idle" | "saving" | "saved" | "error";

/**
 * Why the settings file could not be written by the last save that tried (null: the last save
 * worked): said once, at the top of the sheet, not as a fault of the field changed.
 */
export const saving = $state({ problem: null as string | null });

/**
 * One setting as the screen edits it: `set` saves a new value (after `delayMs`, the last one
 * wins), `flush` saves a pending one now. `status` and `error` say how the last save went;
 * `error` is the core's own message, and the value it refused is not applied.
 */
export class SettingField<K extends keyof Settings> {
  status: SaveStatus = $state("idle");
  error: string | null = $state(null);
  #pending: { value: Settings[K] } | null = null;
  #timer: ReturnType<typeof setTimeout> | null = null;
  #savedTimer: ReturnType<typeof setTimeout> | null = null;
  #seq = 0;

  constructor(
    readonly key: K,
    readonly delayMs = 0,
  ) {}

  /** Saves `value` (after the field's pause). */
  set(value: Settings[K]): void {
    this.#pending = { value };
    if (this.#timer !== null) clearTimeout(this.#timer);
    if (this.delayMs === 0) {
      void this.flush();
      return;
    }
    this.#timer = setTimeout(() => void this.flush(), this.delayMs);
  }

  /** Whether a value waits to be saved. */
  get pending(): boolean {
    return this.#pending !== null;
  }

  /** Saves the waiting value now; whether the field is saved (nothing waiting counts). */
  async flush(): Promise<boolean> {
    if (this.#timer !== null) clearTimeout(this.#timer);
    this.#timer = null;
    const pending = this.#pending;
    if (pending === null) return this.status !== "error";
    this.#pending = null;
    const seq = ++this.#seq;
    this.status = "saving";
    try {
      const saved = await api.updateSettings({ [this.key]: pending.value } as Partial<Settings>);
      if (seq !== this.#seq) return true; // a later value is on its way
      app.settings = saved;
      this.error = null;
      this.status = "saved";
      saving.problem = null;
      if (this.#savedTimer !== null) clearTimeout(this.#savedTimer);
      this.#savedTimer = setTimeout(() => {
        if (this.status === "saved") this.status = "idle";
      }, SAVED_FOR_MS);
      return true;
    } catch (e) {
      if (seq !== this.#seq) return false;
      this.status = "error";
      // The file, not the value (the core refuses values as bad requests): said once, at the
      // top of the sheet, not again beside the field.
      if (e instanceof CoreError && e.code === "io") {
        saving.problem = e.detail ? `${e.message}: ${e.detail}` : e.message;
        this.error = null;
      } else {
        this.error = e instanceof CoreError || e instanceof Error ? e.message : String(e);
      }
      return false;
    }
  }

  /** Forgets a waiting value and any message (the settings were reset). */
  reset(): void {
    if (this.#timer !== null) clearTimeout(this.#timer);
    this.#timer = null;
    this.#pending = null;
    this.#seq++;
    this.status = "idle";
    this.error = null;
  }
}

/** Puts every setting back to its default; the settings after (thrown: the core's error). */
export async function restoreDefaults(): Promise<Settings> {
  const settings = await api.restoreDefaultSettings();
  app.settings = settings;
  applyTheme(settings.theme);
  return settings;
}

/**
 * The words of user command `command` as the core splits it (on whitespace; text in single or
 * double quotes is taken literally), with `path` where a word is `{path}`, else added last:
 * what is run. Null when a quote is not closed (the core refuses it).
 */
export function commandWords(command: string, path: string): string[] | null {
  const words: string[] = [];
  let word = "";
  let inWord = false;
  let quote: string | null = null;
  for (const c of command) {
    if (quote !== null) {
      if (c === quote) quote = null;
      else word += c;
    } else if (c === "'" || c === '"') {
      quote = c;
      inWord = true;
    } else if (/\s/.test(c)) {
      if (inWord) words.push(word);
      word = "";
      inWord = false;
    } else {
      word += c;
      inWord = true;
    }
  }
  if (quote !== null) return null;
  if (inWord) words.push(word);
  if (words.length === 0) return [];
  return words.includes("{path}") ? words.map((w) => (w === "{path}" ? path : w)) : [...words, path];
}

/** The fetch intervals offered, seconds (0: off). */
export const FETCH_CHOICES = [0, 60, 300, 900, 3600] as const;

/** A fetch interval for people: "Off", "1 min", "1 hour", "45 s". */
export function intervalName(secs: number): string {
  if (secs === 0) return "Off";
  if (secs % 3600 === 0) return secs === 3600 ? "1 hour" : `${secs / 3600} hours`;
  if (secs % 60 === 0) return `${secs / 60} min`;
  return `${secs} s`;
}
