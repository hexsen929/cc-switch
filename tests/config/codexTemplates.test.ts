import { describe, expect, it } from "vitest";
import { parse as parseToml } from "smol-toml";
import { getCodexCustomTemplate } from "@/config/codexTemplates";
import {
  codexProviderPresets,
  generateThirdPartyConfig,
} from "@/config/codexProviderPresets";

describe("Codex custom templates", () => {
  it.each([
    ["custom", getCodexCustomTemplate().config],
    [
      "generated key-based",
      generateThirdPartyConfig(
        "acme",
        "https://example.com/v1",
        "custom-model",
      ),
    ],
    [
      "generated managed OAuth",
      generateThirdPartyConfig(
        "oauth",
        "https://example.com/v1",
        "custom-model",
        { requiresOpenAiAuth: false },
      ),
    ],
    ...codexProviderPresets.map((preset) => [preset.name, preset.config ?? ""]),
  ])("%s omits obsolete settings ignored by current Codex", (_name, config) => {
    const parsed = parseToml(config);
    expect(parsed).not.toHaveProperty("disable_response_storage");
    expect(parsed).not.toHaveProperty("preferred_auth_method");
    expect(parsed).not.toHaveProperty("features.remote_connections");
  });

  it("does not force Codex Goal mode in the custom provider template", () => {
    const template = getCodexCustomTemplate();
    const parsed = parseToml(template.config) as {
      features?: { goals?: boolean };
      model_providers?: Record<string, unknown>;
    };

    expect(template.auth).toEqual({ OPENAI_API_KEY: "" });
    expect(parsed.features?.goals).toBeUndefined();
    expect(parsed.model_providers?.custom).toBeDefined();
  });
});
