import { useMemo, type ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { FileText, Puzzle, ServerCog } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Checkbox } from "@/components/ui/checkbox";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { mcpApi } from "@/lib/api/mcp";
import { promptsApi } from "@/lib/api/prompts";
import { skillsApi } from "@/lib/api/skills";
import type { AppId } from "@/lib/api/types";
import { cn } from "@/lib/utils";
import type {
  McpServer,
  ProviderPromptOverrideMode,
  ProviderResourceOverrides,
} from "@/types";
import type { InstalledSkill } from "@/lib/api/skills";
import type { Prompt } from "@/lib/api/prompts";

const EMPTY_PROMPT_VALUE = "__cc_switch_provider_prompt_empty__";

const DEFAULT_OVERRIDES: Required<ProviderResourceOverrides> = {
  mcp: {
    enabled: false,
    disabledServerIds: [],
  },
  skills: {
    enabled: false,
    disabledSkillIds: [],
  },
  prompt: {
    enabled: false,
    mode: "selected",
    promptId: undefined,
  },
};

interface ProviderResourceOverridesConfigProps {
  appId: AppId;
  value?: ProviderResourceOverrides;
  onChange: (value: ProviderResourceOverrides) => void;
}

interface OverrideCardProps {
  icon: ReactNode;
  title: string;
  description: string;
  enabled: boolean;
  onEnabledChange: (enabled: boolean) => void;
  children?: ReactNode;
}

function OverrideCard({
  icon,
  title,
  description,
  enabled,
  onEnabledChange,
  children,
}: OverrideCardProps) {
  return (
    <div className="rounded-lg border border-border/50 bg-muted/20">
      <div className="flex items-start justify-between gap-4 p-4">
        <div className="flex gap-3">
          <div className="mt-0.5 text-muted-foreground">{icon}</div>
          <div className="space-y-1">
            <div className="font-medium">{title}</div>
            <p className="text-xs text-muted-foreground leading-5">
              {description}
            </p>
          </div>
        </div>
        <div className="flex items-center gap-2 shrink-0">
          <Label className="text-xs text-muted-foreground">
            {enabled ? "ON" : "OFF"}
          </Label>
          <Switch checked={enabled} onCheckedChange={onEnabledChange} />
        </div>
      </div>
      {enabled && children ? (
        <div className="border-t border-border/50 px-4 py-3">{children}</div>
      ) : null}
    </div>
  );
}

function ResourceChecklist<T extends { id: string; name: string }>({
  items,
  disabledIds,
  emptyLabel,
  onToggle,
}: {
  items: T[];
  disabledIds: string[];
  emptyLabel: string;
  onToggle: (id: string, disabled: boolean) => void;
}) {
  if (items.length === 0) {
    return <div className="text-xs text-muted-foreground">{emptyLabel}</div>;
  }

  return (
    <div className="max-h-56 overflow-y-auto space-y-2 pr-1">
      {items.map((item) => {
        const checked = disabledIds.includes(item.id);
        return (
          <label
            key={item.id}
            className={cn(
              "flex items-start gap-3 rounded-md border border-border/50 px-3 py-2 cursor-pointer",
              checked ? "bg-muted/40" : "bg-background/60",
            )}
          >
            <Checkbox
              checked={checked}
              onCheckedChange={(next) => onToggle(item.id, Boolean(next))}
              className="mt-0.5"
            />
            <div className="min-w-0 flex-1">
              <div className="text-sm font-medium break-all">{item.name}</div>
              <div className="text-xs text-muted-foreground break-all">
                {item.id}
              </div>
            </div>
          </label>
        );
      })}
    </div>
  );
}

