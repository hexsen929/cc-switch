import { describe, expect, it } from "vitest";
import {
  APP_IDS,
  orderAppIds,
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
  it.each([
    "opencode",
    "openclaw",
    "hermes",
    "pi",
    "mcode",
    "kimi",
    "dsh",
    "omp",
  ])("classifies %s as additive", (appId) => {
    expect(isAdditiveAppId(appId)).toBe(true);
  });

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

describe("DeepSeek Harness capabilities", () => {
  it("keeps native resources additive without enabling fork routing modes", () => {
    expect(isAdditiveAppId("dsh")).toBe(true);
    expect(isPiFamilyApp("dsh")).toBe(false);
    expect(SKILLS_APP_IDS).toContain("dsh");
    expect(MCP_APP_IDS).toContain("dsh");
    expect(PROMPT_APP_IDS).toContain("dsh");
    expect(promptAppOf("dsh")).toBe("dsh");
    expect(promptFileName("dsh")).toBe("AGENTS.md");
    expect(isProxyAppId("dsh")).toBe(false);
    expect(isModeAppId("dsh")).toBe(false);
    expect(isStackAppId("dsh")).toBe(false);
    expect(usesEditorView("dsh")).toBe(false);
    expect(APP_PROFILE_SCOPE.dsh).toBeUndefined();
  });
});

describe("orderAppIds", () => {
  it("falls back to the default order when nothing is saved", () => {
    expect(orderAppIds(undefined)).toEqual(APP_IDS);
    expect(orderAppIds([])).toEqual(APP_IDS);
  });

  it("keeps the saved order and drops unknown or repeated ids", () => {
    const saved = [...APP_IDS].reverse();
    expect(orderAppIds(["future-app", ...saved, "claude"])).toEqual(saved);
  });

  it("puts an app missing from the saved order after its default predecessor", () => {
    const saved = APP_IDS.filter((app) => app !== "kimi");
    const [first, ...rest] = saved;
    // 把 mcode（kimi 默认的前一个）挪到最后，kimi 应跟着它
    const moved = [first, ...rest.filter((app) => app !== "mcode"), "mcode"];
    expect(orderAppIds(moved)).toEqual([...moved, "kimi"]);
    expect(orderAppIds(APP_IDS.filter((app) => app !== "claude"))[0]).toBe(
      "claude",
    );
  });

  it("inserts newly managed DSH after Kimi without discarding the saved order", () => {
    const saved = APP_IDS.filter((app) => app !== "dsh");
    const result = orderAppIds(saved);
    expect(result.filter((app) => app !== "dsh")).toEqual(saved);
    expect(result.indexOf("dsh")).toBe(result.indexOf("kimi") + 1);
  });
});
