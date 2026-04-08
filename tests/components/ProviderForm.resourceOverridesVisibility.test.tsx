import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ProviderForm } from "@/components/providers/forms/ProviderForm";

vi.mock("@tanstack/react-query", () => ({
  useQuery: () => ({ data: [], isLoading: false }),
}));

vi.mock("@/lib/api", () => ({
  providersApi: {
    getOpenCodeLiveProviderIds: vi.fn(),
  },
}));

vi.mock("@/hooks/useOpenClaw", () => ({
  useOpenClawLiveProviderIds: () => ({ data: [], isLoading: false }),
}));

vi.mock("@/components/ui/form", () => ({
  Form: ({ children }: any) => <div>{children}</div>,
  FormField: ({ render }: any) =>
    render({ field: {}, fieldState: {}, formState: {} }),
  FormItem: ({ children }: any) => <div>{children}</div>,
  FormMessage: () => null,
}));

vi.mock("@/components/ui/button", () => ({
  Button: ({ children, ...props }: any) => <button {...props}>{children}</button>,
}));

vi.mock("@/components/ui/input", () => ({
  Input: (props: any) => <input {...props} />,
}));

vi.mock("@/components/ui/label", () => ({
  Label: ({ children, ...props }: any) => <label {...props}>{children}</label>,
}));

vi.mock("@/components/JsonEditor", () => ({
  default: ({ value = "" }: any) => <textarea readOnly value={value} />,
}));

vi.mock("@/components/providers/forms/ProviderPresetSelector", () => ({
  ProviderPresetSelector: () => <div data-testid="provider-preset-selector" />,
}));

vi.mock("@/components/providers/forms/BasicFormFields", () => ({
  BasicFormFields: () => <div data-testid="basic-form-fields" />,
}));

vi.mock("@/components/providers/forms/ClaudeFormFields", () => ({
  ClaudeFormFields: () => <div data-testid="claude-form-fields" />,
}));

vi.mock("@/components/providers/forms/CodexFormFields", () => ({
  CodexFormFields: () => <div data-testid="codex-form-fields" />,
}));

vi.mock("@/components/providers/forms/GeminiFormFields", () => ({
  GeminiFormFields: () => <div data-testid="gemini-form-fields" />,
}));

vi.mock("@/components/providers/forms/OpenCodeFormFields", () => ({
  OpenCodeFormFields: () => <div data-testid="opencode-form-fields" />,
}));

vi.mock("@/components/providers/forms/OpenClawFormFields", () => ({
  OpenClawFormFields: () => <div data-testid="openclaw-form-fields" />,
}));

vi.mock("@/components/providers/forms/OmoFormFields", () => ({
  OmoFormFields: () => <div data-testid="omo-form-fields" />,
}));

vi.mock("@/components/providers/forms/ProviderResourceOverridesConfig", () => ({
  ProviderResourceOverridesConfig: () => (
    <div data-testid="provider-resource-overrides" />
  ),
}));

vi.mock("@/components/providers/forms/ProviderAdvancedConfig", () => ({
  ProviderAdvancedConfig: () => <div data-testid="provider-advanced-config" />,
}));

vi.mock("@/components/providers/forms/CommonConfigEditor", () => ({
  CommonConfigEditor: () => <div data-testid="common-config-editor" />,
}));

vi.mock("@/components/providers/forms/CodexConfigEditor", () => ({
  default: () => <div data-testid="codex-config-editor" />,
}));

vi.mock("@/components/providers/forms/GeminiConfigEditor", () => ({
  default: () => <div data-testid="gemini-config-editor" />,
}));

