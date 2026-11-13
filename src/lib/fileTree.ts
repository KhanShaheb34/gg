// A commit's changed files as a folder tree, for the details panel.

import type { FileChange } from "./types";

/**
 * A folder (`file` null, with children) or a changed file (`file` set, no children). `path`
 * is the full path from the repository root; `name` the part shown, which for a folder
 * holding nothing but one folder takes that folder in too (`app/peaches`).
 */
export type TreeNode = { name: string; path: string; children: TreeNode[]; file: FileChange | null };

/** A node of a flattened tree with how deep it sits (0: at the root). */
export type TreeRow = { node: TreeNode; depth: number };

type Folder = { folders: Map<string, Folder>; files: FileChange[] };

const collator = new Intl.Collator("en", { numeric: true, sensitivity: "base" });

function byName(a: TreeNode, b: TreeNode): number {
  return collator.compare(a.name, b.name) || (a.name < b.name ? -1 : a.name > b.name ? 1 : 0);
}

/** Name after the last `/`. */
function baseName(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
}

/**
 * The tree of `files` (paths use `/`, as git writes them). Folders come before files, each
 * sorted by name; a folder holding only one folder is merged with it into one node.
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
  return [...folders.sort(byName), ...files.sort(byName)];
}

/** The rows of `roots` top to bottom, leaving out what is inside folders in `collapsed` (paths). */
export function flattenTree(roots: TreeNode[], collapsed: ReadonlySet<string>): TreeRow[] {
  const rows: TreeRow[] = [];
  const visit = (nodes: TreeNode[], depth: number) => {
    for (const node of nodes) {
      rows.push({ node, depth });
      if (node.file === null && !collapsed.has(node.path)) visit(node.children, depth + 1);
    }
  };
  visit(roots, 0);
  return rows;
}
