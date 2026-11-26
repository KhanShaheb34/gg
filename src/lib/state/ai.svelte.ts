// "Generate with AI": the command-line AI tools as the core finds them (looked for when a
// field that offers it shows, when the settings open, on "Check again", and when the window
// comes back after a while; a tool is never run for that beyond `--version`, which only the
// settings ask for), which tool and model will run, the first-use notice, and the words the UI
// says about them.

import { api } from "../ipc";
import type { AiOutcome, AiTask, AiTool, AiToolInfo, AiTools, Settings } from "../types";
import { app } from "./app.svelte";

export const ai = $state({
  /** As the core last said; null until asked. */
  tools: null as AiTools | null,
  /** A look is under way. */
  checking: false,
});

/** Looks again when the window gets the focus this long after the last look, ms. */
const FOCUS_RECHECK_MS = 5 * 60_000;
let lastLook = 0;
let pending: Promise<void> | null = null;
let listening = false;

/**
 * Asks the core where the tools are: `versions` also asks each its version; `recheck` reads
 * the login shell's PATH again (a tool installed since gg started). Never throws: what fails
 * leaves what was known.
 */
export async function refreshAi(options: { versions?: boolean; recheck?: boolean } = {}): Promise<void> {
  listenForFocus();
  if (pending && !options.versions && !options.recheck) return pending;
  ai.checking = true;
  lastLook = Date.now();
  const run = (async () => {
    try {
      ai.tools = await api.aiTools(options);
    } catch {
      // No git, say: nothing more is known.
    } finally {
      ai.checking = false;
    }
  })();
  pending = run;
  try {
    await run;
  } finally {
    if (pending === run) pending = null;
  }
}

/** Looks for the tools once, if no look was made yet. */
export function ensureAi(): void {
  if (ai.tools === null && pending === null) void refreshAi();
}

function listenForFocus(): void {
  if (listening || typeof window === "undefined") return;
  listening = true;
  window.addEventListener("focus", () => {
    if (Date.now() - lastLook > FOCUS_RECHECK_MS) void refreshAi();
  });
}

/**
 * The tool that runs with `settings` (the one named, or the first installed in gg's order, as
 * the core picks it), with what the core said of it; null when it is not installed.
 */
export function chosenTool(tools: AiTools | null = ai.tools, settings: Settings = app.settings): AiToolInfo | null {
  if (!tools) return null;
  const installed = tools.tools.filter((t) => t.installed);
  if (settings.aiTool === "auto") return installed[0] ?? null;
  return installed.find((t) => t.tool === settings.aiTool) ?? null;
}

/** Tool `tool`'s info, installed or not. */
export function toolInfo(tool: AiTool, tools: AiTools | null = ai.tools): AiToolInfo | null {
  return tools?.tools.find((t) => t.tool === tool) ?? null;
}

/** The model `tool` runs with ("" for the tool's own default). */
export function modelFor(info: AiToolInfo, settings: Settings = app.settings): string {
  return settings.aiModels[info.tool] ?? info.defaultModel;
}

/** "Claude Code (haiku)", or "Codex (its default model)". */
export function runsWith(info: AiToolInfo, settings: Settings = app.settings): string {
  const model = modelFor(info, settings);
  return `${info.name} (${model === "" ? "its default model" : model})`;
}

/** The settings name a tool that is not installed (null: Automatic, or it is there). */
export function missingChoice(tools: AiTools | null = ai.tools, settings: Settings = app.settings): AiToolInfo | null {
  if (settings.aiTool === "auto" || !tools) return null;
  const info = toolInfo(settings.aiTool, tools);
  return info && !info.installed ? info : null;
}

/** Whether the first-use notice was seen for `tool`. */
export function noticeSeen(tool: AiTool, settings: Settings = app.settings): boolean {
  return settings.aiNoticeSeen.includes(tool);
}

/** Remembers that the first-use notice was seen for `tool` (kept in the settings). */
export async function rememberNotice(tool: AiTool): Promise<void> {
  if (noticeSeen(tool)) return;
  const seen = [...app.settings.aiNoticeSeen, tool];
  app.settings = { ...app.settings, aiNoticeSeen: seen };
  try {
    const saved = await api.updateSettings({ aiNoticeSeen: seen });
    app.settings = { ...app.settings, aiNoticeSeen: saved.aiNoticeSeen };
  } catch {
    // Not saved: asked again next time gg starts.
  }
}

/** What the first-use notice says is sent, for a commit message or a pull request. */
export function noticeText(info: AiToolInfo, kind: AiTask["kind"]): string {
  const what =
    kind === "commit"
      ? "The ticked changes (their diff and file names) and the branch name"
      : "The branch’s commits and diff, and the title and description as they are";
  return `${what} go to ${info.provider} through the ${info.name} you installed, under your own account. gg itself sends nothing anywhere.`;
}

/** What each prompt variable stands for, for the settings. */
export const VARIABLE_HINTS: Record<string, string> = {
  basePrompt: "gg’s own prompt, filled in (to add to it)",
  branch: "the branch’s name",
  stagedFiles: "the ticked files, a status letter and a path each",
  stagedPatch: "their patch, in a code fence, cut to 200 KB",
  baseBranch: "the branch it goes into",
  currentTitle: "the title as the form has it",
  currentBody: "the description as the form has it",
  currentDraft: "true or false",
  commitSummary: "the branch’s commits since its base",
  changedFiles: "the files they change",
  patch: "their patch, in a code fence, cut to 200 KB",
};

let seq = 0;
/** A name for one generation, to cancel it by. */
export function newRequestId(): string {
  seq += 1;
  return `ai-${Date.now().toString(36)}-${seq}`;
}

/**
 * Runs `task` in repository `repoId` as `requestId`. A failure the core could not even start
 * (an unknown repository, a branch that is gone) comes back as a failed outcome too, so the
 * field says it where the user looks.
 */
export async function generate(repoId: number, requestId: string, task: AiTask): Promise<AiOutcome> {
  try {
    return await api.aiGenerate(repoId, requestId, task);
  } catch (e) {
    return {
      status: "failed",
      tool: null,
      kind: "failed",
      message: e instanceof Error ? e.message : String(e),
      detail: "",
      logFrom: 0,
    };
  }
}

/** Cancels generation `requestId`; never throws. */
export async function cancelGeneration(requestId: string): Promise<void> {
  try {
    await api.aiCancel(requestId);
  } catch {
    // The generation ends by itself.
  }
}
