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
    next.skills = {
      enabled: true,
      disabledSkillIds: (value.skills.disabledSkillIds ?? []).filter(Boolean),
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