export function ProviderResourceOverridesConfig({
  appId,
  value,
  onChange,
}: ProviderResourceOverridesConfigProps) {
  const { t } = useTranslation();

  const resolvedValue = useMemo(
    () => ({
      mcp: {
        ...DEFAULT_OVERRIDES.mcp,
        ...(value?.mcp ?? {}),
      },
      skills: {
        ...DEFAULT_OVERRIDES.skills,
        ...(value?.skills ?? {}),
      },
      prompt: {
        ...DEFAULT_OVERRIDES.prompt,
        ...(value?.prompt ?? {}),
      },
    }),
    [value],
  );

  const { data: mcpData, isLoading: isLoadingMcp } = useQuery({
    queryKey: ["mcp", "all"],
    queryFn: () => mcpApi.getAllServers(),
  });

  const { data: skillsData, isLoading: isLoadingSkills } = useQuery({
    queryKey: ["skills", "installed"],
    queryFn: () => skillsApi.getInstalled(),
  });

  const { data: promptsData, isLoading: isLoadingPrompts } = useQuery({
    queryKey: ["provider", "prompt-options", appId],
    queryFn: () => promptsApi.getPrompts(appId),
  });

  const availableMcps = useMemo<McpServer[]>(
    () =>
      Object.values(mcpData ?? {})
        .filter((server) => server.apps?.[appId] === true)
        .sort((a, b) => a.name.localeCompare(b.name)),
    [appId, mcpData],
  );

  const availableSkills = useMemo<InstalledSkill[]>(
    () =>
      (skillsData ?? [])
        .filter((skill) => skill.apps?.[appId] === true)
        .sort((a, b) => a.name.localeCompare(b.name)),
    [appId, skillsData],
  );

  const availablePrompts = useMemo<Prompt[]>(
    () =>
      Object.values(promptsData ?? {}).sort((a, b) =>
        a.name.localeCompare(b.name),
      ),
    [promptsData],
  );

  const globalPrompt = useMemo(
    () => availablePrompts.find((prompt) => prompt.enabled),
    [availablePrompts],
  );

  const updateValue = (next: ProviderResourceOverrides) => {
    onChange(next);
  };

  const updateMcpDisabledIds = (id: string, disabled: boolean) => {
    const currentIds = resolvedValue.mcp.disabledServerIds ?? [];
    const nextIds = disabled
      ? Array.from(new Set([...currentIds, id]))
      : currentIds.filter((item) => item !== id);
    updateValue({
      ...resolvedValue,
      mcp: {
        ...resolvedValue.mcp,
        disabledServerIds: nextIds,
      },
    });
  };

  const updateSkillDisabledIds = (id: string, disabled: boolean) => {
    const currentIds = resolvedValue.skills.disabledSkillIds ?? [];
    const nextIds = disabled
      ? Array.from(new Set([...currentIds, id]))
      : currentIds.filter((item) => item !== id);
    updateValue({
      ...resolvedValue,
      skills: {
        ...resolvedValue.skills,
        disabledSkillIds: nextIds,
      },
    });
  };

  const loadingLabel = t("common.loading", { defaultValue: "加载中..." });

  return (
    <div className="space-y-4">
      <div className="rounded-lg border border-border/50 bg-muted/10 p-4 space-y-1">
        <div className="font-medium">
          {t("providerOverrides.title", {
            defaultValue: "Provider 单独覆盖设置",
          })}
        </div>
        <p className="text-xs text-muted-foreground leading-5">
          {t("providerOverrides.description", {
            defaultValue:
              "关闭覆盖时沿用当前应用的全局 MCP / Skill / Prompt 设置；开启后只对这个供应商生效。",
          })}
        </p>
      </div>

      <OverrideCard
        icon={<ServerCog className="h-4 w-4" />}
        title={t("providerOverrides.mcpTitle", {
          defaultValue: "MCP 覆盖",
        })}
        description={t("providerOverrides.mcpDescription", {
          defaultValue: "基于全局启用列表，额外禁用这个供应商不应加载的 MCP。",
        })}
        enabled={resolvedValue.mcp.enabled}
        onEnabledChange={(enabled) =>
          updateValue({
            ...resolvedValue,
            mcp: {
              ...resolvedValue.mcp,
              enabled,
            },
          })
        }
      >
        <div className="mb-3 flex items-center gap-2">
          <Badge variant="outline">
            {t("providerOverrides.globalEnabledCount", {
              defaultValue: "全局已启用 {{count}} 项",
              count: availableMcps.length,
            })}
          </Badge>
          <span className="text-xs text-muted-foreground">
            {t("providerOverrides.checkedMeansDisabled", {
              defaultValue: "勾选表示对此供应商禁用",
            })}
          </span>
        </div>
        {isLoadingMcp ? (
          <div className="text-xs text-muted-foreground">{loadingLabel}</div>
        ) : (
          <ResourceChecklist
            items={availableMcps}
            disabledIds={resolvedValue.mcp.disabledServerIds ?? []}
            emptyLabel={t("providerOverrides.noMcp", {
              defaultValue: "当前应用还没有启用任何全局 MCP。",
            })}
            onToggle={updateMcpDisabledIds}
          />
        )}
      </OverrideCard>

      <OverrideCard
        icon={<Puzzle className="h-4 w-4" />}
        title={t("providerOverrides.skillTitle", {
          defaultValue: "Skill 覆盖",
        })}
        description={t("providerOverrides.skillDescription", {
          defaultValue:
            "基于全局启用列表，额外禁用这个供应商不应加载的 Skill。",
        })}
        enabled={resolvedValue.skills.enabled}
        onEnabledChange={(enabled) =>
          updateValue({
            ...resolvedValue,
            skills: {
              ...resolvedValue.skills,
              enabled,
            },
          })
        }
      >
        <div className="mb-3 flex items-center gap-2">
          <Badge variant="outline">
            {t("providerOverrides.globalEnabledCount", {
              defaultValue: "全局已启用 {{count}} 项",
              count: availableSkills.length,
            })}
          </Badge>
          <span className="text-xs text-muted-foreground">
            {t("providerOverrides.checkedMeansDisabled", {
              defaultValue: "勾选表示对此供应商禁用",
            })}
          </span>
        </div>
        {isLoadingSkills ? (
          <div className="text-xs text-muted-foreground">{loadingLabel}</div>
        ) : (
          <ResourceChecklist
            items={availableSkills}
            disabledIds={resolvedValue.skills.disabledSkillIds ?? []}
            emptyLabel={t("providerOverrides.noSkill", {
              defaultValue: "当前应用还没有启用任何全局 Skill。",
            })}
            onToggle={updateSkillDisabledIds}
          />
        )}
      </OverrideCard>

      <OverrideCard
        icon={<FileText className="h-4 w-4" />}
        title={t("providerOverrides.promptTitle", {
          defaultValue: "Prompt 覆盖",
        })}
        description={t("providerOverrides.promptDescription", {
          defaultValue: "为这个供应商单独指定 Prompt，或直接禁用 Prompt。",
        })}
        enabled={resolvedValue.prompt.enabled}
        onEnabledChange={(enabled) =>
          updateValue({
            ...resolvedValue,
            prompt: {
              ...resolvedValue.prompt,
              enabled,
            },
          })
        }
      >
        <div className="mb-3 flex items-center gap-2 text-xs text-muted-foreground">
          <Badge variant="outline">
            {t("providerOverrides.globalPrompt", {
              defaultValue: "全局 Prompt：{{name}}",
              name:
                globalPrompt?.name ??
                t("providerOverrides.none", { defaultValue: "未设置" }),
            })}
          </Badge>
        </div>

        {isLoadingPrompts ? (
          <div className="text-xs text-muted-foreground">{loadingLabel}</div>
        ) : (
          <div className="space-y-3">
            <div className="space-y-2">
              <Label>
                {t("providerOverrides.promptMode", {
                  defaultValue: "覆盖方式",
                })}
              </Label>
              <Select
                value={resolvedValue.prompt.mode ?? "selected"}
                onValueChange={(mode) =>
                  updateValue({
                    ...resolvedValue,
                    prompt: {
                      ...resolvedValue.prompt,
                      mode: mode as ProviderPromptOverrideMode,
                    },
                  })
                }
              >
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="selected">
                    {t("providerOverrides.promptModeSelected", {
                      defaultValue: "使用指定 Prompt",
                    })}
                  </SelectItem>
                  <SelectItem value="disabled">
                    {t("providerOverrides.promptModeDisabled", {
                      defaultValue: "不加载 Prompt",
                    })}
                  </SelectItem>
                </SelectContent>
              </Select>
            </div>

            {resolvedValue.prompt.mode !== "disabled" ? (
              <div className="space-y-2">
                <Label>
                  {t("providerOverrides.promptSelect", {
                    defaultValue: "选择 Prompt",
                  })}
                </Label>
                <Select
                  value={resolvedValue.prompt.promptId ?? EMPTY_PROMPT_VALUE}
                  onValueChange={(promptId) =>
                    updateValue({
                      ...resolvedValue,
                      prompt: {
                        ...resolvedValue.prompt,
                        promptId:
                          promptId === EMPTY_PROMPT_VALUE
                            ? undefined
                            : promptId,
                      },
                    })
                  }
                >
                  <SelectTrigger>
                    <SelectValue
                      placeholder={t("providerOverrides.promptPlaceholder", {
                        defaultValue: "请选择 Prompt",
                      })}
                    />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value={EMPTY_PROMPT_VALUE}>
                      {t("providerOverrides.followGlobalIfMissing", {
                        defaultValue: "未选择时回退到全局 Prompt",
                      })}
                    </SelectItem>
                    {availablePrompts.map((prompt) => (
                      <SelectItem key={prompt.id} value={prompt.id}>
                        {prompt.name}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                {availablePrompts.length === 0 ? (
                  <div className="text-xs text-muted-foreground">
                    {t("providerOverrides.noPrompt", {
                      defaultValue: "当前应用还没有可用 Prompt。",
                    })}
                  </div>
                ) : null}
              </div>
            ) : null}
          </div>
        )}
      </OverrideCard>
    </div>
  );
}
