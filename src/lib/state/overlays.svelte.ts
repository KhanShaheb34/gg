// The context menu (at most one) and the modal dialogs: a stack, the top one taking input.
// A dialog opened while another is open (a confirmation the core asks for while a form is
// open) goes on top of it; the one below is kept as it is and takes input again once the
// top one closes. ContextMenu.svelte and the dialog host in App.svelte show what is here.

import { type Component, tick } from "svelte";
import type { MenuItem } from "../menus/types";

export type OpenMenu = {
  /** Changes with every menu opened, so a new one starts afresh. */
  id: number;
  x: number;
  y: number;
  items: MenuItem[];
  onPick: (id: string) => void;
  /** Where focus goes back to when the menu closes (the row's grid). */
  returnFocus: HTMLElement | null;
  /** Opened from the keyboard: its first item is active at once. */
  keyboard: boolean;
  /** The repository its items act on: the menu closes when an operation starts or ends there. */
  repoId: number | null;
  /**
   * Whether what the menu acts on (its rows and labels) is still there, as far as known;
   * asked when the repository's rows change. Without it, the menu stays open.
   */
  alive: (() => Promise<boolean>) | null;
};

export type OpenDialog = {
  id: number;
  // Any dialog component, with the props it was opened with.
  component: Component<any>;
  props: Record<string, unknown>;
  /** What had the focus when it was opened (a button of the dialog under it, say). */
  opener: HTMLElement | null;
};

class Overlays {
  menu: OpenMenu | null = $state.raw(null);
  /** Open dialogs, bottom first. */
  dialogs: OpenDialog[] = $state.raw([]);

  /** The top dialog, the one taking input, or null. */
  get dialog(): OpenDialog | null {
    return this.dialogs.at(-1) ?? null;
  }
}

export const overlays = new Overlays();

let nextId = 1;

/**
 * Opens a menu of `items` at the pointer (`at`, viewport px; it is kept inside the window).
 * Picking an item closes the menu, puts focus back where it was, then calls `onPick` with
 * the item's id. Opening another menu replaces this one. `keyboard`: opened from the
 * keyboard (the menu key), so its first item is made active. `repoId`: the repository its
 * items act on; the menu closes when an operation starts or ends there, or when its rows
 * change and `alive` then says its target is gone. A background refresh alone (a scheduled
 * fetch, the working tree's status) leaves it open (R26).
 */
export function openMenu(
  at: { x: number; y: number },
  items: MenuItem[],
  onPick: (id: string) => void,
  {
    keyboard = false,
    repoId = null,
    alive = null,
  }: { keyboard?: boolean; repoId?: number | null; alive?: (() => Promise<boolean>) | null } = {},
): void {
  const focused = document.activeElement;
  overlays.menu = {
    id: nextId++,
    x: at.x,
    y: at.y,
    items,
    onPick,
    returnFocus: focused instanceof HTMLElement && focused !== document.body ? focused : null,
    keyboard,
    repoId,
    alive,
  };
}

/** Closes the menu; with `restoreFocus`, focus goes back to where it was when it opened. */
export function closeMenu(restoreFocus = true): void {
  const menu = overlays.menu;
  if (!menu) return;
  overlays.menu = null;
  if (restoreFocus && menu.returnFocus?.isConnected) menu.returnFocus.focus({ preventScroll: true });
}

/**
 * Shows `component` with `props` as a modal dialog, on top of any open one (which stays as it
 * is). Returns its id. The dialog says how it ends through its props' callbacks; whoever
 * opened it closes it with `closeDialog(id)`.
 */
export function openDialog<P extends Record<string, unknown>>(component: Component<P>, props: P): number {
  closeMenu(false);
  const id = nextId++;
  // Taken now: once the dialog is shown, what is under it is inert and has lost the focus.
  const focused = typeof document === "undefined" ? null : document.activeElement;
  const opener =
    typeof HTMLElement !== "undefined" && focused instanceof HTMLElement && focused !== document.body ? focused : null;
  overlays.dialogs = [...overlays.dialogs, { id, component, props, opener }];
  return id;
}

/**
 * Closes dialog `id`, wherever it is in the stack; nothing if it is closed already. When it
 * was the top one, the focus goes back to what had it when it opened (once what is under it
 * takes input again), unless something else has taken it meanwhile.
 */
export function closeDialog(id: number): void {
  const closing = overlays.dialogs.find((d) => d.id === id);
  if (!closing) return;
  const wasTop = closing === overlays.dialog;
  overlays.dialogs = overlays.dialogs.filter((d) => d.id !== id);
  const opener = closing.opener;
  if (!wasTop || !opener) return;
  void tick().then(() => {
    const now = document.activeElement;
    const free = now === null || now === document.body;
    if (free && opener.isConnected && !opener.closest("[inert]")) opener.focus({ preventScroll: true });
  });
}

/** Opens `component` as a dialog with `props` and a `close` that closes it. */
export function showDialog<P extends Record<string, unknown>>(
  component: Component<P & { close: () => void }>,
  props: P,
): void {
  const id = openDialog(component, { ...props, close: () => closeDialog(id) } as P & { close: () => void });
}

/**
 * Runs `action`, asked for from the app's native menu (Settings…, Keyboard Shortcuts, Install
 * ‘gg’ Command…), unless a dialog is open: a dialog is modal, for the menu as for the keys.
 */
export function fromAppMenu(action: () => void): void {
  if (overlays.dialog) return;
  action();
}
