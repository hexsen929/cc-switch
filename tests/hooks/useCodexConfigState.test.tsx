import { act, renderHook } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { useCodexConfigState } from "@/components/providers/forms/hooks/useCodexConfigState";

describe("useCodexConfigState", () => {
  it("uses experimental_bearer_token when auth API key is a proxy placeholder", () => {
    const { result } = renderHook(() =>
      useCodexConfigState({
        initialData: {
          settingsConfig: {
            auth: {
              OPENAI_API_KEY: "PROXY_MANAGED",
            },
            config: `model_provider = "custom"

[model_providers.custom]
name = "custom"
base_url = "https://api.example/v1"
experimental_bearer_token = "real-provider-key"
`,
          },
        },
      }),
    );

    expect(result.current.codexApiKey).toBe("real-provider-key");
    expect(result.current.getCodexAuthApiKey(result.current.codexAuth)).toBe(
      "",
    );
  });

  it("keeps model catalog empty when only config.toml model is present", () => {
    const { result } = renderHook(() =>
      useCodexConfigState({
        initialData: {
          settingsConfig: {
            auth: {},
            config: `model_provider = "custom"
model = "deepseek-v4-flash"

[model_providers.custom]
name = "custom"
base_url = "https://api.example/v1"
`,
          },
        },
      }),
    );

    expect(result.current.codexCatalogModels).toEqual([]);
  });

  it("does not seed model catalog from config.toml during preset reset", () => {
    const { result } = renderHook(() => useCodexConfigState({}));

    act(() => {
      result.current.resetCodexConfig(
        {},
        `model_provider = "custom"
model = "deepseek-v4-flash"
`,
      );
    });

    expect(result.current.codexCatalogModels).toEqual([]);
  });

  it("keeps explicit modelCatalog over config.toml model", () => {
    const { result } = renderHook(() =>
      useCodexConfigState({
        initialData: {
          settingsConfig: {
            auth: {},
            config: `model_provider = "custom"
model = "stale-model"
`,
            modelCatalog: {
              models: [{ model: "kimi-k2.6", displayName: "Kimi K2.6" }],
            },
          },
        },
      }),
    );

    expect(result.current.codexCatalogModels).toEqual([
      { model: "kimi-k2.6", displayName: "Kimi K2.6", contextWindow: "" },
    ]);
  });

  it("writes API key to auth and experimental_bearer_token", () => {
    const initialData = {
      settingsConfig: {
        auth: {},
        config: `model_provider = "custom"

[model_providers.custom]
name = "custom"
base_url = "https://api.example/v1"
experimental_bearer_token = "old-key"
`,
      },
    };
    const { result } = renderHook(() =>
      useCodexConfigState({
        initialData,
      }),
    );

    act(() => {
      result.current.handleCodexApiKeyChange("new-provider-key");
    });

    expect(JSON.parse(result.current.codexAuth).OPENAI_API_KEY).toBe(
      "new-provider-key",
    );
    expect(result.current.codexConfig).toContain(
      'experimental_bearer_token = "new-provider-key"',
    );
  });
});
