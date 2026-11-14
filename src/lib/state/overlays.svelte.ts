// The context menu (at most one) and the modal dialogs: a stack, the top one taking input.
// A dialog opened while another is open (a confirmation the core asks for while a form is
// open) goes on top of it; the one below is kept as it is and takes input again once the
// top one closes. ContextMenu.svelte and the dialog host in App.svelte show what is here.

import type { Component } from "svelte";
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
};

export type OpenDialog = {
  id: number;
  // Any dialog component, with the props it was opened with.
  component: Component<any>;
  props: Record<string, unknown>;
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
 * keyboard (the menu key), so its first item is made active.
 */
export function openMenu(
  at: { x: number; y: number },
  items: MenuItem[],
  onPick: (id: string) => void,
  { keyboard = false }: { keyboard?: boolean } = {},
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
  overlays.dialogs = [...overlays.dialogs, { id, component, props }];
  return id;
}

/** Closes dialog `id`, wherever it is in the stack; nothing if it is closed already. */
export function closeDialog(id: number): void {
  if (overlays.dialogs.some((d) => d.id === id)) overlays.dialogs = overlays.dialogs.filter((d) => d.id !== id);
}
