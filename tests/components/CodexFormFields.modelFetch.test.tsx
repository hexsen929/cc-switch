import type { ComponentProps } from "react";
import {
  act,
  fireEvent,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { FormProvider, useForm } from "react-hook-form";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { toast } from "sonner";
import { renderWithQueryClient as render } from "../utils/testQueryClient";
import { CodexFormFields } from "@/components/providers/forms/CodexFormFields";
import {
  copilotGetModels,
  copilotGetModelsForAccount,
  type CopilotModel,
} from "@/lib/api/copilot";
import {
  fetchModelsForConfig,
  fetchXaiOauthModels,
  modelFetchRequestHeaders,
  showFetchModelsError,
  type FetchedModel,
} from "@/lib/api/model-fetch";

vi.mock("@/lib/api/copilot", () => ({
  copilotGetModels: vi.fn(),
  copilotGetModelsForAccount: vi.fn(),
}));
vi.mock("@/lib/api/model-fetch", () => ({
  fetchModelsForConfig: vi.fn(),
  fetchXaiOauthModels: vi.fn(),
  modelFetchRequestHeaders: vi.fn(),
  showFetchModelsError: vi.fn(),
}));
vi.mock("sonner", () => ({
  toast: { error: vi.fn(), success: vi.fn(), info: vi.fn() },
}));
vi.mock("@/components/providers/forms/CopilotAuthSection", () => ({
  CopilotAuthSection: () => null,
}));
vi.mock("@/components/providers/forms/XaiOAuthSection", () => ({
  XaiOAuthSection: () => null,
}));
vi.mock("@/components/providers/forms/shared", async (importOriginal) => {
  const actual =
    await importOriginal<
      typeof import("@/components/providers/forms/shared")
    >();
  return {
    ...actual,
    ModelDropdown: ({ models }: { models: FetchedModel[] }) => (
      <output data-testid="model-options">
        {models.map((model) => model.id).join(",")}
      </output>
    ),
  };
});

type Props = ComponentProps<typeof CodexFormFields>;
type ProviderKind = "copilot" | "xai" | "config";

function pendingModels() {
  let resolve!: (models: Array<CopilotModel & FetchedModel>) => void;
  let reject!: (reason: Error) => void;
  const promise = new Promise<Array<CopilotModel & FetchedModel>>(
    (res, rej) => {
      resolve = res;
      reject = rej;
    },
  );
  return { promise, resolve, reject };
}

function advertisedModel(id = "model-1"): CopilotModel & FetchedModel {
  return {
    id,
    name: "Model One",
    vendor: "vendor",
    ownedBy: "vendor",
    model_picker_enabled: true,
    supported_endpoints: ["/responses"],
    context_window: 400_000,
  };
}

function makeProps(kind: ProviderKind): Props {
  return {
    isCopilotPreset: kind === "copilot",
    isCopilotAuthenticated: true,
    selectedGitHubAccountId: "github-account",
    isXaiOauthPreset: kind === "xai",
    isXaiOauthAuthenticated: true,
    selectedXaiAccountId: "xai-account",
    codexApiKey: "test-key",
    onApiKeyChange: vi.fn(),
    category: "third_party",
    shouldShowApiKeyLink: false,
    websiteUrl: "",
    shouldShowSpeedTest: false,
    codexBaseUrl: "https://example.com/v1",
    onBaseUrlChange: vi.fn(),
    isFullUrl: false,
    onFullUrlChange: vi.fn(),
    isEndpointModalOpen: false,
    onEndpointModalToggle: vi.fn(),
    autoSelect: false,
    onAutoSelectChange: vi.fn(),
    codexModel: "model-1",
    onModelChange: vi.fn(),
    apiFormat: "openai_chat",
    onApiFormatChange: vi.fn(),
    anthropicAuthField: "ANTHROPIC_AUTH_TOKEN",
    onAnthropicAuthFieldChange: vi.fn(),
    impersonateClaudeCode: false,
    onImpersonateClaudeCodeChange: vi.fn(),
    maxOutputTokens: "",
    onMaxOutputTokensChange: vi.fn(),
    promptCacheRouting: "auto",
    onPromptCacheRoutingChange: vi.fn(),
    catalogModels: [],
    onCatalogModelsChange: vi.fn(),
    speedTestEndpoints: [],
    customUserAgent: "",
    onCustomUserAgentChange: vi.fn(),
    localProxyHeadersOverride: "",
    onLocalProxyHeadersOverrideChange: vi.fn(),
    localProxyBodyOverride: "",
    onLocalProxyBodyOverrideChange: vi.fn(),
  };
}

function Harness(props: Props) {
  const form = useForm();
  return (
    <FormProvider {...form}>
      <CodexFormFields {...props} />
    </FormProvider>
  );
}

const fetchMockFor = (kind: ProviderKind) =>
  kind === "copilot"
    ? vi.mocked(copilotGetModelsForAccount)
    : kind === "xai"
      ? vi.mocked(fetchXaiOauthModels)
      : vi.mocked(fetchModelsForConfig);

// 拉到列表后按钮改叫「选择模型」并自动打开选择弹窗（弹窗外的元素被设为 aria-hidden）。
const fetchButton = () =>
  screen.getAllByRole("button", {
    name: /^providerForm\.(fetchModels|modelPicker\.title)$/,
    hidden: true,
  })[0];
const providerKinds: ProviderKind[] = ["copilot", "xai", "config"];

describe("Codex model-fetch lifecycle", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.spyOn(console, "warn").mockImplementation(() => {});
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("fetches only models supporting the selected Copilot protocol", async () => {
    const responsesModel = advertisedModel("responses-model");
    const chatModel = {
      ...advertisedModel("chat-model"),
      supported_endpoints: ["/chat/completions"],
    };
    vi.mocked(copilotGetModelsForAccount).mockResolvedValue([
      responsesModel,
      chatModel,
    ]);
    const props = {
      ...makeProps("copilot"),
      copilotApiFormat: "openai_chat" as const,
    };
    render(<Harness {...props} />);
    fireEvent.click(fetchButton());
    await waitFor(() => expect(fetchButton()).toBeEnabled());
    expect(screen.getByTestId("model-options")).toHaveTextContent("chat-model");
    expect(screen.getByTestId("model-options")).not.toHaveTextContent(
      "responses-model",
    );
    expect(props.onModelChange).toHaveBeenCalledWith("chat-model");
    expect(props.onCatalogModelsChange).toHaveBeenCalledWith([
      expect.objectContaining({ model: "chat-model" }),
    ]);
  });

  it("discards a pending model list after the Copilot protocol changes", async () => {
    const pending = pendingModels();
    vi.mocked(copilotGetModelsForAccount).mockReturnValue(pending.promise);
    const props = makeProps("copilot");
    const { rerender } = render(<Harness {...props} />);
    fireEvent.click(fetchButton());
    rerender(<Harness {...props} copilotApiFormat="openai_chat" />);
    await act(async () => pending.resolve([advertisedModel()]));
    expect(props.onCatalogModelsChange).not.toHaveBeenCalled();
    expect(props.onModelChange).not.toHaveBeenCalled();
    expect(toast.success).not.toHaveBeenCalled();
  });

  it.each(["gpt-6-astra", "claude-model"])(
    "prefers an available GPT model without replacing a valid selection (%s)",
    async (currentModel) => {
      vi.mocked(copilotGetModelsForAccount).mockResolvedValue([
        advertisedModel("claude-model"),
        advertisedModel("gpt-available"),
      ]);
      const props = { ...makeProps("copilot"), codexModel: currentModel };
      render(<Harness {...props} />);
      fireEvent.click(fetchButton());
      await waitFor(() => expect(fetchButton()).toBeEnabled());
      if (currentModel === "claude-model") {
        expect(props.onModelChange).not.toHaveBeenCalled();
      } else {
        expect(props.onModelChange).toHaveBeenCalledWith("gpt-available");
      }
    },
  );

  it.each(providerKinds)(
    "loads %s models through the existing provider route",
    async (kind) => {
      const pending = pendingModels();
      fetchMockFor(kind).mockReturnValue(pending.promise);
      const props = makeProps(kind);
      render(<Harness {...props} />);

      fireEvent.click(fetchButton());
      expect(fetchButton()).toBeDisabled();
      expect(fetchMockFor(kind)).toHaveBeenCalledTimes(1);
      if (kind === "copilot") {
        expect(copilotGetModelsForAccount).toHaveBeenCalledWith(
          "github-account",
        );
      } else if (kind === "xai") {
        expect(fetchXaiOauthModels).toHaveBeenCalledWith("xai-account");
      } else {
        expect(fetchModelsForConfig).toHaveBeenCalledWith(
          props.codexBaseUrl,
          "test-key",
          false,
          undefined,
          "",
          { requestHeaders: undefined },
        );
      }

      await act(async () => pending.resolve([advertisedModel()]));
      await waitFor(() => expect(fetchButton()).toBeEnabled());
      expect(screen.getByTestId("model-options")).toHaveTextContent("model-1");
      // 列表上的按钮拉取成功后直接打开选择弹窗，不再弹成功提示
      expect(await screen.findByRole("dialog")).toBeInTheDocument();
      expect(toast.success).not.toHaveBeenCalled();
      expect(showFetchModelsError).not.toHaveBeenCalled();
      if (kind === "copilot") {
        expect(props.onCatalogModelsChange).toHaveBeenCalledWith([
          {
            model: "model-1",
            displayName: "Model One",
            contextWindow: 400_000,
            supportsParallelToolCalls: false,
            inputModalities: ["text"],
          },
        ]);
      } else {
        expect(props.onCatalogModelsChange).not.toHaveBeenCalled();
      }
    },
  );

  it.each(providerKinds)(
    "keeps the empty-list notification for %s",
    async (kind) => {
      fetchMockFor(kind).mockResolvedValue([]);
      render(<Harness {...makeProps(kind)} />);
      fireEvent.click(fetchButton());

      await waitFor(() =>
        expect(toast.info).toHaveBeenCalledWith(
          "providerForm.fetchModelsEmpty",
        ),
      );
      expect(fetchButton()).toBeEnabled();
      expect(toast.success).not.toHaveBeenCalled();
    },
  );

  it.each(providerKinds)(
    "reports %s failures and releases loading state",
    async (kind) => {
      const error = new Error("model-list unavailable");
      fetchMockFor(kind).mockRejectedValue(error);
      render(<Harness {...makeProps(kind)} />);
      fireEvent.click(fetchButton());

      await waitFor(() =>
        expect(showFetchModelsError).toHaveBeenCalledWith(
          error,
          expect.any(Function),
        ),
      );
      expect(fetchButton()).toBeEnabled();
      expect(toast.success).not.toHaveBeenCalled();
      expect(console.warn).toHaveBeenCalledWith(
        kind === "copilot"
          ? "[Copilot] Failed to fetch models:"
          : kind === "xai"
            ? "[XaiOAuth] Failed to fetch models:"
            : "[ModelFetch] Failed:",
        error,
      );
    },
  );

  it.each(providerKinds)(
    "ignores stale %s results after the request identity changes",
    async (kind) => {
      const pending = pendingModels();
      fetchMockFor(kind).mockReturnValue(pending.promise);
      const props = makeProps(kind);
      const view = render(<Harness {...props} />);
      fireEvent.click(fetchButton());
      view.rerender(
        <Harness
          {...props}
          selectedGitHubAccountId="another-github-account"
          selectedXaiAccountId="another-xai-account"
          codexBaseUrl="https://other.example/v1"
        />,
      );

      await act(async () => pending.resolve([advertisedModel()]));
      await waitFor(() => expect(fetchButton()).toBeEnabled());
      expect(screen.queryByTestId("model-options")).not.toBeInTheDocument();
      expect(props.onCatalogModelsChange).not.toHaveBeenCalled();
      expect(toast.success).not.toHaveBeenCalled();
    },
  );

  it("keeps Copilot filtering, applies reported prompt limits and selects a default model", async () => {
    vi.mocked(copilotGetModels).mockResolvedValue([
      advertisedModel(),
      {
        ...advertisedModel("messages-only"),
        supported_endpoints: ["/v1/messages"],
      },
    ]);
    const props = {
      ...makeProps("copilot"),
      selectedGitHubAccountId: null,
      codexModel: "unavailable",
      catalogModels: [{ model: "model-1", contextWindow: 200_000 }],
    };
    render(<Harness {...props} />);
    fireEvent.click(fetchButton());

    await waitFor(() =>
      expect(props.onModelChange).toHaveBeenCalledWith("model-1"),
    );
    expect(copilotGetModels).toHaveBeenCalledTimes(1);
    expect(copilotGetModelsForAccount).not.toHaveBeenCalled();
    expect(props.onCatalogModelsChange).toHaveBeenCalledWith([
      expect.objectContaining({ model: "model-1", contextWindow: 400_000 }),
    ]);
    expect(screen.getAllByTestId("model-options")[0]).not.toHaveTextContent(
      "messages-only",
    );
  });

  it("overrides stale managed metadata with live Copilot capabilities", async () => {
    vi.mocked(copilotGetModelsForAccount).mockResolvedValue([
      {
        ...advertisedModel(),
        context_window: 922_000,
        supports_parallel_tool_calls: true,
        reasoning_effort: ["none", "low", "medium", "high", "xhigh"],
      },
    ]);
    const props = {
      ...makeProps("copilot"),
      catalogModels: [
        {
          model: "model-1",
          contextWindow: 1_048_576,
          supportsParallelToolCalls: false,
          reasoningLevels: ["max"],
          baseInstructions: "Keep me",
        },
      ],
    };
    render(<Harness {...props} />);
    fireEvent.click(fetchButton());

    await waitFor(() =>
      expect(props.onCatalogModelsChange).toHaveBeenCalledWith([
        expect.objectContaining({
          model: "model-1",
          contextWindow: 922_000,
          supportsParallelToolCalls: true,
          reasoningLevels: ["none", "low", "medium", "high", "xhigh"],
          baseInstructions: "Keep me",
        }),
      ]),
    );
  });

  it("preserves existing managed metadata when Copilot omits it", async () => {
    vi.mocked(copilotGetModelsForAccount).mockResolvedValue([
      {
        ...advertisedModel(),
        context_window: undefined,
      },
    ]);
    const props = {
      ...makeProps("copilot"),
      catalogModels: [
        {
          model: "model-1",
          contextWindow: 222_000,
          supportsParallelToolCalls: true,
          reasoningLevels: ["high", "max"],
        },
      ],
    };
    render(<Harness {...props} />);
    fireEvent.click(fetchButton());

    await waitFor(() =>
      expect(props.onCatalogModelsChange).toHaveBeenCalledWith([
        expect.objectContaining({
          model: "model-1",
          contextWindow: 222_000,
          supportsParallelToolCalls: true,
          reasoningLevels: ["high", "max"],
        }),
      ]),
    );
  });

  it("clears stale reasoning levels when Copilot explicitly reports none", async () => {
    vi.mocked(copilotGetModelsForAccount).mockResolvedValue([
      {
        ...advertisedModel(),
        reasoning_effort: [],
      },
    ]);
    const props = {
      ...makeProps("copilot"),
      catalogModels: [
        {
          model: "model-1",
          reasoningLevels: ["high", "max"],
        },
      ],
    };
    render(<Harness {...props} />);
    fireEvent.click(fetchButton());

    await waitFor(() =>
      expect(props.onCatalogModelsChange).toHaveBeenCalledWith([
        expect.objectContaining({
          model: "model-1",
          reasoningLevels: [],
        }),
      ]),
    );
  });

  it.each(providerKinds)("preserves %s preflight validation", (kind) => {
    render(
      <Harness
        {...makeProps(kind)}
        isCopilotAuthenticated={false}
        isXaiOauthAuthenticated={false}
        codexApiKey=""
        codexBaseUrl=""
      />,
    );
    fireEvent.click(fetchButton());

    expect(fetchMockFor(kind)).not.toHaveBeenCalled();
    expect(fetchButton()).toBeEnabled();
    if (kind === "config") {
      expect(showFetchModelsError).toHaveBeenCalledWith(
        null,
        expect.any(Function),
        { hasApiKey: false, hasBaseUrl: false },
      );
    } else {
      expect(toast.error).toHaveBeenCalledTimes(1);
    }
  });
  it("removes unchecked catalog rows and moves the default to the new first row", async () => {
    vi.mocked(fetchModelsForConfig).mockResolvedValue([
      { id: "model-1", ownedBy: null },
      { id: "model-2", ownedBy: null },
    ]);
    const props = {
      ...makeProps("config"),
      codexModel: "model-1",
      catalogModels: [
        { model: "model-1", contextWindow: 200_000 },
        { model: "model-2", contextWindow: 128_000 },
      ],
    };
    render(<Harness {...props} />);
    fireEvent.click(fetchButton());

    const dialog = await screen.findByRole("dialog");
    fireEvent.click(within(dialog).getByRole("checkbox", { name: "model-1" }));
    fireEvent.click(
      within(dialog).getByRole("button", {
        name: "providerForm.modelPicker.apply",
      }),
    );

    expect(props.onModelChange).toHaveBeenCalledWith("model-2");
    await waitFor(() =>
      expect(props.onCatalogModelsChange).toHaveBeenLastCalledWith([
        expect.objectContaining({ model: "model-2", contextWindow: 128_000 }),
      ]),
    );
  });
  it("keeps the success notification for the default model fetch button without a model list", async () => {
    vi.mocked(fetchModelsForConfig).mockResolvedValue([advertisedModel()]);
    render(
      <Harness {...makeProps("config")} onCatalogModelsChange={undefined} />,
    );
    fireEvent.click(fetchButton());

    await waitFor(() =>
      expect(toast.success).toHaveBeenCalledWith(
        "providerForm.fetchModelsSuccess",
      ),
    );
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
});

const reviewerFetchButton = () =>
  screen.getByRole("button", { name: "拉取模型", hidden: true });

describe("Codex approval reviewer model fetching", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    Element.prototype.scrollIntoView = vi.fn();
    vi.spyOn(console, "warn").mockImplementation(() => {});
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it.each(providerKinds)(
    "fetches %s reviewer choices without opening the catalog picker or changing catalog/main model",
    async (kind) => {
      fetchMockFor(kind).mockResolvedValue([advertisedModel("reviewer-1")]);
      const props = {
        ...makeProps(kind),
        onAutoReviewModelChange: vi.fn(),
        catalogModels: [{ model: "main-model", contextWindow: 200_000 }],
      };
      render(<Harness {...props} />);
      fireEvent.click(reviewerFetchButton());
      const reviewer = await screen.findByRole("option", {
        name: "reviewer-1",
      });
      expect(screen.getByRole("option", { name: "main-model" })).toBeVisible();
      expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
      expect(
        screen.queryByText("providerForm.modelPicker.apply"),
      ).not.toBeInTheDocument();
      expect(props.onCatalogModelsChange).not.toHaveBeenCalled();
      expect(props.onModelChange).not.toHaveBeenCalled();
      expect(props.onAutoReviewModelChange).not.toHaveBeenCalled();
      fireEvent.click(reviewer);
      expect(props.onAutoReviewModelChange).toHaveBeenCalledWith("reviewer-1");
      expect(props.onCatalogModelsChange).not.toHaveBeenCalled();
      expect(props.onModelChange).not.toHaveBeenCalled();
    },
  );

  it("uses the same URL, API key, full-URL option, UA and request headers as catalog fetching", async () => {
    vi.mocked(fetchModelsForConfig).mockResolvedValue([
      advertisedModel("reviewer-1"),
    ]);
    vi.mocked(modelFetchRequestHeaders).mockReturnValue({
      "X-Model-Scope": "reviewer",
    });
    const props = {
      ...makeProps("config"),
      codexBaseUrl: "https://example.com/custom/responses",
      isFullUrl: true,
      customUserAgent: "reviewer-client",
      localProxyHeadersOverride: '{"X-Model-Scope":"reviewer"}',
      onAutoReviewModelChange: vi.fn(),
    };
    render(<Harness {...props} />);
    fireEvent.click(reviewerFetchButton());
    await screen.findByRole("option", { name: "reviewer-1" });
    expect(modelFetchRequestHeaders).toHaveBeenCalledWith(
      props.localProxyHeadersOverride,
    );
    expect(fetchModelsForConfig).toHaveBeenCalledWith(
      props.codexBaseUrl,
      props.codexApiKey,
      true,
      undefined,
      "reviewer-client",
      { requestHeaders: { "X-Model-Scope": "reviewer" } },
    );
  });

  it.each(providerKinds)(
    "keeps a newer %s reviewer request loading when an old identity response finishes",
    async (kind) => {
      const oldRequest = pendingModels();
      const newRequest = pendingModels();
      fetchMockFor(kind)
        .mockReturnValueOnce(oldRequest.promise)
        .mockReturnValueOnce(newRequest.promise);
      const props = { ...makeProps(kind), onAutoReviewModelChange: vi.fn() };
      const { rerender } = render(<Harness {...props} />);
      fireEvent.click(reviewerFetchButton());
      rerender(
        <Harness
          {...props}
          codexApiKey="other-key"
          selectedGitHubAccountId="other-github-account"
          selectedXaiAccountId="other-xai-account"
        />,
      );
      expect(reviewerFetchButton()).toBeEnabled();
      fireEvent.click(reviewerFetchButton());
      await act(async () =>
        oldRequest.resolve([advertisedModel("stale-reviewer")]),
      );
      expect(reviewerFetchButton()).toBeDisabled();
      expect(
        screen.queryByRole("option", { name: "stale-reviewer" }),
      ).not.toBeInTheDocument();
      await act(async () =>
        newRequest.resolve([advertisedModel("new-reviewer")]),
      );
      expect(
        await screen.findByRole("option", { name: "new-reviewer" }),
      ).toBeVisible();
      expect(
        screen.queryByRole("option", { name: "stale-reviewer" }),
      ).not.toBeInTheDocument();
      expect(props.onCatalogModelsChange).not.toHaveBeenCalled();
      expect(props.onModelChange).not.toHaveBeenCalled();
      expect(props.onAutoReviewModelChange).not.toHaveBeenCalled();
    },
  );

  it.each(providerKinds)(
    "ignores stale %s reviewer failures after credentials change",
    async (kind) => {
      const pending = pendingModels();
      fetchMockFor(kind).mockReturnValueOnce(pending.promise);
      const props = { ...makeProps(kind), onAutoReviewModelChange: vi.fn() };
      const { rerender } = render(<Harness {...props} />);
      fireEvent.click(reviewerFetchButton());
      rerender(
        <Harness
          {...props}
          codexApiKey="other-key"
          selectedGitHubAccountId="other-github-account"
          selectedXaiAccountId="other-xai-account"
        />,
      );
      await act(async () =>
        pending.reject(new Error("stale unauthorized response")),
      );
      expect(reviewerFetchButton()).toBeEnabled();
      expect(showFetchModelsError).not.toHaveBeenCalled();
      expect(console.warn).not.toHaveBeenCalled();
      expect(screen.queryByRole("option")).not.toBeInTheDocument();
    },
  );

  it.each(providerKinds)(
    "preserves %s reviewer fetch preflight validation",
    (kind) => {
      const props = {
        ...makeProps(kind),
        isCopilotAuthenticated: false,
        isXaiOauthAuthenticated: false,
        codexApiKey: "",
        codexBaseUrl: "",
        onAutoReviewModelChange: vi.fn(),
      };
      render(<Harness {...props} />);
      fireEvent.click(reviewerFetchButton());
      expect(fetchMockFor(kind)).not.toHaveBeenCalled();
      expect(reviewerFetchButton()).toBeEnabled();
      expect(props.onAutoReviewModelChange).not.toHaveBeenCalled();
    },
  );

  it.each(["official", "stack"] as const)(
    "discards pending reviewer results after switching to %s and back",
    async (mode) => {
      const pending = pendingModels();
      vi.mocked(fetchModelsForConfig).mockReturnValueOnce(pending.promise);
      const props = {
        ...makeProps("config"),
        onAutoReviewModelChange: vi.fn(),
      };
      const { rerender } = render(<Harness {...props} />);
      fireEvent.click(reviewerFetchButton());
      const hiddenMode: Partial<Props> =
        mode === "official" ? { category: "official" } : { variant: "stack" };
      rerender(<Harness {...props} {...hiddenMode} />);
      expect(
        document.getElementById("codexAutoReviewModel"),
      ).not.toBeInTheDocument();
      rerender(<Harness {...props} />);
      expect(reviewerFetchButton()).toBeEnabled();
      await act(async () =>
        pending.resolve([advertisedModel("stale-reviewer")]),
      );
      expect(screen.queryByRole("option")).not.toBeInTheDocument();
      expect(
        screen.queryByRole("button", { name: "选择审批模型" }),
      ).not.toBeInTheDocument();
      expect(props.onCatalogModelsChange).not.toHaveBeenCalled();
      expect(props.onModelChange).not.toHaveBeenCalled();
      expect(props.onAutoReviewModelChange).not.toHaveBeenCalled();
    },
  );

  it.each(["official", "stack"] as const)(
    "closes and clears fetched reviewer choices after switching to %s and back",
    async (mode) => {
      vi.mocked(fetchModelsForConfig).mockResolvedValueOnce([
        advertisedModel("stale-reviewer"),
      ]);
      const props = {
        ...makeProps("config"),
        onAutoReviewModelChange: vi.fn(),
      };
      const { rerender } = render(<Harness {...props} />);
      fireEvent.click(reviewerFetchButton());
      expect(
        await screen.findByRole("option", { name: "stale-reviewer" }),
      ).toBeVisible();
      const hiddenMode: Partial<Props> =
        mode === "official" ? { category: "official" } : { variant: "stack" };
      rerender(<Harness {...props} {...hiddenMode} />);
      rerender(<Harness {...props} />);
      expect(reviewerFetchButton()).toBeEnabled();
      expect(screen.queryByRole("option")).not.toBeInTheDocument();
      expect(
        screen.queryByRole("button", { name: "选择审批模型" }),
      ).not.toBeInTheDocument();
      expect(props.onCatalogModelsChange).not.toHaveBeenCalled();
      expect(props.onModelChange).not.toHaveBeenCalled();
      expect(props.onAutoReviewModelChange).not.toHaveBeenCalled();
    },
  );
});
