import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SearchMcpPanel } from "@/components/settings/SearchMcpPanel";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  success: vi.fn(),
  error: vi.fn(),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@/lib/toast", () => ({
  toast: { success: mocks.success, error: mocks.error },
}));

const engines = ["tavily", "brave", "exa", "firecrawl", "you", "searxng"];
const sourceNames = [
  "Tavily",
  "Brave Search",
  "Exa",
  "Firecrawl",
  "You.com",
  "SearXNG",
];
const fixture = () => ({
  config: {
    enabled: false,
    sources: engines.map((engine) => ({
      engine,
      enabled: false,
      baseUrl: "",
      keys: [] as string[],
      orderedKeys: false,
      keyCount: engine === "tavily" ? 2 : 0,
      clearKeys: false,
    })),
    keyBackoff: true,
    timeoutSeconds: 30,
    maxResultChars: 20_000,
    dailyCallLimit: 1000,
  },
  endpoint: "http://127.0.0.1:15721/mcp/cc-switch/web-search",
  running: false,
});
type View = ReturnType<typeof fixture>;
type Config = View["config"];

const clone = <T,>(value: T): T => JSON.parse(JSON.stringify(value)) as T;
let serverView: View;
let savedConfigs: Config[];

const renderPanel = async () => {
  render(<SearchMcpPanel />);
  await screen.findByRole("switch", { name: "启用统一联网搜索 MCP" });
};
const keyInput = () => screen.getByLabelText("Tavily API 密钥");
const sourceCard = (name: string) => {
  const toggle = screen.getByRole("switch", { name });
  return within(toggle.parentElement!.parentElement!);
};
const save = async (expectedCount: number) => {
  fireEvent.click(screen.getByRole("button", { name: "保存" }));
  await waitFor(() => expect(savedConfigs).toHaveLength(expectedCount));
  await waitFor(() =>
    expect(screen.getByRole("button", { name: "保存" })).not.toBeDisabled(),
  );
};

