// A commit's changed files as a folder tree or a sorted flat list, for the details panel.
// Both are built once per list of files (a commit's details, which the loader keeps) and
// reused when the commit is shown again or the view is switched.

import type { FileChange } from "./types";

/**
 * A folder (`file` null, with children) or a changed file (`file` set, no children). `path`
 * is the full path from the repository root; `name` the part shown, which for a folder
 * holding nothing but one folder takes that folder in too (`app/peaches`).
 */
export type TreeNode = { name: string; path: string; children: TreeNode[]; file: FileChange | null };

/**
 * A node of a flattened tree: how deep it sits (0: at the root), and its place among its
 * siblings (`pos` from 1 of `size`).
 */
export type TreeRow = { node: TreeNode; depth: number; pos: number; size: number };

type Folder = { folders: Map<string, Folder>; files: FileChange[] };

// --- Natural order ------------------------------------------------------------------------

/** The sort key of `name` for `compareNatural`: computed once per name, not per comparison. */
export function naturalKey(name: string): string {
  return name.toLowerCase();
}

const isDigit = (c: number) => c >= 48 && c <= 57;
const SLASH = 47;

/**
 * Compares two names by their keys (`naturalKey`): runs of digits by their value
 * (`file-8` before `file-16`), everything else by character, ignoring case; names whose keys
 * are alike by their exact text, so the order is total.
 *
 * For ASCII names this matches what `Intl.Collator(numeric, base)` gave, except that
 * punctuation sorts by code (`a-b`, `a.b` before digits; `a_b` after them) rather than all
 * before digits, and `/` before everything (so whole paths sort folder by folder). Other letters sort by code point after ASCII ones (`é` after `z`), and
 * accents are not ignored: cheap and stable, which matters with ten thousand files.
 */
export function compareNatural(a: string, b: string, rawA: string, rawB: string): number {
  const la = a.length;
  const lb = b.length;
  let i = 0;
  let j = 0;
  while (i < la && j < lb) {
    const ca = a.charCodeAt(i);
    const cb = b.charCodeAt(j);
    if (isDigit(ca) && isDigit(cb)) {
      let si = i;
      let sj = j;
      while (si < la && a.charCodeAt(si) === 48) si++;
      while (sj < lb && b.charCodeAt(sj) === 48) sj++;
      let ei = si;
      let ej = sj;
      while (ei < la && isDigit(a.charCodeAt(ei))) ei++;
      while (ej < lb && isDigit(b.charCodeAt(ej))) ej++;
      // More significant digits is a larger number; then digit by digit.
      if (ei - si !== ej - sj) return ei - si - (ej - sj);
      for (let k = 0; k < ei - si; k++) {
        const d = a.charCodeAt(si + k) - b.charCodeAt(sj + k);
        if (d !== 0) return d;
      }
      i = ei;
      j = ej;
      continue;
    }
    if (ca !== cb) {
      // A path's `/` sorts before every other character, so a folder's paths come right
      // after the folder (`docs/a` before `docs-site/b`), in the order the tree shows them.
      if (ca === SLASH) return -1;
      if (cb === SLASH) return 1;
      return ca - cb;
    }
    i++;
    j++;
  }
  if (i < la || j < lb) return la - i - (lb - j);
  return rawA < rawB ? -1 : rawA > rawB ? 1 : 0;
}

/** `items` sorted naturally by `name(item)`, each key computed once. */
function sortByName<T>(items: T[], name: (item: T) => string): T[] {
  const keyed = items.map((item) => ({ item, raw: name(item), key: naturalKey(name(item)) }));
  keyed.sort((x, y) => compareNatural(x.key, y.key, x.raw, y.raw));
  return keyed.map((k) => k.item);
}

// --- Tree ---------------------------------------------------------------------------------

/** Name after the last `/`. */
function baseName(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
}

/**
 * The tree of `files` (paths use `/`, as git writes them). Folders come before files, each
 * in natural order (`compareNatural`); a folder holding only one folder is merged with it
 * into one node.
 */
