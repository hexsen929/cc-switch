import type { ReactNode } from "react";
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
import {
  skillsApi,
  type InstalledSkill,
  type SkillApps,
} from "@/lib/api/skills";
import type { ProviderResourceOverrides } from "@/types";

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

const createClient = () =>
  new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });

const renderPanel = (
  value: ProviderResourceOverrides,
  onChange: (next: ProviderResourceOverrides) => void,
  ui: (children: ReactNode) => ReactNode = (children) => children,
) =>
  render(
    <QueryClientProvider client={createClient()}>
      {ui(
        <ProviderResourceOverridesConfig
          appId="claude"
          value={value}
          onChange={onChange}
        />,
      )}
    </QueryClientProvider>,
  );

const skillApps = (overrides: Partial<SkillApps>): SkillApps => ({
  claude: false,
  codex: false,
  gemini: false,
  opencode: false,
  openclaw: false,
  hermes: false,
  pi: false,
  ...overrides,
});

const skill = (id: string, name: string, apps: SkillApps): InstalledSkill => ({
  id,
  name,
  directory: id,
  apps,
  installedAt: 0,
  updatedAt: 0,
});

const GLOBALLY_ON = skill(
  "owner/repo:on",
  "Globally On Skill",
  skillApps({ claude: true }),
);
const GLOBALLY_OFF = skill(
  "owner/repo:off",
  "Globally Off Skill",
  skillApps({ codex: true }),
);

function skillRowCheckbox(name: string) {
  const row = screen.getByText(name).closest("label");
  if (!row) throw new Error(`no checklist row for ${name}`);
  return within(row).getByRole("checkbox");
}

describe("ProviderResourceOverridesConfig 供应商级 Skill 启用名单", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    vi.spyOn(mcpApi, "getAllServers").mockResolvedValue({});
    vi.spyOn(promptsApi, "getPrompts").mockResolvedValue({});
    vi.spyOn(skillsApi, "getInstalled").mockResolvedValue([
      GLOBALLY_ON,
      GLOBALLY_OFF,
    ]);
  });

  it("把 Skills 管理里关掉的条目列进「全局已关闭」分组", async () => {
    renderPanel({ skills: { enabled: true } }, vi.fn());

    await waitFor(() => {
      expect(screen.getByText("Globally Off Skill")).toBeInTheDocument();
    });
    expect(screen.getByText("Globally On Skill")).toBeInTheDocument();
    expect(screen.getByText("全局已关闭 1 项")).toBeInTheDocument();
    expect(screen.getByText("全局已启用 1 项")).toBeInTheDocument();
  });

  it("勾选后写入 enabledSkillIds", async () => {
    const onChange = vi.fn();
    renderPanel({ skills: { enabled: true } }, onChange);

    await waitFor(() => {
      expect(screen.getByText("Globally Off Skill")).toBeInTheDocument();
    });
    fireEvent.click(skillRowCheckbox("Globally Off Skill"));

    expect(onChange).toHaveBeenCalledTimes(1);
    expect(onChange.mock.calls[0][0].skills).toMatchObject({
      enabled: true,
      enabledSkillIds: ["owner/repo:off"],
      disabledSkillIds: [],
    });
  });

  it("勾选启用时把同一个 ID 从禁用名单里摘掉", async () => {
    // 全局状态翻转过的存量配置里，同一个 ID 可能还留在 disabledSkillIds。
    // 若不摘掉，后端「禁用优先」会让这次勾选看起来生效实际不生效。
    const onChange = vi.fn();
    renderPanel(
      {
        skills: {
          enabled: true,
          disabledSkillIds: ["owner/repo:off", "owner/repo:other"],
        },
      },
      onChange,
    );

    await waitFor(() => {
      expect(screen.getByText("Globally Off Skill")).toBeInTheDocument();
    });
    fireEvent.click(skillRowCheckbox("Globally Off Skill"));

    expect(onChange.mock.calls[0][0].skills).toMatchObject({
      enabledSkillIds: ["owner/repo:off"],
      disabledSkillIds: ["owner/repo:other"],
    });
  });

  it("禁用全局启用项时把同一个 ID 从启用名单里摘掉", async () => {
    const onChange = vi.fn();
    renderPanel(
      {
        skills: {
          enabled: true,
          enabledSkillIds: ["owner/repo:on"],
        },
      },
      onChange,
    );

    await waitFor(() => {
      expect(screen.getByText("Globally On Skill")).toBeInTheDocument();
    });
    fireEvent.click(skillRowCheckbox("Globally On Skill"));

    expect(onChange.mock.calls[0][0].skills).toMatchObject({
      disabledSkillIds: ["owner/repo:on"],
      enabledSkillIds: [],
    });
  });

  it("覆盖开关关闭时不渲染任何 Skill 名单", async () => {
    renderPanel({ skills: { enabled: false } }, vi.fn());

    await waitFor(() => {
      expect(skillsApi.getInstalled).toHaveBeenCalled();
    });
    expect(screen.queryByText("Globally Off Skill")).not.toBeInTheDocument();
    expect(screen.queryByText("全局已关闭 1 项")).not.toBeInTheDocument();
  });
});
