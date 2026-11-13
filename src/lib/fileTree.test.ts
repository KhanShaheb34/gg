import { describe, expect, it } from "vitest";
import { buildFileTree, flattenTree, type TreeNode } from "./fileTree";
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

  it("handles ten thousand files quickly", () => {
    const many = Array.from({ length: 10_000 }, (_, i) => file(`src/m${i % 40}/f${i}.ts`, "A"));
    const started = performance.now();
    const rows = flattenTree(buildFileTree(many), new Set());
    expect(rows).toHaveLength(10_000 + 40 + 1);
    expect(performance.now() - started).toBeLessThan(500);
  });
});
