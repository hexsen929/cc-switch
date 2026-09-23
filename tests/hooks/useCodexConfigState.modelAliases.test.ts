import { renderHook } from "@testing-library/react";
import { describe, it, expect } from "vitest";
import { useCodexConfigState } from "@/components/providers/forms/hooks/useCodexConfigState";
import { serializeModelAliases } from "@/utils/modelAliases";

// 回归：编辑已存在的 Codex 供应商时，读回精确模型别名（modelAliases）必须保留，
// 否则保存路径（ProviderForm 从零重建 configObj）会把它们丢掉。
// 注意：initialData 必须是稳定引用（hook 的 init effect 依赖 [initialData]）。
describe("useCodexConfigState modelAliases load", () => {
  it("loads modelAliases from settingsConfig into editable entries", () => {
    const initialData = {
      settingsConfig: {
        auth: { OPENAI_API_KEY: "sk-x" },
        config: "",
        modelAliases: {
          "codex-auto-review": "gpt-5.6-sol",
          "some-model": "other-model",
        },
      },
    };

    const { result } = renderHook(() => useCodexConfigState({ initialData }));

    expect(
      result.current.codexModelAliases.map((e) => ({
        source: e.source,
        target: e.target,
      })),
    ).toEqual([
      { source: "codex-auto-review", target: "gpt-5.6-sol" },
      { source: "some-model", target: "other-model" },
    ]);
    // load→serialize 回环无损
    expect(serializeModelAliases(result.current.codexModelAliases)).toEqual(
      initialData.settingsConfig.modelAliases,
    );
  });

  it("defaults to empty when modelAliases is absent", () => {
    const initialData = {
      settingsConfig: { auth: {}, config: "" },
    };
    const { result } = renderHook(() => useCodexConfigState({ initialData }));
    expect(result.current.codexModelAliases).toEqual([]);
  });
});
