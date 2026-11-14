// Instant feedback on a branch or tag name being typed: the rules of the core's name check
// (crates/gg-core/src/git/names.rs: `git check-ref-format --branch`, no leading "-", not
// HEAD), and whether the name is taken. The core stays the authority: it checks again, and
// what it refuses is shown the same way.

/** Why `name` cannot be a new `what` ("branch" or "tag"), or null if it can. `existing`: names taken. */
export function refNameProblem(name: string, existing: string[], what: "branch" | "tag" = "branch"): string | null {
  const why = formProblem(name);
  if (why) return why;
  if (existing.includes(name)) return `A ${what} named “${name}” already exists.`;
  return null;
}

/** Why `name` is not a usable ref name, or null. In the core's order, with its words; full sentences. */
function formProblem(name: string): string | null {
  if (name === "") return "A name is needed.";
  if (name.startsWith("-")) return "It can’t start with a hyphen.";
  if (name === "HEAD") return "HEAD is git’s name for the current commit.";
  if (name === "@") return "“@” alone is git’s name for HEAD.";
  const odd = /[\u0000-\u001f\u007f ]/.exec(name);
  if (odd) return odd[0] === " " ? "It can’t contain spaces." : "It can’t contain control characters.";
  if (/[~^:?*[\\]/.test(name)) return "It can’t contain any of: ~ ^ : ? * [ \\ or a space.";
  if (name.includes("..")) return "It can’t contain two dots in a row.";
  if (name.includes("@{")) return "It can’t contain “@{” anywhere.";
  if (name.startsWith("/") || name.endsWith("/") || name.includes("//")) {
    return "It can’t start or end with a slash, or have two in a row.";
  }
  if (name.endsWith(".")) return "It can’t end with a dot.";
  for (const part of name.split("/")) {
    if (part.startsWith(".")) return "No part of it can start with a dot.";
    if (part.endsWith(".lock")) return "No part of it can end in “.lock”, git’s own ending.";
  }
  return null;
}
