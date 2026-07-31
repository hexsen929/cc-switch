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
model_instructions_file = "./instruction_5.6.md"
`,
        [],
        ["./instruction_default.md"],
      );
    });

    expect(result.current.codexCatalogModels).toEqual([]);
    expect(result.current.codexModelInstructionsEnabled).toBe(true);
    expect(result.current.codexModelInstructionsFile).toBe(
      "./instruction_5.6.md",
    );
    expect(result.current.codexModelInstructionsFiles).toEqual([
      "./instruction_default.md",
      "./instruction_5.6.md",
    ]);

    act(() => {
      result.current.resetCodexConfig({}, 'model_provider = "custom"\n');
    });

    expect(result.current.codexModelInstructionsEnabled).toBe(false);
    expect(result.current.codexModelInstructionsFile).toBe("");
    expect(result.current.codexModelInstructionsFiles).toEqual([]);
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

  it("loads, switches, and disables saved model instruction files", () => {
    const initialData = {
      settingsConfig: {
        auth: {},
        config: `model_provider = "custom"
model_instructions_file = "./instruction_5.6.md"

[model_providers.custom]
name = "custom"
`,
        modelInstructionsFiles: [
          "./instruction_default.md",
          "./instruction_5.6.md",
        ],
      },
    };
    const { result } = renderHook(() => useCodexConfigState({ initialData }));

    expect(result.current.codexModelInstructionsEnabled).toBe(true);
    expect(result.current.codexModelInstructionsFile).toBe(
      "./instruction_5.6.md",
    );
    expect(result.current.codexModelInstructionsFiles).toEqual([
      "./instruction_default.md",
      "./instruction_5.6.md",
    ]);

    act(() => {
      result.current.handleCodexModelInstructionsActiveFileChange(
        "./instruction_default.md",
      );
    });
    expect(result.current.codexConfig).toContain(
      'model_instructions_file = "./instruction_default.md"',
    );

    act(() => {
      result.current.handleCodexModelInstructionsActiveFileChange(null);
    });
    expect(result.current.codexModelInstructionsEnabled).toBe(false);
    expect(result.current.codexConfig).not.toContain("model_instructions_file");
    expect(result.current.codexModelInstructionsFiles).toEqual([
      "./instruction_default.md",
      "./instruction_5.6.md",
    ]);

    act(() => {
      result.current.handleCodexModelInstructionsActiveFileChange(
        "./instruction_default.md",
      );
    });
    expect(result.current.codexConfig).toContain(
      'model_instructions_file = "./instruction_default.md"',
    );
  });

  it("adds a hand-written active instruction file to the saved list", () => {
    const initialData = {
      settingsConfig: {
        auth: {},
        config: 'model_instructions_file = "./manual.md"\n',
        modelInstructionsFiles: ["./saved.md"],
      },
    };
    const { result } = renderHook(() => useCodexConfigState({ initialData }));

    expect(result.current.codexModelInstructionsFiles).toEqual([
      "./saved.md",
      "./manual.md",
    ]);
  });

  it("keeps the file manager in sync with manual config edits", () => {
    const initialData = {
      settingsConfig: {
        auth: {},
        config: "",
        modelInstructionsFiles: ["./saved.md"],
      },
    };
    const { result } = renderHook(() =>
      useCodexConfigState({
        initialData,
      }),
    );

    act(() => {
      result.current.handleCodexConfigChange(
        'model_instructions_file = "./typed-in-toml.md"\n',
      );
    });

    expect(result.current.codexModelInstructionsEnabled).toBe(true);
    expect(result.current.codexModelInstructionsFile).toBe(
      "./typed-in-toml.md",
    );
    expect(result.current.codexModelInstructionsFiles).toEqual([
      "./saved.md",
      "./typed-in-toml.md",
    ]);
  });
});
