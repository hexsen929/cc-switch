import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { parse } from "smol-toml";
import {
  codexProviderPresets,
  generateThirdPartyConfig,
} from "@/config/codexProviderPresets";
import { getCodexCustomTemplate } from "@/config/codexTemplates";

/** Export actual application templates, never a separately maintained "good" copy. */
describe("official Codex configuration fixtures", () => {
  it("exports every preset and generated custom configuration for CLI loading", () => {
    const fixtures = [
      ...codexProviderPresets.map((preset, index) => ({
        id: `preset-${index}`,
        name: preset.name,
        config: preset.config,
      })),
      {
        id: "custom-template",
        name: "Custom template",
        config: getCodexCustomTemplate().config,
      },
      {
        id: "generated-api-key",
        name: "Generated API key provider",
        config: generateThirdPartyConfig(
          "Fixture provider",
          "http://127.0.0.1:9/v1",
          "fixture-model",
        ),
      },
      {
        id: "generated-managed-oauth",
        name: "Generated managed OAuth provider",
        config: generateThirdPartyConfig(
          "Managed fixture",
          "http://127.0.0.1:9/v1",
          "fixture-model",
          { requiresOpenAiAuth: false },
        ),
      },
      {
        id: "generated-escaped-values",
        name: "Generated quoted provider and model values",
        config: generateThirdPartyConfig(
          'Fixture "quoted" provider',
          "http://127.0.0.1:9/v1",
          'fixture/"model"',
        ),
      },
    ];
    expect(fixtures.length).toBeGreaterThan(20);
    for (const fixture of fixtures) {
      expect(() => parse(fixture.config), fixture.name).not.toThrow();
    }
    const output = process.env.CODEX_CONFIG_FIXTURES_DIR;
    if (!output) return;
    mkdirSync(output, { recursive: true });
    for (const fixture of fixtures) {
      writeFileSync(path.join(output, `${fixture.id}.toml`), fixture.config);
    }
    writeFileSync(
      path.join(output, "manifest.json"),
      JSON.stringify(
        fixtures.map(({ id, name }) => ({ id, name })),
        null,
        2,
      ),
    );
  });
});
