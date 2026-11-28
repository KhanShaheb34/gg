import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AiToolInfo, AiTools, Settings } from "../types";

vi.mock("../ipc", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../ipc")>();
  return {
    CoreError: actual.CoreError,
    onEvent: vi.fn(() => () => {}),
    api: { aiTools: vi.fn(), aiGenerate: vi.fn(), aiCancel: vi.fn(), updateSettings: vi.fn() },
  };
});

type Ipc = typeof import("../ipc");

function info(tool: "claude" | "codex", installed: boolean): AiToolInfo {
  const claude = tool === "claude";
  return {
    tool,
    name: claude ? "Claude Code" : "Codex",
    provider: claude ? "Anthropic" : "OpenAI",
    installed,
    version: null,
    path: installed ? `/bin/${tool}` : null,
    installUrl: "https://example.invalid",
    signIn: claude ? "claude" : "codex login",
    defaultModel: claude ? "haiku" : "",
    models: [],
  };
}

function tools(claude: boolean, codex: boolean, chosen: AiTools["chosen"]): AiTools {
  return { tools: [info("claude", claude), info("codex", codex)], chosen, commitPrompt: "", commitVariables: [], prPrompt: "", prVariables: [] };
}

const settings = (patch: Partial<Settings> = {}): Settings => ({
  theme: "system",
  fetchIntervalSecs: 300,
  editorCommand: null,
  terminalCommand: null,
  worktreePattern: "../{repo}.worktrees/{branch}",
  dateFormat: "standard",
  checkForUpdates: true,
  openPrTabAfterPush: true,
  aiTool: "auto",
  aiModels: {},
  aiCommitPrompt: null,
  aiPrPrompt: null,
  aiNoticeSeen: [],
  cloneDir: null,
  ...patch,
});

describe("Generate with AI's words and choices", () => {
  let ai: typeof import("./ai.svelte");
  let ipc: Ipc;

  beforeEach(async () => {
    vi.resetModules();
    ipc = await import("../ipc");
    ai = await import("./ai.svelte");
  });

  it("names the tool and the model that will run", () => {
    expect(ai.runsWith(info("claude", true), settings())).toBe("Claude Code (haiku)");
    expect(ai.runsWith(info("claude", true), settings({ aiModels: { claude: "sonnet" } }))).toBe("Claude Code (sonnet)");
    expect(ai.runsWith(info("codex", true), settings())).toBe("Codex (its default model)");
    expect(ai.runsWith(info("claude", true), settings({ aiModels: { claude: "" } }))).toBe("Claude Code (its default model)");
  });

  it("knows which tool runs and which one the settings miss", () => {
    expect(ai.chosenTool(tools(true, true, "claude"))?.name).toBe("Claude Code");
    expect(ai.chosenTool(tools(false, false, null))).toBeNull();
    expect(ai.chosenTool(null)).toBeNull();
    expect(ai.chosenTool(tools(true, true, "claude"), settings({ aiTool: "codex" }))?.name).toBe("Codex");
    expect(ai.chosenTool(tools(false, true, "codex"), settings())?.name).toBe("Codex");
    expect(ai.chosenTool(tools(true, false, "claude"), settings({ aiTool: "codex" }))).toBeNull();
    expect(ai.missingChoice(tools(true, false, null), settings({ aiTool: "codex" }))?.name).toBe("Codex");
    expect(ai.missingChoice(tools(false, false, null), settings())).toBeNull();
    expect(ai.missingChoice(tools(true, true, "claude"), settings({ aiTool: "claude" }))).toBeNull();
  });

  it("says what the first-use notice sends, and to whom", () => {
    const commit = ai.noticeText(info("claude", true), "commit");
    expect(commit).toContain("ticked changes");
    expect(commit).toContain("go to Anthropic through the Claude Code you installed");
    expect(commit).toContain("gg itself sends nothing anywhere");
    expect(ai.noticeText(info("codex", true), "pr")).toContain("go to OpenAI through the Codex you installed");
    expect(ai.noticeSeen("codex", settings({ aiNoticeSeen: ["codex"] }))).toBe(true);
    expect(ai.noticeSeen("claude", settings({ aiNoticeSeen: ["codex"] }))).toBe(false);
  });

  it("turns a request the core refused into a failure the field can say", async () => {
    vi.mocked(ipc.api.aiGenerate).mockRejectedValueOnce(new Error("There is no local branch named “x”."));
    const out = await ai.generate(1, ai.newRequestId(), { kind: "commit", path: "/r", paths: ["a"] });
    expect(out).toEqual({
      status: "failed",
      tool: null,
      kind: "failed",
      message: "There is no local branch named “x”.",
      detail: "",
      logFrom: 0,
    });
    expect(ai.newRequestId()).not.toBe(ai.newRequestId());
  });

  it("looks for the tools once, however many fields ask", async () => {
    vi.mocked(ipc.api.aiTools).mockResolvedValue(tools(true, false, "claude"));
    ai.ensureAi();
    ai.ensureAi();
    await ai.refreshAi();
    expect(ipc.api.aiTools).toHaveBeenCalledTimes(1);
    expect(ai.ai.tools?.chosen).toBe("claude");
    await ai.refreshAi({ versions: true });
    expect(ipc.api.aiTools).toHaveBeenLastCalledWith({ versions: true });
  });
});
