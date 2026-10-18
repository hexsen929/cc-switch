import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  ProviderForm,
  type ProviderFormValues,
} from "@/components/providers/forms/ProviderForm";
import type { ProviderCategory, ProviderMeta } from "@/types";
import type { AppMode } from "@/types/proxy";
import { createTestQueryClient } from "../utils/testQueryClient";

vi.mock("@/components/claude/ClaudeShellWrapperCard", () => ({
  ClaudeShellWrapperCard: () => <div data-testid="claude-shell-wrapper" />,
}));
vi.mock(
  "@/components/providers/forms/ClaudeAppendInstructionsFileField",
  () => ({
    ClaudeSystemInstructionsFileField: () => (
      <div data-testid="claude-system-instructions" />
    ),
    ClaudeAppendInstructionsFileField: () => (
      <div data-testid="claude-append-instructions" />
    ),
  }),
);
vi.mock("@/components/providers/forms/CodexInstructionsFileField", () => ({
  CodexInstructionsFileField: () => <div data-testid="codex-instructions" />,
}));
vi.mock("@/components/providers/forms/ModelAliasEditor", () => ({
  ModelAliasEditor: () => <div data-testid="exact-model-aliases" />,
}));

const settingsState = vi.hoisted(() => ({ enableStackMode: true }));

// 聚合版式只看应用当前是不是聚合模式（和应用页同一份 get_app_mode 数据）
vi.mock("@/lib/query/proxy", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/query/proxy")>();
  return {
    ...actual,
    useAppMode: () => ({
      data: {
        mode: settingsState.enableStackMode ? "stack" : "direct",
        attached: false,
        routeProviderId: null,
        directProviderId: null,
      },
    }),
  };
});

vi.mock("@/components/providers/forms/CodexConfigEditor", () => ({
  default: () => <div data-testid="codex-config-editor" />,
}));

vi.mock("@/components/providers/forms/hooks", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("@/components/providers/forms/hooks")>();
  const signedOut = {
    isAuthenticated: false,
    isStatusSuccess: true,
    isStatusError: false,
    accounts: [],
  };
  return {
    ...actual,
    useCopilotAuth: () => signedOut,
    useCodexOauth: () => signedOut,
    useXaiOauth: () => signedOut,
  };
});

const CLAUDE_ENV = {
  ANTHROPIC_BASE_URL: "https://relay.example",
  ANTHROPIC_AUTH_TOKEN: "sk-test",
  ANTHROPIC_MODEL: "m-a",
  ANTHROPIC_DEFAULT_SONNET_MODEL: "m-s[1M]",
  ANTHROPIC_DEFAULT_SONNET_MODEL_NAME: "Model S",
};

const CODEX_CONFIG =
  'model_provider = "custom"\nmodel = "m-1"\n\n[model_providers.custom]\nname = "custom"\nbase_url = "https://relay.example/v1"\nexperimental_bearer_token = "sk-test"\n';

function renderForm(
  appId: "claude" | "codex",
  onSubmit: (values: ProviderFormValues) => void,
  options: {
    meta?: ProviderMeta;
    modelAliases?: Record<string, string>;
    category?: ProviderCategory;
    codexCatalog?: Array<{ model: string; displayName?: string }>;
    codexAutoReviewModel?: string;
    codexLegacyAutoReviewModel?: string;
    codexInstructionsFiles?: string[];
    codexActiveInstructionsFile?: string;
    modeView?: AppMode;
    onStackLayoutChange?: (stackLayout: boolean) => void;
  } = {},
) {
  const settingsConfig =
    appId === "claude"
      ? { env: CLAUDE_ENV, modelAliases: options.modelAliases }
      : {
          modelAliases: options.modelAliases,
          ...(options.codexAutoReviewModel !== undefined
            ? { autoReviewModel: options.codexAutoReviewModel }
            : {}),
          auth: {},
          config: options.codexActiveInstructionsFile
            ? `model_instructions_file = ${JSON.stringify(options.codexActiveInstructionsFile)}\n${CODEX_CONFIG}`
            : CODEX_CONFIG,
          modelInstructionsFiles: options.codexInstructionsFiles,
          ...(options.codexCatalog
            ? {
                modelCatalog: {
                  models: options.codexCatalog,
                  ...(options.codexLegacyAutoReviewModel !== undefined
                    ? { autoReviewModel: options.codexLegacyAutoReviewModel }
                    : {}),
                },
              }
            : {}),
        };
  return render(
    <QueryClientProvider client={createTestQueryClient()}>
      <ProviderForm
        appId={appId}
        providerId="relay"
        submitLabel="save-provider"
        onSubmit={onSubmit}
        onCancel={vi.fn()}
        modeView={options.modeView}
        onStackLayoutChange={options.onStackLayoutChange}
        initialData={{
          name: "Relay",
          category: options.category ?? "third_party",
          settingsConfig,
          meta: options.meta,
        }}
      />
    </QueryClientProvider>,
  );
}

