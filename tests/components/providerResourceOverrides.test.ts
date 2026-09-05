import { describe, expect, it } from "vitest";

import { normalizeProviderResourceOverrides } from "@/components/providers/forms/providerResourceOverrides";

describe("normalizeProviderResourceOverrides", () => {
  it("保留供应商单独启用的 Skill 名单", () => {
    const result = normalizeProviderResourceOverrides({
      skills: {
        enabled: true,
        disabledSkillIds: ["off-for-this-provider"],
        enabledSkillIds: ["globally-off-but-wanted"],
      },
    });

    expect(result?.skills).toEqual({
      enabled: true,
      disabledSkillIds: ["off-for-this-provider"],
      enabledSkillIds: ["globally-off-but-wanted"],
    });
  });

  it("同一个 ID 同时出现在两张名单时只保留禁用项", () => {
    // 后端 is_effectively_enabled_for_app 以禁用为准，写盘时就把冲突项摘掉，
    // 避免配置里留下一条永远不生效的启用记录。
    const result = normalizeProviderResourceOverrides({
      skills: {
        enabled: true,
        disabledSkillIds: ["conflict"],
        enabledSkillIds: ["conflict", "kept"],
      },
    });

    expect(result?.skills?.disabledSkillIds).toEqual(["conflict"]);
    expect(result?.skills?.enabledSkillIds).toEqual(["kept"]);
  });

  it("覆盖关闭时整段丢弃，不残留启用名单", () => {
    const result = normalizeProviderResourceOverrides({
      skills: {
        enabled: false,
        enabledSkillIds: ["globally-off-but-wanted"],
      },
    });

    expect(result).toBeUndefined();
  });

  it("过滤空 ID", () => {
    const result = normalizeProviderResourceOverrides({
      skills: {
        enabled: true,
        disabledSkillIds: ["", "real-disabled"],
        enabledSkillIds: ["", "real-enabled"],
      },
    });

    expect(result?.skills?.disabledSkillIds).toEqual(["real-disabled"]);
    expect(result?.skills?.enabledSkillIds).toEqual(["real-enabled"]);
  });
});
