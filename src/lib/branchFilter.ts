// The branch filter's list and selection. A selection of null shows all branches.

export type FilterItem =
  | { kind: "all" }
  | { kind: "heading"; label: string }
  | { kind: "branch"; name: string; remote: boolean };

const ALL: FilterItem = { kind: "all" };

/**
 * The filter's rows: "Show all", then the local branches, then (when shown) the remote ones,
 * each under a heading, keeping only names that contain `text` (any case).
 */
export function branchItems(local: string[], remote: string[], showRemotes: boolean, text: string): FilterItem[] {
  const needle = text.trim().toLowerCase();
  const keep = (names: string[]) => (needle === "" ? names : names.filter((n) => n.toLowerCase().includes(needle)));
  const items: FilterItem[] = [ALL];
  const section = (label: string, names: string[], isRemote: boolean) => {
    if (names.length === 0) return;
    items.push({ kind: "heading", label });
    for (const name of names) items.push({ kind: "branch", name, remote: isRemote });
  };
  section("Local branches", keep(local), false);
  if (showRemotes) section("Remote branches", keep(remote), true);
  return items;
}

/** The selection with `name` added (or removed, if it was in it); empty means all. */
export function toggleBranch(selected: string[] | null, name: string): string[] | null {
  if (selected === null) return [name];
  const next = selected.includes(name) ? selected.filter((n) => n !== name) : [...selected, name];
  return next.length === 0 ? null : next;
}

/** The filter button's text for a selection. */
export function filterLabel(selected: string[] | null): string {
  const n = selected?.length ?? 0;
  if (n === 0) return "Show all";
  return n === 1 ? "1 branch" : `${n} branches`;
}
