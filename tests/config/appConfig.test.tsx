import { describe, expect, it } from "vitest";
import {
  isAdditiveAppId,
  SKILLS_APP_IDS,
  MCP_APP_IDS,
  isProxyAppId,
  isModeAppId,
  isStackAppId,
  usesEditorView,
} from "@/config/appConfig";
import { APP_PROFILE_SCOPE } from "@/components/profiles/scope";
import { PROMPT_APP_IDS, promptAppOf } from "@/lib/query/prompts";
import { promptFileName } from "@/components/prompts/promptUtils";

describe("appConfig provider lifecycle", () => {
  it.each(["opencode", "openclaw", "hermes", "pi", "mcode"])(
    "classifies %s as additive",
    (appId) => {
      expect(isAdditiveAppId(appId)).toBe(true);
    },
  );

  it.each(["claude", "claude-desktop", "codex", "gemini", "grokbuild"])(
    "does not classify %s as additive",
    (appId) => {
      expect(isAdditiveAppId(appId)).toBe(false);
    },
  );
});

// Antigravity shares management UI, but has independent configuration and no CCS proxy.
describe("Antigravity capabilities", () => {
  it("supports skills and MCP with direct provider switching", () => {
    expect(SKILLS_APP_IDS).toContain("agy");
    expect(MCP_APP_IDS).toContain("agy");
    expect(isAdditiveAppId("agy")).toBe(false);
    expect(isProxyAppId("agy")).toBe(false);
    expect(isModeAppId("agy")).toBe(false);
    expect(isStackAppId("agy")).toBe(false);
    expect(usesEditorView("agy")).toBe(false);
    expect(APP_PROFILE_SCOPE.agy).toBeUndefined();
    expect(PROMPT_APP_IDS).toContain("agy");
    expect(promptAppOf("agy")).toBe("agy");
    expect(promptFileName("agy")).toBe("AGENTS.md");
  });
});