describe("SearchMcpPanel", () => {
  beforeEach(() => {
    mocks.invoke.mockReset();
    mocks.success.mockReset();
    mocks.error.mockReset();
    serverView = fixture();
    savedConfigs = [];
    mocks.invoke.mockImplementation(
      async (command: string, payload?: { config?: Config }) => {
        if (command === "get_search_mcp_config") return clone(serverView);
        if (command === "set_search_mcp_config") {
          savedConfigs.push(clone(payload!.config!));
          return;
        }
        if (command === "add_search_mcp_to_apps") return;
        if (command === "get_search_mcp_quotas") return [];
        throw new Error(`Unexpected command: ${command}`);
      },
    );
  });

  it("starts disabled, names every switch and source input, and does not install on mount", async () => {
    await renderPanel();
    expect(
      screen.getByRole("switch", { name: "启用统一联网搜索 MCP" }),
    ).not.toBeChecked();
    for (const name of sourceNames) {
      expect(screen.getByRole("switch", { name })).not.toBeChecked();
      expect(screen.getByLabelText(`${name} 搜索源地址`)).toBeInTheDocument();
      expect(screen.getByLabelText(`${name} API 密钥`)).toHaveAttribute(
        "type",
        "password",
      );
    }
    expect(
      screen.getByRole("switch", { name: "被拒绝或限流的密钥暂时后置" }),
    ).toBeChecked();
    expect(
      screen.getByRole("button", { name: "保存并添加到 MCP" }),
    ).toBeDisabled();
    expect(mocks.invoke.mock.calls.map(([command]) => command)).toEqual([
      "get_search_mcp_config",
    ]);
  });

  it.each([
    [false, false, "搜索 MCP 未启用", "共享本地代理未运行"],
    [false, true, "搜索 MCP 未启用", "共享本地代理运行中"],
    [true, false, "搜索 MCP 已启用，代理未运行", "共享本地代理未运行"],
    [true, true, "搜索 MCP 已启用", "共享本地代理运行中"],
  ] as const)(
    "distinguishes saved search enabled=%s from proxy running=%s",
    async (enabled, running, searchLabel, proxyLabel) => {
      serverView.config.enabled = enabled;
      serverView.running = running;
      await renderPanel();
      const status = within(screen.getByRole("status"));
      expect(status.getByText(searchLabel)).toBeInTheDocument();
      expect(status.getByText(proxyLabel)).toBeInTheDocument();
      expect(mocks.invoke.mock.calls.map(([command]) => command)).toEqual([
        "get_search_mcp_config",
      ]);
    },
  );

  it.each([false, true])(
    "keeps saved enabled=%s until a pending toggle is successfully saved",
    async (enabled) => {
      serverView.config.enabled = enabled;
      serverView.running = true;
      const previous = enabled ? "搜索 MCP 已启用" : "搜索 MCP 未启用";
      const next = enabled ? "搜索 MCP 未启用" : "搜索 MCP 已启用";
      await renderPanel();
      fireEvent.click(
        screen.getByRole("switch", { name: "启用统一联网搜索 MCP" }),
      );
      expect(screen.getByText(previous)).toBeInTheDocument();
      expect(
        screen.getByText("启用状态尚未保存，点击保存后生效。"),
      ).toBeInTheDocument();

      const baseInvoke = mocks.invoke.getMockImplementation()!;
      mocks.invoke.mockImplementation(
        async (command: string, payload?: { config?: Config }) => {
          if (command === "set_search_mcp_config") {
            throw new Error("save failed");
          }
          return baseInvoke(command, payload);
        },
      );
      fireEvent.click(screen.getByRole("button", { name: "保存" }));
      await waitFor(() => expect(mocks.error).toHaveBeenCalled());
      expect(screen.getByText(previous)).toBeInTheDocument();

      mocks.invoke.mockImplementation(
        async (command: string, payload?: { config?: Config }) => {
          if (command === "set_search_mcp_config")
            serverView.config = clone(payload!.config!);
          return baseInvoke(command, payload);
        },
      );
      await save(1);
      expect(screen.getByText(next)).toBeInTheDocument();
      expect(
        screen.queryByText("启用状态尚未保存，点击保存后生效。"),
      ).not.toBeInTheDocument();
    },
  );

  it("does not report the old stopped proxy state after saving succeeds but refreshing fails", async () => {
    await renderPanel();
    const baseInvoke = mocks.invoke.getMockImplementation()!;
    mocks.invoke.mockImplementation(
      async (command: string, payload?: { config?: Config }) => {
        if (command === "get_search_mcp_config")
          throw new Error("refresh failed");
        return baseInvoke(command, payload);
      },
    );
    fireEvent.click(
      screen.getByRole("switch", { name: "启用统一联网搜索 MCP" }),
    );
    await save(1);
    expect(screen.getByText("搜索 MCP 已启用")).toBeInTheDocument();
    expect(screen.getByText("共享本地代理状态待刷新")).toBeInTheDocument();
    expect(screen.queryByText("共享本地代理未运行")).not.toBeInTheDocument();
    expect(mocks.error).toHaveBeenCalledWith(
      expect.stringContaining("refresh failed"),
    );
  });

  it("saves untouched redacted keys as preserve and never implicitly installs clients", async () => {
    await renderPanel();
    expect(keyInput()).toHaveValue("");
    expect(keyInput()).toHaveAttribute(
      "placeholder",
      expect.stringContaining("已保存 2 个密钥"),
    );
    await save(1);
    expect(savedConfigs[0].sources[0]).toMatchObject({
      engine: "tavily",
      keys: [],
      clearKeys: false,
    });
    expect(
      mocks.invoke.mock.calls.some(
        ([command]) => command === "add_search_mcp_to_apps",
      ),
    ).toBe(false);
    expect(keyInput()).toHaveValue("");
  });

  it("replaces entered keys, saves source priority, and makes deletion explicit", async () => {
    await renderPanel();
    fireEvent.change(keyInput(), {
      target: { value: "new-one, new-two  new-three" },
    });
    fireEvent.click(
      sourceCard("Brave Search").getByRole("button", { name: "提高优先级" }),
    );
    await save(1);
    expect(
      savedConfigs[0].sources.map((source) => source.engine).slice(0, 2),
    ).toEqual(["brave", "tavily"]);
    expect(
      savedConfigs[0].sources.find((source) => source.engine === "tavily"),
    ).toMatchObject({
      keys: ["new-one", "new-two", "new-three"],
      clearKeys: false,
    });
    expect(keyInput()).toHaveValue("");

    fireEvent.click(
      sourceCard("Tavily").getByRole("checkbox", { name: "清除已保存密钥" }),
    );
    expect(keyInput()).toBeDisabled();
    await save(2);
    expect(
      savedConfigs[1].sources.find((source) => source.engine === "tavily"),
    ).toMatchObject({ keys: [], clearKeys: true });
    expect(keyInput()).not.toBeDisabled();
  });

  it("refreshes saved server state and clears secret drafts when client installation fails", async () => {
    const baseInvoke = mocks.invoke.getMockImplementation()!;
    mocks.invoke.mockImplementation(
      async (command: string, payload?: { config?: Config }) => {
        if (command === "set_search_mcp_config") {
          serverView = fixture();
          serverView.config.enabled = true;
          serverView.config.sources[0].keyCount = 1;
          serverView.running = true;
          serverView.endpoint =
            "http://127.0.0.1:17890/mcp/cc-switch/web-search";
        }
        if (command === "add_search_mcp_to_apps")
          throw new Error("native MCP write failed");
        return baseInvoke(command, payload);
      },
    );
    await renderPanel();
    fireEvent.change(keyInput(), { target: { value: "replacement-secret" } });
    fireEvent.click(
      screen.getByRole("switch", { name: "启用统一联网搜索 MCP" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "保存并添加到 MCP" }));

    expect(await screen.findByText("共享本地代理运行中")).toBeInTheDocument();
    expect(screen.getByText("搜索 MCP 已启用")).toBeInTheDocument();
    expect(screen.getByText(serverView.endpoint)).toBeInTheDocument();
    expect(keyInput()).toHaveValue("");
    expect(keyInput()).toHaveAttribute(
      "placeholder",
      expect.stringContaining("已保存 1 个密钥"),
    );
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "保存" })).not.toBeDisabled(),
    );
    expect(mocks.error).toHaveBeenCalledWith(
      expect.stringContaining("搜索设置已保存，但添加到客户端失败"),
    );
    expect(mocks.success).not.toHaveBeenCalled();
    expect(mocks.invoke).toHaveBeenCalledWith("add_search_mcp_to_apps", {
      apps: ["claude", "codex", "gemini"],
    });
    expect(
      mocks.invoke.mock.calls.filter(
        ([command]) => command === "get_search_mcp_config",
      ),
    ).toHaveLength(2);
  });

  it("invalidates displayed saved-key quotas when key or endpoint drafts change", async () => {
    const baseInvoke = mocks.invoke.getMockImplementation()!;
    mocks.invoke.mockImplementation(
      async (command: string, payload?: { config?: Config }) =>
        command === "get_search_mcp_quotas"
          ? [
              {
                engine: "tavily",
                keyNumber: 1,
                known: true,
                used: 7,
                limit: 10,
              },
            ]
          : baseInvoke(command, payload),
    );
    await renderPanel();
    fireEvent.click(
      screen.getByRole("button", { name: "查询已保存密钥的额度" }),
    );
    expect(await screen.findByText(/7 \/ 10/)).toBeInTheDocument();
    expect(mocks.invoke).toHaveBeenCalledWith("get_search_mcp_quotas", {
      fresh: true,
    });
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "保存" })).not.toBeDisabled(),
    );
    fireEvent.change(keyInput(), { target: { value: "changed-secret" } });
    expect(screen.queryByText(/7 \/ 10/)).not.toBeInTheDocument();
    await save(1);
    expect(screen.queryByText(/7 \/ 10/)).not.toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("button", { name: "查询已保存密钥的额度" }),
    );
    expect(await screen.findByText(/7 \/ 10/)).toBeInTheDocument();
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "保存" })).not.toBeDisabled(),
    );
    fireEvent.change(screen.getByLabelText("Tavily 搜索源地址"), {
      target: { value: "https://relay.example" },
    });
    expect(screen.queryByText(/7 \/ 10/)).not.toBeInTheDocument();
  });
});