vi.mock("@/components/providers/forms/hooks", () => ({
  useProviderCategory: () => ({ category: "third_party" }),
  useApiKeyState: () => ({
    apiKey: "",
    handleApiKeyChange: vi.fn(),
    showApiKey: () => true,
  }),
  useBaseUrlState: () => ({
    baseUrl: "",
    handleClaudeBaseUrlChange: vi.fn(),
  }),
  useModelState: () => ({
    claudeModel: "",
    reasoningModel: "",
    defaultHaikuModel: "",
    defaultSonnetModel: "",
    defaultOpusModel: "",
    handleModelChange: vi.fn(),
  }),
  useCodexConfigState: () => ({
    codexAuth: "",
    codexConfig: "",
    codexApiKey: "",
    codexBaseUrl: "",
    codexModelName: "",
    codexAuthError: "",
    setCodexAuth: vi.fn(),
    handleCodexApiKeyChange: vi.fn(),
    handleCodexBaseUrlChange: vi.fn(),
    handleCodexModelNameChange: vi.fn(),
    handleCodexConfigChange: vi.fn(),
    resetCodexConfig: vi.fn(),
  }),
  useApiKeyLink: () => ({
    shouldShowApiKeyLink: false,
    websiteUrl: "",
    isPartner: false,
    partnerPromotionKey: undefined,
  }),
  useTemplateValues: () => ({
    templateValues: {},
    templateValueEntries: [],
    selectedPreset: null,
    handleTemplateValueChange: vi.fn(),
    validateTemplateValues: () => ({ isValid: true }),
  }),
  useCommonConfigSnippet: () => ({
    useCommonConfig: false,
    commonConfigSnippet: "",
    commonConfigError: "",
    handleCommonConfigToggle: vi.fn(),
    handleCommonConfigSnippetChange: vi.fn(),
    isExtracting: false,
    handleExtract: vi.fn(),
  }),
  useCodexCommonConfig: () => ({
    useCommonConfig: false,
    commonConfigSnippet: "",
    commonConfigError: "",
    handleCommonConfigToggle: vi.fn(),
    handleCommonConfigSnippetChange: vi.fn(),
    isExtracting: false,
    handleExtract: vi.fn(),
    clearCommonConfigError: vi.fn(),
  }),
  useSpeedTestEndpoints: () => [],
  useCodexTomlValidation: () => ({
    configError: "",
    debouncedValidate: vi.fn(),
  }),
  useGeminiConfigState: () => ({
    geminiEnv: "",
    geminiConfig: "",
    geminiApiKey: "",
    geminiBaseUrl: "",
    geminiModel: "",
    envError: "",
    configError: "",
    handleGeminiApiKeyChange: vi.fn(),
    handleGeminiBaseUrlChange: vi.fn(),
    handleGeminiModelChange: vi.fn(),
    handleGeminiEnvChange: vi.fn(),
    handleGeminiConfigChange: vi.fn(),
    resetGeminiConfig: vi.fn(),
    envStringToObj: vi.fn(),
    envObjToString: vi.fn(),
  }),
  useGeminiCommonConfig: () => ({
    useCommonConfig: false,
    commonConfigSnippet: "",
    commonConfigError: "",
    handleCommonConfigToggle: vi.fn(),
    handleCommonConfigSnippetChange: vi.fn(),
    isExtracting: false,
    handleExtract: vi.fn(),
    clearCommonConfigError: vi.fn(),
  }),
  useOmoModelSource: () => ({
    omoModelOptions: [],
    omoModelVariantsMap: {},
    omoPresetMetaMap: {},
    existingOpencodeKeys: [],
  }),
  useOpencodeFormState: () => ({
    opencodeProviderKey: "test-provider",
    setOpencodeProviderKey: vi.fn(),
    opencodeNpm: "",
    handleOpencodeNpmChange: vi.fn(),
    opencodeApiKey: "",
    handleOpencodeApiKeyChange: vi.fn(),
    opencodeBaseUrl: "",
    handleOpencodeBaseUrlChange: vi.fn(),
    opencodeModels: { "gpt-4.1": { name: "gpt-4.1" } },
    handleOpencodeModelsChange: vi.fn(),
    opencodeExtraOptions: {},
    handleOpencodeExtraOptionsChange: vi.fn(),
    resetOpencodeState: vi.fn(),
  }),
  useOmoDraftState: () => ({
    omoAgents: [],
    setOmoAgents: vi.fn(),
    omoCategories: [],
    setOmoCategories: vi.fn(),
    omoOtherFieldsStr: "",
    setOmoOtherFieldsStr: vi.fn(),
    mergedOmoJsonPreview: "{}",
  }),
  useOpenclawFormState: () => ({
    existingOpenclawKeys: [],
    openclawProviderKey: "test-provider",
    setOpenclawProviderKey: vi.fn(),
    openclawBaseUrl: "",
    handleOpenclawBaseUrlChange: vi.fn(),
    openclawApiKey: "",
    handleOpenclawApiKeyChange: vi.fn(),
    openclawApi: "openai-completions",
    handleOpenclawApiChange: vi.fn(),
    openclawModels: [{ id: "gpt-4.1" }],
    handleOpenclawModelsChange: vi.fn(),
    openclawUserAgent: false,
    handleOpenclawUserAgentChange: vi.fn(),
    resetOpenclawState: vi.fn(),
  }),
  useCopilotAuth: () => ({
    isAuthenticated: false,
  }),
}));

vi.mock("@/lib/authBinding", () => ({
  resolveManagedAccountId: () => null,
}));

function renderProviderForm(appId: "claude" | "opencode" | "openclaw") {
  return render(
    <ProviderForm
      appId={appId}
      submitLabel="保存"
      onSubmit={vi.fn()}
      onCancel={vi.fn()}
      showButtons={false}
      initialData={{
        name: "Test Provider",
        websiteUrl: "",
        notes: "",
        settingsConfig: {},
        meta: {},
      }}
    />,
  );
}

describe("ProviderForm resource overrides visibility", () => {
  it("不会在 OpenCode 供应商表单中显示资源覆盖面板", () => {
    renderProviderForm("opencode");

    expect(
      screen.queryByTestId("provider-resource-overrides"),
    ).not.toBeInTheDocument();
  });

  it("不会在 OpenClaw 供应商表单中显示资源覆盖面板", () => {
    renderProviderForm("openclaw");

    expect(
      screen.queryByTestId("provider-resource-overrides"),
    ).not.toBeInTheDocument();
  });

  it("仍会在 Claude 供应商表单中显示资源覆盖面板", () => {
    renderProviderForm("claude");

    expect(screen.getByTestId("provider-resource-overrides")).toBeInTheDocument();
  });
});
