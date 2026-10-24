import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_VISIBLE_APPS } from "@/config/appConfig";
import { UNKNOWN_PROJECT_DIR_KEY } from "@/components/sessions/utils";
import { SessionManagerPage } from "@/components/sessions/SessionManagerPage";
import { piApi } from "@/lib/api/pi";
import { sessionsApi } from "@/lib/api/sessions";
import type { SessionMessage, SessionMeta } from "@/types";
import { setSessionFixtures, setSettings } from "../msw/state";

const toastSuccessMock = vi.fn();
const toastErrorMock = vi.fn();
const platform = vi.hoisted(() => ({ mac: false }));

vi.mock("sonner", () => ({
  toast: {
    success: (...args: unknown[]) => toastSuccessMock(...args),
    error: (...args: unknown[]) => toastErrorMock(...args),
  },
}));

vi.mock("@/lib/platform", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/platform")>();
  return { ...actual, isMac: () => platform.mac };
});

// jsdom 没有布局，虚拟列表一条都不渲染；这里让它把全部条目都画出来
vi.mock("@tanstack/react-virtual", () => ({
  useVirtualizer: ({ count }: { count: number }) => ({
    getTotalSize: () => count * 100,
    getVirtualItems: () =>
      Array.from({ length: count }, (_, index) => ({
        index,
        key: index,
        start: index * 100,
      })),
    measureElement: () => undefined,
    scrollToIndex: () => undefined,
  }),
}));

const HOUR = 3600 * 1000;

const renderPage = (
  appId = "codex",
  props: Partial<Parameters<typeof SessionManagerPage>[0]> = {},
) => {
  const client = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false },
    },
  });

  return {
    client,
    ...render(
      <QueryClientProvider client={client}>
        <SessionManagerPage appId={appId} {...props} />
      </QueryClientProvider>,
    ),
  };
};

const sessionList = () => screen.getByRole("region", { name: "会话列表" });

const openRow = (title: string) =>
  fireEvent.click(within(sessionList()).getByRole("button", { name: title }));

const openAppMenu = async () =>
  userEvent.click(screen.getByRole("button", { name: /^应用：/ }));

const EXPANDED_KEY = "cc-switch.sessionManager.expandedProjects";

