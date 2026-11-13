import { describe, expect, it } from "vitest";
import {
  buildFileTree,
  compareNatural,
  fileTreeOf,
  flattenTree,
  naturalKey,
  sortedFilesOf,
  type TreeNode,
} from "./fileTree";
import type { FileChange } from "./types";

function file(path: string, status: FileChange["status"] = "M", oldPath: string | null = null): FileChange {
  return { path, oldPath, status, additions: 1, deletions: 0 };
}

/** The tree as nested names: folders as `[name, children]`, files as their name. */
function shape(nodes: TreeNode[]): unknown[] {
  return nodes.map((n) => (n.file ? n.name : [n.name, shape(n.children)]));
}

describe("buildFileTree", () => {
  it("groups files under their folders", () => {
    const roots = buildFileTree(
      ["app/peaches/a.py", "app/peaches/b.py", "app/routes/r.py", "docs/A.md"].map((p) => file(p)),
    );
    expect(shape(roots)).toEqual([
      [
        "app",
        [
          ["peaches", ["a.py", "b.py"]],
          ["routes", ["r.py"]],
        ],
      ],
      ["docs", ["A.md"]],
    ]);
    expect(roots[0].path).toBe("app");
    expect(roots[0].children[0].path).toBe("app/peaches");
    expect(roots[0].children[0].children[1].path).toBe("app/peaches/b.py");
  });

  it("collapses a chain of single folders into one node", () => {
    const roots = buildFileTree([file("a/b/c/x.txt")]);
    expect(roots).toHaveLength(1);
    expect(roots[0].name).toBe("a/b/c");
    expect(roots[0].path).toBe("a/b/c");
    expect(roots[0].file).toBeNull();
    expect(shape(roots[0].children)).toEqual(["x.txt"]);
  });

  it("collapses only while a folder holds nothing but one folder", () => {
    const roots = buildFileTree([file("a/b/x.txt"), file("a/b/c/y.txt"), file("a/z.txt")]);
    expect(shape(roots)).toEqual([["a", [["b", [["c", ["y.txt"]], "x.txt"]], "z.txt"]]]);
    const nested = buildFileTree([file("src/lib/deep/one/a.ts"), file("src/lib/deep/two/b.ts")]);
    expect(shape(nested)).toEqual([["src/lib/deep", [["one", ["a.ts"]], ["two", ["b.ts"]]]]]);
    expect(nested[0].children[1].path).toBe("src/lib/deep/two");
  });

  it("puts folders before files, then sorts by name", () => {
    const roots = buildFileTree(
      ["zeta.txt", "b/x.txt", "Alpha.md", "a/y.txt", "b/a/q.txt", "b/c.txt"].map((p) => file(p)),
    );
    expect(shape(roots)).toEqual([["a", ["y.txt"]], ["b", [["a", ["q.txt"]], "c.txt", "x.txt"]], "Alpha.md", "zeta.txt"]);
  });

  it("keeps each file's change, a renamed file's old path included", () => {
    const renamed = file("docs/new-name.txt", "R", "notes/old-name.txt");
    const roots = buildFileTree([renamed, file("docs/A.md", "D")]);
    const docs = roots[0];
    expect(docs.children.map((n) => n.name)).toEqual(["A.md", "new-name.txt"]);
    expect(docs.children[1].file).toBe(renamed);
    expect(docs.children[1].file?.oldPath).toBe("notes/old-name.txt");
    expect(docs.children[0].file?.status).toBe("D");
  });

  it("allows a file and a folder of the same name (a file replaced by a folder)", () => {
    const roots = buildFileTree([file("a", "D"), file("a/b.txt", "A")]);
    expect(shape(roots)).toEqual([["a", ["b.txt"]], "a"]);
  });

  it("is empty for no files", () => {
    expect(buildFileTree([])).toEqual([]);
  });
});