/** Codex 模型列表每一行的「实际请求模型」，按显示顺序。 */
function codexRowModels() {
  return screen
    .getAllByPlaceholderText("例如: deepseek-v4-flash")
    .map((input) => (input as HTMLInputElement).value);
}

async function save(onSubmit: ReturnType<typeof vi.fn>) {
  fireEvent.click(screen.getByRole("button", { name: "save-provider" }));
  await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1));
  const values = onSubmit.mock.calls[0][0] as ProviderFormValues;
  return { values, settings: JSON.parse(values.settingsConfig) };
}

describe("ProviderForm Stack layout (Claude Code)", () => {
  beforeEach(() => {
    settingsState.enableStackMode = true;
  });

  it("开了 Stack 设置时默认用简化面板：模型列表跟着映射，没有映射和配置编辑器", () => {
    renderForm("claude", vi.fn());

    expect(screen.getByText("模型列表")).toBeInTheDocument();
    expect(screen.getByDisplayValue("m-a")).toBeInTheDocument();
    expect(screen.getByDisplayValue("m-s")).toBeInTheDocument();
    expect(screen.getByDisplayValue("Model S")).toBeInTheDocument();
    // 没有 ★：第一行是默认模型，写在「模型列表」后面的「?」里。
    expect(
      screen.getByRole("button", { name: "模型列表" }),
    ).toBeInTheDocument();
    expect(screen.queryByText("providerForm.modelMappingLabel")).toBeNull();
    expect(screen.queryByText("默认兜底模型")).toBeNull();
  });

  it("简化面板的说明收进「?」，高级选项折叠时不挂说明", () => {
    renderForm("claude", vi.fn());

    expect(
      screen.getByRole("button", { name: "上游格式" }),
    ).toBeInTheDocument();
    expect(screen.queryByText(/一般无需修改/)).toBeNull();
    fireEvent.click(
      screen.getByRole("button", {
        name: "providerForm.advancedOptionsToggle",
      }),
    );
    expect(
      screen.getByText("以下两项只在路由、聚合模式下生效。"),
    ).toBeVisible();
    expect(
      screen.getByRole("button", { name: "自定义 User-Agent" }),
    ).toBeInTheDocument();
  });

  it("没动过列表时保存不写 stackModels，ANTHROPIC_MODEL 不变", async () => {
    const onSubmit = vi.fn();
    renderForm("claude", onSubmit);

    const { values, settings } = await save(onSubmit);
    expect(values.meta?.stackModels).toBeUndefined();
    expect(settings.env.ANTHROPIC_MODEL).toBe("m-a");
  });

  it("删掉第一行，保存时 ANTHROPIC_MODEL 跟着新的第一行", async () => {
    const onSubmit = vi.fn();
    renderForm("claude", onSubmit);

    fireEvent.click(
      screen.getAllByRole("button", { name: "common.delete" })[0],
    );

    const { values, settings } = await save(onSubmit);
    expect(values.meta?.stackModels).toEqual([
      { model: "m-s", displayName: "Model S", oneM: true },
    ]);
    expect(settings.env.ANTHROPIC_MODEL).toBe("m-s[1M]");
    expect(settings.env.ANTHROPIC_DEFAULT_SONNET_MODEL).toBe("m-s[1M]");
  });

  it("行里配了列表就用它；编辑后保存，第一行没变时 ANTHROPIC_MODEL 也不变", async () => {
    const onSubmit = vi.fn();
    renderForm("claude", onSubmit, {
      meta: { stackModels: [{ model: "m-b", displayName: "B" }] },
    });

    expect(screen.getByDisplayValue("m-b")).toBeInTheDocument();
    expect(screen.queryByDisplayValue("m-a")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "手动添加" }));
    const inputs = screen.getAllByPlaceholderText("例如 deepseek-v4-pro");
    fireEvent.change(inputs[1], { target: { value: "m-c[1M]" } });

    const { values, settings } = await save(onSubmit);
    expect(values.meta?.stackModels).toEqual([
      { model: "m-b", displayName: "B" },
      { model: "m-c", oneM: true },
    ]);
    expect(settings.env.ANTHROPIC_MODEL).toBe("m-a");
  });

  it("清空列表后保存为空列表，不再回落到模型映射", async () => {
    const onSubmit = vi.fn();
    renderForm("claude", onSubmit);

    // 从最后一行删起：第一行一直没变，删光了也不碰 ANTHROPIC_MODEL。
    for (const button of screen
      .getAllByRole("button", { name: "common.delete" })
      .reverse()) {
      fireEvent.click(button);
    }
    expect(screen.queryByDisplayValue("m-a")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "save-provider" }));
    fireEvent.click(await screen.findByRole("button", { name: "仍要保存" }));

    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1));
    const values = onSubmit.mock.calls[0][0] as ProviderFormValues;
    expect(values.meta?.stackModels).toEqual([]);
    expect(JSON.parse(values.settingsConfig).env.ANTHROPIC_MODEL).toBe("m-a");
  });

  it("没开 Stack 设置时用完整表单，保存时原样保留列表", async () => {
    settingsState.enableStackMode = false;
    const onSubmit = vi.fn();
    renderForm("claude", onSubmit, {
      meta: { stackModels: [{ model: "m-b", oneM: true }] },
    });

    expect(screen.queryByText("模型列表")).toBeNull();
    const { values, settings } = await save(onSubmit);
    expect(values.meta?.stackModels).toEqual([{ model: "m-b", oneM: true }]);
    expect(settings.env.ANTHROPIC_MODEL).toBe("m-a");
  });

  it("官方供应商不用简化面板", () => {
    renderForm("claude", vi.fn(), { category: "official" });
    expect(screen.queryByText("模型列表")).toBeNull();
  });

  it("直连生效时从聚合那格打开：用简化面板", async () => {
    settingsState.enableStackMode = false;
    const onSubmit = vi.fn();
    renderForm("claude", onSubmit, { modeView: "stack" });

    expect(screen.getByText("模型列表")).toBeInTheDocument();
    expect(screen.queryByText("默认兜底模型")).toBeNull();
    // 没显示的模型映射原样保存
    const { settings } = await save(onSubmit);
    expect(settings.env.ANTHROPIC_DEFAULT_SONNET_MODEL).toBe("m-s[1M]");
    expect(settings.env.ANTHROPIC_MODEL).toBe("m-a");
  });

  it("聚合生效时从直连那格打开：用完整表单", () => {
    renderForm("claude", vi.fn(), { modeView: "direct" });
    expect(screen.queryByText("模型列表")).toBeNull();
  });

  it("把用不用简化面板报给页头，卸载时报 false", () => {
    const onStackLayoutChange = vi.fn();
    const { unmount } = renderForm("claude", vi.fn(), {
      modeView: "stack",
      onStackLayoutChange,
    });
    expect(onStackLayoutChange).toHaveBeenLastCalledWith(true);
    unmount();
    expect(onStackLayoutChange).toHaveBeenLastCalledWith(false);

    const fullForm = vi.fn();
    renderForm("claude", vi.fn(), {
      modeView: "direct",
      onStackLayoutChange: fullForm,
    });
    expect(fullForm).toHaveBeenLastCalledWith(false);
  });
});

