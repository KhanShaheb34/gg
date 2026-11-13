// The colour theme: the `theme` setting resolved against the OS preference and applied as
// `data-theme` on <html>. public/theme-init.js applies the cached setting before first
// paint with the same rules; keep the two in step.

import type { Settings } from "./types";

export type ThemeSetting = Settings["theme"];
export type ResolvedTheme = "light" | "dark";

/** localStorage key under which the setting is cached for public/theme-init.js. */
export const THEME_CACHE_KEY = "gg-theme";

const ORDER: ThemeSetting[] = ["system", "light", "dark"];
const DARK_QUERY = "(prefers-color-scheme: dark)";

/** `setting` as a concrete theme; anything but light or dark follows the OS. */
export function resolveTheme(setting: string | null, systemDark: boolean): ResolvedTheme {
  if (setting === "light" || setting === "dark") return setting;
  return systemDark ? "dark" : "light";
}

/** The setting after `setting` in the toggle's cycle: system, light, dark. */
export function nextTheme(setting: ThemeSetting): ThemeSetting {
  return ORDER[(ORDER.indexOf(setting) + 1) % ORDER.length];
}

/** Sets `data-theme` on <html> from `setting` and caches the setting for the next start. */
export function applyTheme(setting: ThemeSetting): void {
  if (typeof document === "undefined") return;
  const dark = window.matchMedia?.(DARK_QUERY).matches ?? false;
  document.documentElement.dataset.theme = resolveTheme(setting, dark);
  try {
    localStorage.setItem(THEME_CACHE_KEY, setting);
  } catch {
    // Storage may be unavailable; the stored setting still applies once the core answers.
  }
}

/** Calls `onChange` whenever the OS switches between light and dark; returns a stop function. */
export function onSystemThemeChange(onChange: () => void): () => void {
  if (typeof window === "undefined" || !window.matchMedia) return () => {};
  const query = window.matchMedia(DARK_QUERY);
  query.addEventListener("change", onChange);
  return () => query.removeEventListener("change", onChange);
}
