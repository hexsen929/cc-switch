import { describe, expect, it } from "vitest";
import {
  kimiConfigFromToml,
  kimiConfigToToml,
} from "@/components/providers/forms/helpers/kimiToml";
import {
  yamlNodeFromText,
  yamlNodeToText,
} from "@/components/providers/forms/helpers/yamlNode";
import type { KimiProviderConfig } from "@/config/kimiProviderPresets";

describe("native provider configuration helpers", () => {
  it.each(["custom_provider", "providers"])(
    "preserves unknown native values in %s",
    (section) => {
      const native = {
        enabled: false,
        api: "openai-completions",
        options: { baseURL: "https://relay.example/v1", futureFlag: false },
        models: { sample: { limit: { context: 128000 }, nativeExtra: [1, 2] } },
        futureConfig: { resources: ["reference"], retries: 0 },
      };
      const text = yamlNodeToText(section, "gateway", native);
      expect(yamlNodeFromText(text, section, "gateway")).toEqual({
        ok: true,
        value: native,
      });
    },
  );

  it("rejects inherited properties instead of treating them as the selected YAML node", () => {
    const text = "providers:\n  other:\n    api: openai-completions\n";
    expect(yamlNodeFromText(text, "providers", "__proto__")).toEqual({
      ok: false,
      error: { kind: "scope" },
    });
  });

  it.each(["__proto__", "constructor"])(
    "accepts %s only when that exact YAML key is present",
    (key) => {
      const value = { api: "openai-completions", models: [] };
      expect(
        yamlNodeFromText(
          yamlNodeToText("providers", key, value),
          "providers",
          key,
        ),
      ).toEqual({ ok: true, value });
    },
  );

  it("keeps Kimi native provider and model extensions through TOML editing", () => {
    const config: KimiProviderConfig = {
      provider: {
        type: "openai",
        base_url: "https://relay.example/v1",
        api_key: "fixture-key",
        native_extension: { retries: 0, enabled: false },
      },
      models: {
        "gateway/model-a": {
          provider: "old-key",
          model: "model-a",
          max_context_size: 128000,
          capabilities: ["tool_use", "future_capability"],
          overrides: { native_option: "keep" },
          extra_model_config: { limit: 7 },
        },
      },
    };
    const before = structuredClone(config);
    const result = kimiConfigFromToml(
      kimiConfigToToml(config, "gateway"),
      "gateway",
    );
    const expectedModel = { ...config.models["gateway/model-a"] };
    delete expectedModel.provider;
    expect(result).toEqual({
      ok: true,
      provider: config.provider,
      models: {
        "gateway/model-a": expectedModel,
      },
    });
    expect(config).toEqual(before);
  });
});