describe("ProviderForm Stack layout (Codex)", () => {
  beforeEach(() => {
    settingsState.enableStackMode = true;
  });

  it.each(["m-2", "removed-model"])(
    "saves aliases and instructions alongside auto-review selection %s",
    async (autoReviewModel) => {
      const onSubmit = vi.fn();
      const aliases = { "codex-auto-review": "m-2" };
      renderForm("codex", onSubmit, {
        modelAliases: aliases,
        codexCatalog: [{ model: "m-1" }, { model: "m-2" }],
        codexAutoReviewModel: autoReviewModel,
        codexInstructionsFiles: ["./saved.md"],
        codexActiveInstructionsFile: "./active.md",
      });

      const { settings } = await save(onSubmit);

      expect(settings.modelAliases).toEqual(aliases);
      expect(settings.modelInstructionsFiles).toEqual([
        "./saved.md",
        "./active.md",
      ]);
      expect(settings.config).toContain(
        'model_instructions_file = "./active.md"',
      );
      expect(settings.modelCatalog.models).toEqual([
        { model: "m-1" },
        { model: "m-2" },
      ]);
      expect(settings.autoReviewModel).toBe(autoReviewModel);
      expect(settings.modelCatalog.autoReviewModel).toBeUndefined();
    },
  );

  it.each(["review-outside-list", ""])(
    "saves an independent reviewer %s without creating a model list",
    async (autoReviewModel) => {
      const onSubmit = vi.fn();
      renderForm("codex", onSubmit, {
        modeView: "direct",
        codexAutoReviewModel: autoReviewModel,
        codexCatalog: [],
      });
      const { settings } = await save(onSubmit);
      expect(settings.autoReviewModel).toBe(autoReviewModel);
      expect(settings.modelCatalog).toBeUndefined();
    },
  );

  it.each([false, true])(
    "keeps an untouched alias-only reviewer unchanged on ordinary save (alias chain: %s)",
    async (withChain) => {
      const onSubmit = vi.fn();
      const aliases = {
        "codex-auto-review": "review-model",
        ...(withChain ? { "review-model": "second-hop" } : {}),
      };
      renderForm("codex", onSubmit, {
        modeView: "direct",
        modelAliases: aliases,
      });
      expect(document.getElementById("codexAutoReviewModel")).toHaveValue(
        "review-model",
      );
      const { settings } = await save(onSubmit);
      expect(settings).not.toHaveProperty("autoReviewModel");
      expect(settings.modelCatalog).toBeUndefined();
      expect(settings.modelAliases).toEqual(aliases);
    },
  );

  it("persists an explicit reviewer reset while retaining the alias table", async () => {
    const onSubmit = vi.fn();
    const aliases = {
      "codex-auto-review": "review-model",
      "review-model": "second-hop",
    };
    renderForm("codex", onSubmit, {
      modeView: "direct",
      modelAliases: aliases,
    });
    fireEvent.change(document.getElementById("codexAutoReviewModel")!, {
      target: { value: "" },
    });
    const { settings } = await save(onSubmit);
    expect(settings.autoReviewModel).toBe("");
    expect(settings.modelAliases).toEqual(aliases);
    expect(settings.modelCatalog).toBeUndefined();
  });

  it.each(["independent", "legacy"])(
    "preserves an existing empty %s reviewer instead of reviving an alias",
    async (source) => {
      const onSubmit = vi.fn();
      const aliases = { "codex-auto-review": "old-reviewer" };
      renderForm("codex", onSubmit, {
        modeView: "direct",
        modelAliases: aliases,
        codexCatalog: [{ model: "m-1" }],
        ...(source === "independent"
          ? {
              codexAutoReviewModel: "",
              codexLegacyAutoReviewModel: "older-reviewer",
            }
          : { codexLegacyAutoReviewModel: "" }),
      });
      expect(document.getElementById("codexAutoReviewModel")).toHaveValue("");
      const { settings } = await save(onSubmit);
      expect(settings.autoReviewModel).toBe("");
      expect(settings.modelCatalog.autoReviewModel).toBeUndefined();
      expect(settings.modelAliases).toEqual(aliases);
    },
  );

  it("简化面板：模型列表在外面，没有配置编辑器；列表为空时显示要发布的默认模型", () => {
    renderForm("codex", vi.fn());

    expect(screen.getByText("模型列表")).toBeInTheDocument();
    expect(
      screen.getByText("未配置模型：聚合模式下只发布下面的默认模型。"),
    ).toBeInTheDocument();
    // 列表为空时聚合发布的就是 config.toml 的 model，框要看得见。
    expect(document.getElementById("codexDefaultModel")).not.toBeNull();
    expect(screen.queryByTestId("codex-config-editor")).toBeNull();
  });

  it("列表第一行是默认模型，保存时写进 model", async () => {
    const onSubmit = vi.fn();
    renderForm("codex", onSubmit, {
      codexCatalog: [
        { model: "m-1", displayName: "M1" },
        { model: "m-2", displayName: "M2" },
      ],
    });

    expect(
      screen.getByRole("button", { name: "模型列表" }),
    ).toBeInTheDocument();
    // 有列表时第一行就是默认模型，不再单独显示默认模型框。
    expect(document.getElementById("codexDefaultModel")).toBeNull();
    fireEvent.change(screen.getByDisplayValue("m-1"), {
      target: { value: "m-9" },
    });

    const { settings } = await save(onSubmit);
    expect(
      settings.modelCatalog.models.map(
        (model: { model: string }) => model.model,
      ),
    ).toEqual(["m-9", "m-2"]);
    expect(settings.config).toContain('model = "m-9"');
  });

  it("加载时把原来的 model 挪到第一行显示，不改动保存时 model 和顺序都不变", async () => {
    const onSubmit = vi.fn();
    renderForm("codex", onSubmit, {
      codexCatalog: [
        { model: "m-2", displayName: "M2" },
        { model: "m-1", displayName: "M1" },
      ],
    });

    expect(codexRowModels()).toEqual(["m-1", "m-2"]);

    const { settings } = await save(onSubmit);
    expect(
      settings.modelCatalog.models.map(
        (model: { model: string }) => model.model,
      ),
    ).toEqual(["m-2", "m-1"]);
    expect(settings.config).toContain('model = "m-1"');
  });

  it("默认模型不在列表里时提示，可以加进列表", async () => {
    const onSubmit = vi.fn();
    renderForm("codex", onSubmit, {
      codexCatalog: [{ model: "m-2", displayName: "M2" }],
    });

    expect(codexRowModels()).toEqual(["m-2"]);
    expect(
      screen.getByText(
        "默认模型 m-1 不在列表里：Codex 默认仍请求它，但 /model 里没有这一项。",
      ),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "加入列表" }));
    expect(codexRowModels()).toEqual(["m-1", "m-2"]);

    const { settings } = await save(onSubmit);
    expect(
      settings.modelCatalog.models.map(
        (model: { model: string }) => model.model,
      ),
    ).toEqual(["m-1", "m-2"]);
    expect(settings.config).toContain('model = "m-1"');
  });

  it("删掉默认那行，model 换成剩下的第一行", async () => {
    const onSubmit = vi.fn();
    renderForm("codex", onSubmit, {
      codexCatalog: [
        { model: "m-2", displayName: "M2" },
        { model: "m-1", displayName: "M1" },
        { model: "m-3", displayName: "M3" },
      ],
    });

    // 默认的 m-1 加载时已挪到第一行。
    expect(codexRowModels()).toEqual(["m-1", "m-2", "m-3"]);
    fireEvent.click(screen.getAllByRole("button", { name: "删除" })[0]);
    expect(codexRowModels()).toEqual(["m-2", "m-3"]);

    const { settings } = await save(onSubmit);
    expect(settings.config).toContain('model = "m-2"');
  });

  it("没开 Stack 设置时仍是完整表单", () => {
    settingsState.enableStackMode = false;
    renderForm("codex", vi.fn());
    // 完整表单也有模型列表，但说明是直连版的。
    expect(screen.queryByText(/选中后请求直达这家/)).toBeNull();
    expect(document.getElementById("codexDefaultModel")).not.toBeNull();
    expect(screen.getByTestId("codex-config-editor")).toBeInTheDocument();
  });
  it.each(["claude", "codex"] as const)(
    "%s Stack retains fork instructions, aliases and tool bridge",
    async (appId) => {
      const onSubmit = vi.fn();
      const aliases = { "codex-auto-review": "gpt-5.6-sol" };
      const meta: ProviderMeta = {
        apiFormat: "openai_chat",
        toolCallBridge: true,
        ...(appId === "claude"
          ? {
              claudeAppendInstructions: {
                files: ["./append.md"],
                activeFile: "./append.md",
              },
              claudeSystemInstructions: {
                files: ["./system.md"],
                activeFile: "./system.md",
              },
            }
          : {}),
      };
      renderForm(appId, onSubmit, { meta, modelAliases: aliases });
      expect(screen.getByTestId("exact-model-aliases")).toBeInTheDocument();
      expect(screen.getByLabelText("工具调用桥接")).toBeChecked();
      if (appId === "claude") {
        for (const id of [
          "claude-shell-wrapper",
          "claude-system-instructions",
          "claude-append-instructions",
        ]) {
          expect(screen.getByTestId(id)).toBeInTheDocument();
        }
      } else {
        expect(screen.getByTestId("codex-instructions")).toBeInTheDocument();
      }
      const { values, settings } = await save(onSubmit);
      expect(settings.modelAliases).toEqual(aliases);
      expect(values.meta?.toolCallBridge).toBe(true);
      if (appId === "claude") {
        expect(values.meta?.claudeAppendInstructions).toEqual(
          meta.claudeAppendInstructions,
        );
        expect(values.meta?.claudeSystemInstructions).toEqual(
          meta.claudeSystemInstructions,
        );
      }
    },
  );

  it("直连生效时从聚合那格打开：用简化面板", () => {
    settingsState.enableStackMode = false;
    renderForm("codex", vi.fn(), { modeView: "stack" });
    expect(screen.getByText("模型列表")).toBeInTheDocument();
    expect(screen.queryByTestId("codex-config-editor")).toBeNull();
  });
});
