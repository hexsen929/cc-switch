import { describe, expect, it } from "vitest";
import {
  isAdditiveAppId,
  isPiFamilyApp,
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
  it.each(["opencode", "openclaw", "hermes", "pi", "mcode", "kimi", "omp"])(
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

// Kimi keeps its own additive catalog and native authentication; global resources remain available.
describe("Kimi Code capabilities", () => {
  it("supports native global resources without enabling fork proxy or Profile modes", () => {
    expect(isAdditiveAppId("kimi")).toBe(true);
    expect(SKILLS_APP_IDS).toContain("kimi");
    expect(MCP_APP_IDS).toContain("kimi");
    expect(PROMPT_APP_IDS).toContain("kimi");
    expect(promptAppOf("kimi")).toBe("kimi");
    expect(promptFileName("kimi")).toBe("AGENTS.md");
    expect(isProxyAppId("kimi")).toBe(false);
    expect(isModeAppId("kimi")).toBe(false);
    expect(isStackAppId("kimi")).toBe(false);
    expect(usesEditorView("kimi")).toBe(false);
    expect(APP_PROFILE_SCOPE.kimi).toBeUndefined();
  });
});

// Oh My Pi shares Pi's catalog UI, but keeps independent native files and capability boundaries.
describe("Oh My Pi capabilities", () => {
  it("supports native global resources without enabling proxy, Stack, or Profile modes", () => {
    expect(isPiFamilyApp("omp")).toBe(true);
    expect(isPiFamilyApp("pi")).toBe(true);
    expect(isPiFamilyApp("kimi")).toBe(false);
    expect(isAdditiveAppId("omp")).toBe(true);
    expect(SKILLS_APP_IDS).toContain("omp");
    expect(MCP_APP_IDS).toContain("omp");
    expect(PROMPT_APP_IDS).toContain("omp");
    expect(promptAppOf("omp")).toBe("omp");
    expect(promptFileName("omp")).toBe("AGENTS.md");
    expect(isProxyAppId("omp")).toBe(false);
    expect(isModeAppId("omp")).toBe(false);
    expect(isStackAppId("omp")).toBe(false);
    expect(usesEditorView("omp")).toBe(false);
    expect(APP_PROFILE_SCOPE.omp).toBeUndefined();
  });
});