export function buildFileTree(files: FileChange[]): TreeNode[] {
  const root: Folder = { folders: new Map(), files: [] };
  for (const file of files) {
    const parts = file.path.split("/");
    let folder = root;
    for (let i = 0; i < parts.length - 1; i++) {
      let next = folder.folders.get(parts[i]);
      if (!next) folder.folders.set(parts[i], (next = { folders: new Map(), files: [] }));
      folder = next;
    }
    folder.files.push(file);
  }
  return nodesOf(root, "");
}

function nodesOf(folder: Folder, prefix: string): TreeNode[] {
  const folders: TreeNode[] = [];
  for (const [name, sub] of folder.folders) {
    // Follow a chain of folders that each hold only the next one.
    let label = name;
    let inner = sub;
    while (inner.files.length === 0 && inner.folders.size === 1) {
      const [nextName, next] = inner.folders.entries().next().value!;
      label += `/${nextName}`;
      inner = next;
    }
    const path = prefix + label;
    folders.push({ name: label, path, children: nodesOf(inner, `${path}/`), file: null });
  }
  const files: TreeNode[] = folder.files.map((file) => ({
    name: baseName(file.path),
    path: file.path,
    children: [],
    file,
  }));
  const byName = (n: TreeNode) => n.name;
  return [...sortByName(folders, byName), ...sortByName(files, byName)];
}

/** The rows of `roots` top to bottom, leaving out what is inside folders in `collapsed` (paths). */
export function flattenTree(roots: TreeNode[], collapsed: ReadonlySet<string>): TreeRow[] {
  const all = collapsed.size === 0;
  if (all) {
    const hit = flatCache.get(roots);
    if (hit) return hit;
  }
  const rows: TreeRow[] = [];
  const visit = (nodes: TreeNode[], depth: number) => {
    for (let i = 0; i < nodes.length; i++) {
      const node = nodes[i];
      rows.push({ node, depth, pos: i + 1, size: nodes.length });
      if (node.file === null && !collapsed.has(node.path)) visit(node.children, depth + 1);
    }
  };
  visit(roots, 0);
  if (all) flatCache.set(roots, rows);
  return rows;
}

// --- Built once per list of files ----------------------------------------------------------

const treeCache = new WeakMap<FileChange[], TreeNode[]>();
const flatCache = new WeakMap<TreeNode[], TreeRow[]>();
const sortedCache = new WeakMap<FileChange[], FileChange[]>();
const listCache = new WeakMap<FileChange[], TreeRow[]>();

/** The tree of `files` (`buildFileTree`), built on first use and kept with the list. */
export function fileTreeOf(files: FileChange[]): TreeNode[] {
  let tree = treeCache.get(files);
  if (!tree) treeCache.set(files, (tree = buildFileTree(files)));
  return tree;
}

/** `files` by whole path in natural order (a new list; `files` is left as it is), kept. */
export function sortedFilesOf(files: FileChange[]): FileChange[] {
  let sorted = sortedCache.get(files);
  if (!sorted) sortedCache.set(files, (sorted = sortByName(files, (f) => f.path)));
  return sorted;
}

/** The rows of the flat list of `files` (`sortedFilesOf`), kept. */
export function listRowsOf(files: FileChange[]): TreeRow[] {
  let rows = listCache.get(files);
  if (!rows) {
    const sorted = sortedFilesOf(files);
    rows = sorted.map((file, i) => ({
      node: { name: file.path, path: file.path, children: [], file },
      depth: 0,
      pos: i + 1,
      size: sorted.length,
    }));
    listCache.set(files, rows);
  }
  return rows;
}

const widthCache = new WeakMap<FileChange[], { add: number; del: number }>();

/**
 * How many characters the `+a` and `−d` counts of `files` need (sign included, at least 4),
 * so the bars between them line up down the list whatever the counts; kept with the list.
 */
export function countWidths(files: FileChange[]): { add: number; del: number } {
  let widths = widthCache.get(files);
  if (!widths) {
    let add = 0;
    let del = 0;
    for (const f of files) {
      add = Math.max(add, f.additions ?? 0);
      del = Math.max(del, f.deletions ?? 0);
    }
    widths = { add: Math.max(4, String(add).length + 1), del: Math.max(4, String(del).length + 1) };
    widthCache.set(files, widths);
  }
  return widths;
}