describe("flattenTree", () => {
  const roots = buildFileTree(["app/peaches/a.py", "app/routes/r.py", "docs/A.md", "top.txt"].map((p) => file(p)));

  it("lists every node depth first with its depth", () => {
    expect(flattenTree(roots, new Set()).map((r) => `${r.depth}:${r.node.name}`)).toEqual([
      "0:app",
      "1:peaches",
      "2:a.py",
      "1:routes",
      "2:r.py",
      "0:docs",
      "1:A.md",
      "0:top.txt",
    ]);
  });

  it("leaves out what is inside a collapsed folder", () => {
    expect(flattenTree(roots, new Set(["app/peaches", "docs"])).map((r) => r.node.path)).toEqual([
      "app",
      "app/peaches",
      "app/routes",
      "app/routes/r.py",
      "docs",
      "top.txt",
    ]);
  });

  it("gives each row its place among its siblings, for assistive technology", () => {
    const rows = flattenTree(roots, new Set());
    expect(rows.map((r) => `${r.node.name}:${r.pos}/${r.size}`)).toEqual([
      "app:1/3",
      "peaches:1/2",
      "a.py:1/1",
      "routes:2/2",
      "r.py:1/1",
      "docs:2/3",
      "A.md:1/1",
      "top.txt:3/3",
    ]);
  });

  it("lays out ten thousand files (a smoke check of size, not of time)", () => {
    const many = Array.from({ length: 10_000 }, (_, i) => file(`src/m${i % 40}/f${i}.ts`, "A"));
    const rows = flattenTree(buildFileTree(many), new Set());
    expect(rows).toHaveLength(10_000 + 40 + 1);
    expect(rows[1].node.name).toBe("m0");
    expect(rows[2].node.name).toBe("f0.ts");
    expect(rows[3].node.name).toBe("f40.ts");
  });
});

describe("natural order", () => {
  const sorted = (names: string[]) =>
    names
      .map((n) => ({ n, k: naturalKey(n) }))
      .sort((a, b) => compareNatural(a.k, b.k, a.n, b.n))
      .map((x) => x.n);

  it("sorts numbers by value and letters without case", () => {
    expect(sorted(["file-16.ts", "file-8.ts", "File-9.ts", "file-100.ts", "b", "A", "a"])).toEqual([
      "A",
      "a",
      "b",
      "file-8.ts",
      "File-9.ts",
      "file-16.ts",
      "file-100.ts",
    ]);
  });

  it("orders equal numbers with leading zeros, and long numbers, consistently", () => {
    expect(sorted(["v010", "v10", "v9", "v99999999999999999999", "v100000000000000000000"])).toEqual([
      "v9",
      "v010",
      "v10",
      "v99999999999999999999",
      "v100000000000000000000",
    ]);
  });

  it("is a total order: names differing only in case still compare apart", () => {
    expect(compareNatural(naturalKey("a"), naturalKey("A"), "a", "A")).toBeGreaterThan(0);
    expect(compareNatural(naturalKey("x"), naturalKey("x"), "x", "x")).toBe(0);
  });
});

describe("cached views of a commit's files", () => {
  const files = ["src/file-16.ts", "src/file-8.ts", "a/z.txt", "B.md"].map((p) => file(p));

  it("builds the tree of one list of files once", () => {
    const tree = fileTreeOf(files);
    expect(fileTreeOf(files)).toBe(tree);
    expect(fileTreeOf([...files])).not.toBe(tree);
    expect(shape(tree)).toEqual([["a", ["z.txt"]], ["src", ["file-8.ts", "file-16.ts"]], "B.md"]);
  });

  it("sorts the flat list once, by whole path in the tree's natural order", () => {
    const list = sortedFilesOf(files);
    expect(sortedFilesOf(files)).toBe(list);
    expect(list.map((f) => f.path)).toEqual(["a/z.txt", "B.md", "src/file-8.ts", "src/file-16.ts"]);
    expect(files.map((f) => f.path)).toEqual(["src/file-16.ts", "src/file-8.ts", "a/z.txt", "B.md"]);
  });
});
