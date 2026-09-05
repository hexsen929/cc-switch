import type { ProviderResourceOverrides } from "@/types";

export function normalizeProviderResourceOverrides(
  value: ProviderResourceOverrides,
): ProviderResourceOverrides | undefined {
  const next: ProviderResourceOverrides = {};

  if (value.mcp?.enabled) {
    next.mcp = {
      enabled: true,
      disabledServerIds: (value.mcp.disabledServerIds ?? []).filter(Boolean),
    };
  }

  if (value.skills?.enabled) {
    const disabledSkillIds = (value.skills.disabledSkillIds ?? []).filter(
      Boolean,
    );
    next.skills = {
      enabled: true,
      disabledSkillIds,
      // 禁用优先与后端 is_effectively_enabled_for_app 对齐：同一个 ID 不能两张
      // 名单都留着，否则界面上勾了「为此供应商启用」却仍旧不生效。
      enabledSkillIds: (value.skills.enabledSkillIds ?? []).filter(
        (id) => Boolean(id) && !disabledSkillIds.includes(id),
      ),
    };
  }

  if (value.prompt?.enabled) {
    next.prompt = {
      enabled: true,
      mode: value.prompt.mode ?? "selected",
      promptId:
        (value.prompt.mode ?? "selected") === "selected"
          ? value.prompt.promptId || undefined
          : undefined,
    };
  }

  return Object.keys(next).length > 0 ? next : undefined;
}