describe("SessionManagerPage", () => {
  beforeEach(() => {
    // 固定在本地正午，避免近期会话在午夜跨入昨天；异步交互仍使用真实计时器。
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(new Date(2026, 9, 18, 12));
    toastSuccessMock.mockReset();
    toastErrorMock.mockReset();
    platform.mac = false;
    window.localStorage.clear();
    // 项目分组默认收起；其余用例要直接看到会话，先把模拟项目都记成已展开
    window.localStorage.setItem(
      EXPANDED_KEY,
      JSON.stringify(["/mock/codex", "/mock/claude", UNKNOWN_PROJECT_DIR_KEY]),
    );
    Object.assign(navigator, {
      clipboard: { writeText: vi.fn().mockResolvedValue(undefined) },
    });

    const now = Date.now();
    const sessions: SessionMeta[] = [
      {
        providerId: "codex",
        sessionId: "codex-session-1",
        title: "Alpha Session",
        summary: "Alpha summary",
        projectDir: "/mock/codex",
        createdAt: now - 3 * HOUR,
        lastActiveAt: now - HOUR,
        sourcePath: "/mock/codex/session-1.jsonl",
        resumeCommand: "codex resume codex-session-1",
      },
      {
        providerId: "codex",
        sessionId: "codex-session-2",
        title: "Beta Session",
        summary: "Beta summary",
        projectDir: "/mock/codex",
        createdAt: now - 5 * HOUR,
        lastActiveAt: now - 2 * HOUR,
        sourcePath: "/mock/codex/session-2.jsonl",
        resumeCommand: "codex resume codex-session-2",
      },
      {
        providerId: "claude",
        sessionId: "claude-session-1",
        title: "Claude Session",
        summary: "Claude summary",
        projectDir: "/mock/claude",
        createdAt: now - 2 * HOUR,
        lastActiveAt: now - 30 * 60 * 1000,
        sourcePath: "/mock/claude/session-1.jsonl",
        resumeCommand: "claude --resume claude-session-1",
      },
      {
        providerId: "codex",
        sessionId: "codex-session-3",
        title: "Gamma Session",
        summary: "Gamma summary",
        projectDir: null,
        createdAt: now - 30 * 24 * HOUR,
        lastActiveAt: now - 30 * 24 * HOUR,
        sourcePath: "/mock/codex/archived_sessions/session-3.jsonl",
        resumeCommand: "codex resume codex-session-3",
      },
      {
        providerId: "hermes",
        sessionId: "hermes-1",
        title: "Hermes Session",
        projectDir: null,
        createdAt: now - 4 * HOUR,
        lastActiveAt: now - 4 * HOUR,
        sourcePath: "/mock/hermes/state.db",
      },
    ];
    const messages: Record<string, SessionMessage[]> = {
      "codex:/mock/codex/session-1.jsonl": [
        { role: "user", content: "alpha question", ts: now - 3 * HOUR },
        { role: "assistant", content: "[Tool: shell]" },
        { role: "tool", content: "output line" },
        {
          role: "assistant",
          content: "Here:\n```ts\nconst a = 1;\n```",
          ts: now - HOUR,
        },
      ],
      "codex:/mock/codex/session-2.jsonl": [
        { role: "user", content: "beta", ts: now - 2 * HOUR },
      ],
      "codex:/mock/codex/archived_sessions/session-3.jsonl": [
        { role: "user", content: "gamma", ts: now - 30 * 24 * HOUR },
      ],
      "claude:/mock/claude/session-1.jsonl": [
        { role: "user", content: "claude", ts: now },
      ],
      "hermes:/mock/hermes/state.db": [],
    };

    setSessionFixtures(sessions, messages);
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("shows Oh My Pi sessions from its app and in the all-apps filter", async () => {
    setSessionFixtures(
      [
        {
          providerId: "omp",
          sessionId: "omp-native-session",
          title: "Native OMP conversation",
          sourcePath: "/mock/omp/sessions/project/native.jsonl",
          lastActiveAt: Date.now(),
        },
      ],
      {},
    );
    setSettings({ visibleApps: { ...DEFAULT_VISIBLE_APPS, omp: true } });
    renderPage("omp");
    expect(
      await screen.findByText("Native OMP conversation"),
    ).toBeInTheDocument();
    await openAppMenu();
    const menu = await screen.findByRole("menu");
    expect(
      within(menu).getByRole("menuitemradio", { name: /Oh My Pi/ }),
    ).toBeInTheDocument();
    await userEvent.click(
      within(menu).getByRole("menuitemradio", { name: /全部应用/ }),
    );
    expect(
      await screen.findByText("Native OMP conversation"),
    ).toBeInTheDocument();
  });

  it("passes the native OMP source and identity when resuming a main session", async () => {
    platform.mac = true;
    window.localStorage.setItem("cc-switch.sessionManager.groupMode", "time");
    const session: SessionMeta = {
      providerId: "omp",
      sessionId: "omp-main",
      title: "Resume OMP Session",
      projectDir: "/mock/omp-project",
      sourcePath: "/mock/omp/sessions/project/native.jsonl",
      resumeCommand:
        "OMP_PROFILE=default omp --resume '/mock/omp/sessions/project/native.jsonl'",
      lastActiveAt: Date.now(),
    };
    setSessionFixtures([session], {
      "omp:/mock/omp/sessions/project/native.jsonl": [],
    });
    setSettings({ visibleApps: { ...DEFAULT_VISIBLE_APPS, omp: true } });
    const launch = vi
      .spyOn(sessionsApi, "launchTerminal")
      .mockResolvedValue(true);
    renderPage("omp");
    await screen.findByText("Resume OMP Session");
    openRow("Resume OMP Session");
    fireEvent.click(await screen.findByRole("button", { name: /中恢复$/ }));
    await waitFor(() =>
      expect(launch).toHaveBeenCalledWith({
        command: session.resumeCommand,
        cwd: session.projectDir,
        providerId: "omp",
        sessionId: "omp-main",
        sourcePath: session.sourcePath,
      }),
    );
    launch.mockRestore();
  });

  it("surfaces a relative Pi sessionDir instead of presenting an empty scan as authoritative", async () => {
    const discovery = vi.spyOn(piApi, "getSessionDiscovery").mockResolvedValue({
      status: "requires_project_context",
      configuredPath: ".pi/sessions",
    });

    renderPage("pi");

    expect(await screen.findByText(".pi/sessions")).toBeInTheDocument();
    expect(discovery).toHaveBeenCalledTimes(1);
    discovery.mockRestore();
  });

  it.each(["claude", "codex", "opencode"])(
    "%s nests child sessions and search opens their ancestors and descendants",
    async (providerId) => {
      window.localStorage.setItem("cc-switch.sessionManager.groupMode", "time");
      const now = Date.now();
      const treeSessions: SessionMeta[] = [
        {
          providerId,
          sessionId: "main",
          title: "Parent Session",
          projectDir: "/mock/tree",
          sourcePath: "/mock/tree/main.jsonl",
          lastActiveAt: now - HOUR,
        },
        {
          providerId,
          sessionId: "worker",
          parentSessionId: "main",
          role: "subagent",
          readOnly: true,
          title: "Worker Session",
          nickname: "Scout",
          agentType: "explorer",
          task: "needlewords",
          projectDir: "/mock/tree",
          sourcePath: "/mock/tree/worker.jsonl",
          lastActiveAt: now,
        },
        {
          providerId,
          sessionId: "nested",
          parentSessionId: "worker",
          role: "subagent",
          readOnly: true,
          title: "Nested Session",
          projectDir: "/mock/tree",
          sourcePath: "/mock/tree/nested.jsonl",
          lastActiveAt: now,
        },
      ];
      setSessionFixtures(treeSessions, {});
      renderPage(providerId);

      await screen.findByRole("button", { name: "Parent Session" });
      expect(
        screen.queryByRole("button", { name: "Worker Session" }),
      ).not.toBeInTheDocument();
      fireEvent.click(
        screen.getByRole("button", {
          name: "展开 Parent Session 的子代理会话",
        }),
      );
      expect(
        screen.getByRole("button", { name: "Worker Session" }),
      ).toBeInTheDocument();
      expect(
        screen.queryByRole("button", { name: "Nested Session" }),
      ).not.toBeInTheDocument();
      fireEvent.click(
        screen.getByRole("button", {
          name: "展开 Worker Session 的子代理会话",
        }),
      );
      expect(
        screen.getByRole("button", { name: "Nested Session" }),
      ).toBeInTheDocument();
      fireEvent.click(
        screen.getByRole("button", {
          name: "收起 Parent Session 的子代理会话",
        }),
      );
      expect(
        screen.queryByRole("button", { name: "Worker Session" }),
      ).not.toBeInTheDocument();

      fireEvent.change(screen.getByRole("textbox", { name: "搜索会话" }), {
        target: { value: "needlewords" },
      });
      expect(
        await screen.findByRole("button", { name: "Worker Session" }),
      ).toBeInTheDocument();
      expect(
        screen.getByRole("button", { name: "Parent Session" }),
      ).toBeInTheDocument();
      expect(
        screen.getByRole("button", { name: "Nested Session" }),
      ).toBeInTheDocument();
    },
  );

  it("keeps child sessions readable but blocks resume and batch selection", async () => {
    platform.mac = true;
    window.localStorage.setItem("cc-switch.sessionManager.groupMode", "time");
    const parent: SessionMeta = {
      providerId: "codex",
      sessionId: "main",
      title: "Parent Session",
      projectDir: "/mock/tree",
      sourcePath: "/mock/tree/main.jsonl",
    };
    const child: SessionMeta = {
      ...parent,
      sessionId: "child",
      parentSessionId: "main",
      role: "subagent",
      readOnly: true,
      title: "Read Only Worker",
      sourcePath: "/mock/tree/child.jsonl",
      resumeCommand: "must never launch",
    };
    setSessionFixtures([parent, child], {
      "codex:/mock/tree/child.jsonl": [
        { role: "user", content: "child question" },
        { role: "assistant", content: "child answer" },
      ],
    });
    const launch = vi
      .spyOn(sessionsApi, "launchTerminal")
      .mockResolvedValue(true);
    renderPage("codex");
    fireEvent.click(
      await screen.findByRole("button", {
        name: "展开 Parent Session 的子代理会话",
      }),
    );
    expect(
      screen.getByRole("checkbox", { name: "选择 Read Only Worker" }),
    ).toBeDisabled();
    expect(
      screen.queryByRole("button", { name: /中恢复 Read Only Worker/ }),
    ).not.toBeInTheDocument();
    openRow("Read Only Worker");
    expect(
      await screen.findByText("child question", { selector: "p" }),
    ).toBeInTheDocument();
    const resume = screen.getByRole("button", { name: /中恢复$/ });
    expect(resume).toHaveAttribute("aria-disabled", "true");
    fireEvent.click(resume);
    expect(launch).not.toHaveBeenCalled();
    await userEvent.click(
      screen.getByRole("button", { name: "Read Only Worker 的更多操作" }),
    );
    expect(screen.getByRole("menuitem", { name: "删除…" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    expect(
      screen.getByRole("menuitem", { name: "导出为 Markdown 文件" }),
    ).not.toHaveAttribute("aria-disabled", "true");
    fireEvent.keyDown(document.activeElement ?? document.body, {
      key: "Escape",
    });
    fireEvent.click(screen.getByRole("button", { name: "返回会话列表" }));
    fireEvent.click(
      within(sessionList()).getByRole("checkbox", {
        name: "选择 Parent Session",
      }),
    );
    fireEvent.click(screen.getByRole("button", { name: "全选匹配结果" }));
    expect(
      screen.getByRole("checkbox", { name: "选择 Parent Session" }),
    ).toBeChecked();
    expect(
      screen.getByRole("checkbox", { name: "选择 Read Only Worker" }),
    ).not.toBeChecked();
    launch.mockRestore();
  });

  it("lists the current app's sessions grouped by project with unknown last", async () => {
    renderPage("codex");

    await screen.findByRole("button", { name: "Alpha Session" });
    const headings = within(sessionList())
      .getAllByRole("heading", { level: 3 })
      .map((heading) => heading.textContent);
    expect(headings[0]).toContain("codex");
    expect(headings[1]).toContain("未知目录");
    expect(screen.queryByText("Claude Session")).not.toBeInTheDocument();
    // 归档的 Codex 会话带「已归档」
    expect(screen.getByText("已归档")).toBeInTheDocument();
    // 应用按钮写全称和数量
    expect(screen.getByRole("button", { name: /^应用：/ })).toHaveTextContent(
      "Codex3",
    );
  });

  it("collapses a project group and remembers it", async () => {
    renderPage("codex");

    const toggle = await screen.findByRole("button", { name: /^codex/ });
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    fireEvent.click(toggle);

    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByText("Alpha Session")).not.toBeInTheDocument();
    expect(window.localStorage.getItem(EXPANDED_KEY)).not.toContain(
      "/mock/codex",
    );
  });

  it("starts with project groups collapsed and remembers expanded ones", async () => {
    window.localStorage.removeItem(EXPANDED_KEY);
    renderPage("codex");

    const toggle = await screen.findByRole("button", { name: /^codex/ });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByText("Alpha Session")).not.toBeInTheDocument();

    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText("Alpha Session")).toBeInTheDocument();
    expect(window.localStorage.getItem(EXPANDED_KEY)).toContain("/mock/codex");
  });

  it("opens collapsed groups while searching", async () => {
    window.localStorage.removeItem(EXPANDED_KEY);
    renderPage("codex");

    await screen.findByRole("button", { name: /^codex/ });
    expect(screen.queryByText("Alpha Session")).not.toBeInTheDocument();

    fireEvent.change(screen.getByRole("textbox", { name: "搜索会话" }), {
      target: { value: "Alpha" },
    });
    // 标题里的匹配部分会被高亮拆开，按高亮的那段找
    expect(await screen.findByText("Alpha")).toBeInTheDocument();
  });

  it("switches to all apps and to time buckets", async () => {
    renderPage("codex");
    await screen.findByText("Alpha Session");

    await openAppMenu();
    const menu = await screen.findByRole("menu");
    const items = within(menu).getAllByRole("menuitemradio");
    // 全部应用 + 12 个来源，按会话数从多到少
    expect(items).toHaveLength(13);
    expect(items[0]).toHaveTextContent("全部应用5");
    expect(items[1]).toHaveTextContent("Codex3");
    expect(items[items.length - 1]).toHaveTextContent("无会话");
    await userEvent.click(items[0]);

    expect(await screen.findByText("Claude Session")).toBeInTheDocument();
    expect(screen.getByText("Codex 2")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "按时间" }));
    expect(
      within(sessionList()).getByRole("heading", { name: "今天" }),
    ).toBeInTheDocument();
    expect(
      within(sessionList()).getByRole("heading", { name: "更早" }),
    ).toBeInTheDocument();
  });

  it("only offers apps that are shown in the sidebar", async () => {
    setSettings({
      visibleApps: {
        claude: true,
        "claude-desktop": true,
        codex: true,
        gemini: false,
        grokbuild: false,
        opencode: false,
        openclaw: false,
        hermes: false,
        pi: false,
        mcode: false,
        kimi: false,
        agy: false,
        omp: false,
      },
    });
    renderPage("codex");
    await screen.findByText("Alpha Session");
    await waitFor(async () => {
      await openAppMenu();
      const menu = await screen.findByRole("menu");
      expect(within(menu).getAllByRole("menuitemradio")).toHaveLength(3);
    });
    const menu = screen.getByRole("menu");
    expect(within(menu).queryByText("Hermes")).not.toBeInTheDocument();
    // 隐藏应用的会话也不计入「全部应用」
    expect(within(menu).getAllByRole("menuitemradio")[0]).toHaveTextContent(
      "全部应用4",
    );
  });

  it("shows the Claude Desktop note when coming from Claude Desktop", async () => {
    renderPage("claude", { fromApp: "claude-desktop" });
    expect(
      await screen.findByText(
        "Claude Desktop 没有单独的会话记录，这里显示 Claude Code 的会话。",
      ),
    ).toBeInTheDocument();
  });

  it("filters by search and offers to clear an empty search", async () => {
    renderPage("codex");
    await screen.findByText("Alpha Session");

    const search = screen.getByRole("textbox", { name: "搜索会话" });
    fireEvent.change(search, { target: { value: "Beta" } });
    await waitFor(() =>
      expect(screen.queryByText("Alpha Session")).not.toBeInTheDocument(),
    );
    expect(screen.getByText("Beta")).toBeInTheDocument();

    fireEvent.change(search, { target: { value: "zzz" } });
    expect(
      await screen.findByText("没有标题、目录或首末消息匹配“zzz”的会话"),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "清除搜索" }));
    expect(await screen.findByText("Alpha Session")).toBeInTheDocument();
  });

  it("drills into a session, merges tool calls and steps between sessions", async () => {
    renderPage("codex");
    await screen.findByText("Alpha Session");

    openRow("Alpha Session");

    expect(
      await screen.findByRole("heading", { level: 1, name: "Alpha Session" }),
    ).toBeInTheDocument();
    // 返回和会话名并进页头：只剩一层页头，「会话」那条不再单独出现
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(screen.getByRole("button", { name: "返回会话列表" })).toBeVisible();
    // 正文和右侧对话目录各出现一次
    expect(await screen.findAllByText("alpha question")).toHaveLength(2);
    expect(
      within(screen.getByRole("navigation", { name: "对话目录" })).getByText(
        "alpha question",
      ),
    ).toBeInTheDocument();
    expect(screen.getByText("const a = 1;")).toBeInTheDocument();
    // 工具调用和输出合并进执行过程，默认折叠成一行摘要；展开后能看到
    expect(screen.queryByText("shell")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /^展开执行过程/ }));
    expect(screen.getByText("shell")).toBeInTheDocument();
    // 列表藏起来了
    expect(screen.queryByRole("region", { name: "会话列表" })).toBeNull();

    // 只看对话：工具调用隐藏
    fireEvent.click(screen.getByRole("button", { name: "对话" }));
    expect(screen.queryByText("shell")).not.toBeInTheDocument();
    expect(screen.getAllByText("alpha question")).toHaveLength(2);

    fireEvent.click(screen.getByRole("button", { name: "下一个会话" }));
    expect(
      await screen.findByRole("heading", { level: 1, name: "Beta Session" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "上一个会话" })).toBeEnabled();

    fireEvent.click(screen.getByRole("button", { name: "返回会话列表" }));
    expect(await screen.findByText("Alpha Session")).toBeInTheDocument();
  });

  it("explains why a Hermes session cannot be resumed", async () => {
    renderPage("hermes");
    await screen.findByText("Hermes Session");
    openRow("Hermes Session");

    expect(
      await screen.findByText("暂不支持从命令行恢复 Hermes 会话"),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "复制恢复命令" }),
    ).toHaveAttribute("aria-disabled", "true");
    expect(
      screen.getByText("Hermes 只显示最近 500 个会话"),
    ).toBeInTheDocument();
  });

  it("shows the read error instead of an empty session", async () => {
    const spy = vi
      .spyOn(sessionsApi, "getMessages")
      .mockRejectedValue(new Error("no such column: created_at"));
    renderPage("codex");
    await screen.findByText("Alpha Session");
    openRow("Alpha Session");

    expect(await screen.findByText("无法读取这个会话")).toBeInTheDocument();
    expect(screen.getByText("no such column: created_at")).toBeInTheDocument();
    spy.mockRestore();
  });

  it("launches the resume command in the preferred terminal on macOS", async () => {
    platform.mac = true;
    setSettings({ preferredTerminal: "iterm2" });
    const launch = vi
      .spyOn(sessionsApi, "launchTerminal")
      .mockResolvedValue(true);
    renderPage("codex");
    await screen.findByText("Alpha Session");
    openRow("Alpha Session");

    const resume = await screen.findByRole("button", {
      name: /中恢复$/,
    });
    fireEvent.click(resume);

    await waitFor(() =>
      expect(launch).toHaveBeenCalledWith({
        command: "codex resume codex-session-1",
        cwd: "/mock/codex",
        providerId: "codex",
        sessionId: "codex-session-1",
        sourcePath: "/mock/codex/session-1.jsonl",
      }),
    );
    expect(toastSuccessMock).toHaveBeenCalled();
    launch.mockRestore();
  });

  it("resumes AGY binary-only index sessions from both list and reader without a fake source", async () => {
    platform.mac = true;
    window.localStorage.setItem("cc-switch.sessionManager.groupMode", "time");
    const session: SessionMeta = {
      providerId: "agy",
      sessionId: "indexed-agy",
      title: "Indexed AGY Session",
      summary: "Native index summary only",
      projectDir: "/mock/agy",
      lastActiveAt: Date.now(),
      resumeCommand: "agy --conversation=indexed-agy",
    };
    setSessionFixtures([session], {});
    const launch = vi
      .spyOn(sessionsApi, "launchTerminal")
      .mockResolvedValue(true);
    const stream = vi.spyOn(sessionsApi, "streamMessages");
    const read = vi.spyOn(sessionsApi, "getMessages");
    renderPage("agy");

    fireEvent.click(
      await screen.findByRole("button", {
        name: /中恢复 Indexed AGY Session$/,
      }),
    );
    await waitFor(() => expect(launch).toHaveBeenCalledTimes(1));
    openRow("Indexed AGY Session");
    const resume = await screen.findByRole("button", { name: /中恢复$/ });
    expect(resume).not.toHaveAttribute("aria-disabled", "true");
    fireEvent.click(resume);
    await waitFor(() => expect(launch).toHaveBeenCalledTimes(2));
    for (const [options] of launch.mock.calls) {
      expect(options).toEqual({
        providerId: "agy",
        sessionId: "indexed-agy",
        command: "agy --conversation=indexed-agy",
        cwd: "/mock/agy",
        sourcePath: undefined,
      });
    }
    expect(stream).not.toHaveBeenCalled();
    expect(read).not.toHaveBeenCalled();
    expect(screen.getByText("这个会话没有可显示的消息")).toBeInTheDocument();
    await userEvent.click(
      screen.getByRole("button", { name: "Indexed AGY Session 的更多操作" }),
    );
    for (const name of [
      "复制整段为 Markdown",
      "导出为 Markdown 文件",
      "删除…",
    ]) {
      expect(screen.getByRole("menuitem", { name })).toHaveAttribute(
        "aria-disabled",
        "true",
      );
    }
    launch.mockRestore();
    stream.mockRestore();
    read.mockRestore();
  });

  it("copies an AGY index-only resume command where one-click resume is unavailable", async () => {
    window.localStorage.setItem("cc-switch.sessionManager.groupMode", "time");
    setSessionFixtures(
      [
        {
          providerId: "agy",
          sessionId: "indexed-copy",
          title: "Indexed AGY Copy",
          lastActiveAt: Date.now(),
          resumeCommand: "agy --conversation=indexed-copy",
        },
      ],
      {},
    );
    renderPage("agy");
    await screen.findByText("Indexed AGY Copy");
    openRow("Indexed AGY Copy");
    fireEvent.click(
      await screen.findByRole("button", { name: "复制恢复命令" }),
    );
    await waitFor(() =>
      expect(navigator.clipboard.writeText).toHaveBeenCalledWith(
        "agy --conversation=indexed-copy",
      ),
    );
  });

  it.each([
    { providerId: "codex", marker: {} },
    { providerId: "kimi", marker: {} },
    { providerId: "omp", marker: {} },
    { providerId: "agy", marker: { readOnly: true } },
    { providerId: "agy", marker: { role: "subagent" as const } },
    { providerId: "agy", marker: { parentSessionId: "missing-parent" } },
  ])(
    "does not resume a source-less $providerId session with marker $marker",
    async ({ providerId, marker }) => {
      platform.mac = true;
      window.localStorage.setItem("cc-switch.sessionManager.groupMode", "time");
      setSessionFixtures(
        [
          {
            providerId,
            sessionId: "blocked-index",
            title: "Blocked Index Session",
            lastActiveAt: Date.now(),
            resumeCommand: "must never launch",
            ...marker,
          },
        ],
        {},
      );
      const launch = vi
        .spyOn(sessionsApi, "launchTerminal")
        .mockResolvedValue(true);
      renderPage(providerId);
      await screen.findByText("Blocked Index Session");
      expect(
        screen.queryByRole("button", { name: /中恢复 Blocked Index Session$/ }),
      ).not.toBeInTheDocument();
      openRow("Blocked Index Session");
      const resume = await screen.findByRole("button", { name: /中恢复$/ });
      expect(resume).toHaveAttribute("aria-disabled", "true");
      fireEvent.click(resume);
      expect(launch).not.toHaveBeenCalled();
      launch.mockRestore();
    },
  );

  it("copies the resume command where one-click resume is unavailable", async () => {
    renderPage("codex");
    await screen.findByText("Alpha Session");
    openRow("Alpha Session");

    fireEvent.click(
      await screen.findByRole("button", { name: "复制恢复命令" }),
    );
    await waitFor(() =>
      expect(navigator.clipboard.writeText).toHaveBeenCalledWith(
        "codex resume codex-session-1",
      ),
    );
  });

  it("deletes the open session after confirming and returns to the list", async () => {
    renderPage("codex");
    await screen.findByText("Alpha Session");
    openRow("Alpha Session");
    await screen.findByRole("heading", { level: 1, name: "Alpha Session" });

    await userEvent.click(
      screen.getByRole("button", { name: "Alpha Session 的更多操作" }),
    );
    await userEvent.click(
      await screen.findByRole("menuitem", { name: "删除…" }),
    );

    const dialog = await screen.findByRole("alertdialog");
    expect(dialog).toHaveTextContent("删除会话「Alpha Session」？");
    expect(dialog).toHaveTextContent("只删除这一个会话文件");
    fireEvent.click(within(dialog).getByRole("button", { name: "删除会话" }));

    await waitFor(() =>
      expect(
        screen.queryByRole("heading", { level: 1, name: "Alpha Session" }),
      ).not.toBeInTheDocument(),
    );
    await waitFor(() =>
      expect(screen.queryByText("Alpha Session")).not.toBeInTheDocument(),
    );
    expect(screen.getByText("Beta Session")).toBeInTheDocument();
  });

  it("selects several sessions and deletes them in one batch", async () => {
    const deleteMany = vi.spyOn(sessionsApi, "deleteMany");
    renderPage("codex");
    await screen.findByText("Alpha Session");

    await userEvent.click(
      screen.getByRole("button", { name: "会话的更多操作" }),
    );
    await userEvent.click(
      await screen.findByRole("menuitem", { name: "选择多个…" }),
    );

    fireEvent.click(
      screen.getByRole("checkbox", { name: "选择 Alpha Session" }),
    );
    fireEvent.click(
      screen.getByRole("checkbox", { name: "选择 Beta Session" }),
    );
    expect(screen.getByText("已选 2 个会话")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "删除 2 个会话…" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog).toHaveTextContent("删除 2 个会话？");
    expect(dialog).toHaveTextContent("Codex 2 个");
    fireEvent.click(
      within(dialog).getByRole("button", { name: "删除 2 个会话" }),
    );

    await waitFor(() => expect(deleteMany).toHaveBeenCalledTimes(1));
    await waitFor(() =>
      expect(screen.queryByText("Alpha Session")).not.toBeInTheDocument(),
    );
    expect(screen.queryByText("Beta Session")).not.toBeInTheDocument();
    expect(screen.getByText("Gamma Session")).toBeInTheDocument();
    // 删完退出选择
    expect(screen.queryByText(/已选/)).not.toBeInTheDocument();
    deleteMany.mockRestore();
  });

  it("keeps the selection when the batch delete request fails", async () => {
    const deleteMany = vi
      .spyOn(sessionsApi, "deleteMany")
      .mockRejectedValue(new Error("boom"));
    renderPage("codex");
    await screen.findByText("Alpha Session");

    await userEvent.click(
      screen.getByRole("button", { name: "会话的更多操作" }),
    );
    await userEvent.click(
      await screen.findByRole("menuitem", { name: "选择多个…" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "全选匹配结果" }));
    expect(screen.getByText("已选 3 个会话")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "删除 3 个会话…" }));
    fireEvent.click(
      within(await screen.findByRole("alertdialog")).getByRole("button", {
        name: "删除 3 个会话",
      }),
    );

    await waitFor(() => expect(toastErrorMock).toHaveBeenCalled());
    expect(screen.getByText("已选 3 个会话")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "删除 3 个会话…" }),
    ).toBeEnabled();
    deleteMany.mockRestore();
  });

  it("drops hidden selections from the count and keeps search usable while selecting", async () => {
    renderPage("codex");
    await screen.findByText("Alpha Session");

    // 行首的复选框直接进入选择态
    fireEvent.click(
      screen.getAllByRole("checkbox", { name: "选择 Alpha Session" })[0],
    );
    expect(screen.getByText("已选 1 个会话")).toBeInTheDocument();

    fireEvent.change(screen.getByRole("textbox", { name: "搜索会话" }), {
      target: { value: "Beta" },
    });
    expect(await screen.findByText("匹配 1 个会话")).toBeInTheDocument();
    expect(screen.getByText(/其中 1 个不在当前结果里/)).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "退出选择" }));
    expect(screen.queryByText(/已选/)).not.toBeInTheDocument();
  });
});
