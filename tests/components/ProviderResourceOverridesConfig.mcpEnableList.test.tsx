import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ProviderResourceOverridesConfig } from "@/components/providers/forms/ProviderResourceOverridesConfig";
import { mcpApi } from "@/lib/api/mcp";
import { promptsApi } from "@/lib/api/prompts";
import { skillsApi } from "@/lib/api/skills";
import type { AppId } from "@/lib/api/types";
import type { McpServer, ProviderResourceOverrides } from "@/types";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (_key: string, options?: { defaultValue?: string; count?: number }) => {
      const template = options?.defaultValue ?? "";
      return options?.count === undefined
        ? template
        : template.replace("{{count}}", String(options.count));
    },
  }),
}));

function renderPanel(
  value: ProviderResourceOverrides,
  onChange = vi.fn(),
  appId: AppId = "claude",
) {
  return render(
    <QueryClientProvider
      client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
    >
      <ProviderResourceOverridesConfig
        appId={appId}
        value={value}
        onChange={onChange}
      />
    </QueryClientProvider>,
  );
}

function server(id: string, enabledApps: AppId[] = []): McpServer {
  return {
    id,
    name: id,
    server: { command: "fixture-command" },
    apps: {
      claude: enabledApps.includes("claude"),
      codex: enabledApps.includes("codex"),
      gemini: enabledApps.includes("gemini"),
      grokbuild: enabledApps.includes("grokbuild"),
      opencode: false,
      openclaw: false,
      hermes: false,
    },
  };
}

function checkbox(id: string) {
  const row = screen.getAllByText(id)[0].closest("label");
  if (!row) throw new Error(`Missing MCP row: ${id}`);
  return within(row).getByRole("checkbox");
}

describe("ProviderResourceOverridesConfig 供应商级 MCP 启用名单", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    vi.spyOn(mcpApi, "getAllServers").mockResolvedValue({
      "globally-on": server("globally-on", ["claude"]),
      "globally-off": server("globally-off", ["codex"]),
      "all-off": server("all-off"),
    });
    vi.spyOn(promptsApi, "getPrompts").mockResolvedValue({});
    vi.spyOn(skillsApi, "getInstalled").mockResolvedValue([]);
  });

  it("按当前应用分组，也列出所有应用都关闭的 MCP", async () => {
    renderPanel({ mcp: { enabled: true } });
    await waitFor(() => expect(checkbox("globally-off")).toBeInTheDocument());
    expect(checkbox("all-off")).toBeInTheDocument();
    expect(checkbox("globally-on")).toBeInTheDocument();
    expect(screen.getByText("全局已启用 1 项")).toBeInTheDocument();
    expect(screen.getByText("全局已关闭 2 项")).toBeInTheDocument();
  });

  it("勾选只修改供应商名单，不修改 MCP 全局开关", async () => {
    const onChange = vi.fn();
    const toggle = vi.spyOn(mcpApi, "toggleApp");
    renderPanel({ mcp: { enabled: true } }, onChange);
    await waitFor(() => expect(checkbox("globally-off")).toBeInTheDocument());
    fireEvent.click(checkbox("globally-off"));
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(onChange.mock.calls[0][0].mcp).toEqual({
      enabled: true,
      enabledServerIds: ["globally-off"],
      disabledServerIds: [],
    });
    expect(toggle).not.toHaveBeenCalled();
  });

  it("取消额外启用时保留其他名单", async () => {
    const onChange = vi.fn();
    renderPanel(
      {
        mcp: {
          enabled: true,
          enabledServerIds: ["globally-off", "all-off"],
          disabledServerIds: ["globally-on"],
        },
      },
      onChange,
    );
    await waitFor(() => expect(checkbox("globally-off")).toBeChecked());
    fireEvent.click(checkbox("globally-off"));
    expect(onChange.mock.calls[0][0].mcp).toMatchObject({
      enabledServerIds: ["all-off"],
      disabledServerIds: ["globally-on"],
    });
  });

  it("启用时移除同 ID 的旧禁用记录", async () => {
    const onChange = vi.fn();
    renderPanel(
      { mcp: { enabled: true, disabledServerIds: ["globally-off", "other"] } },
      onChange,
    );
    await waitFor(() => expect(checkbox("globally-off")).toBeInTheDocument());
    fireEvent.click(checkbox("globally-off"));
    expect(onChange.mock.calls[0][0].mcp).toMatchObject({
      enabledServerIds: ["globally-off"],
      disabledServerIds: ["other"],
    });
  });

  it("禁用时移除同 ID 的旧启用记录", async () => {
    const onChange = vi.fn();
    renderPanel(
      { mcp: { enabled: true, enabledServerIds: ["globally-on"] } },
      onChange,
    );
    await waitFor(() => expect(checkbox("globally-on")).toBeInTheDocument());
    fireEvent.click(checkbox("globally-on"));
    expect(onChange.mock.calls[0][0].mcp).toMatchObject({
      enabledServerIds: [],
      disabledServerIds: ["globally-on"],
    });
  });

  it("关闭覆盖时隐藏全部 MCP 名单", async () => {
    renderPanel({ mcp: { enabled: false, enabledServerIds: ["globally-off"] } });
    await waitFor(() => expect(mcpApi.getAllServers).toHaveBeenCalled());
    expect(screen.queryByText("全局已关闭 2 项")).not.toBeInTheDocument();
    expect(screen.queryByText("globally-off")).not.toBeInTheDocument();
  });

  it.each<AppId>(["claude", "codex", "gemini", "grokbuild"])(
    "%s 支持供应商单独启用",
    async (appId) => {
      renderPanel({ mcp: { enabled: true } }, vi.fn(), appId);
      await waitFor(() => expect(checkbox("all-off")).toBeInTheDocument());
      expect(screen.getByText("勾选表示只为此供应商启用")).toBeInTheDocument();
    },
  );

  it.each<AppId>([
    "pi",
    "omp",
    "agy",
    "opencode",
    "openclaw",
    "claude-desktop",
    "hermes",
    "mcode",
    "kimi",
    "dsh",
    "copilot",
    "workbuddy",
  ])("%s 不显示不会生效的供应商启用入口", async (appId) => {
    renderPanel({ mcp: { enabled: true } }, vi.fn(), appId);
    await waitFor(() => expect(mcpApi.getAllServers).toHaveBeenCalled());
    expect(screen.queryByText("勾选表示只为此供应商启用")).not.toBeInTheDocument();
    expect(screen.queryByText("all-off")).not.toBeInTheDocument();
  });
});
