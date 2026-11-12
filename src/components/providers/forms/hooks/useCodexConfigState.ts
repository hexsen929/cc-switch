import { useState, useCallback, useEffect, useMemo, useRef } from "react";
import {
  extractCodexAgentsMaxThreads,
  extractCodexAgentsString,
  extractCodexBaseUrl,
  extractCodexExperimentalBearerToken,
  extractCodexModelName,
  extractCodexTopLevelString,
  normalizeCodexModelInstructionsFiles,
  removeCodexTopLevelField,
  extractCodexMultiAgentV2Total,
  setCodexAgentsMaxThreads,
  setCodexBaseUrl as setCodexBaseUrlInConfig,
  setCodexAgentsString,
  setCodexModelName as setCodexModelNameInConfig,
  setCodexTopLevelString,
  updateCodexExperimentalBearerToken,
} from "@/utils/providerConfigUtils";
import { normalizeTomlText } from "@/utils/textNormalization";
import type { CodexCatalogModel } from "@/types";
import {
  modelAliasesEqual,
  parseModelAliasesFromValue,
  serializeModelAliases,
  type ModelAliasEntry,
} from "@/utils/modelAliases";

const PROXY_MANAGED_PLACEHOLDER = "PROXY_MANAGED";
const MODEL_INSTRUCTIONS_FIELD = "model_instructions_file";

const isProxyManagedPlaceholder = (value: unknown): boolean =>
  typeof value === "string" && value.trim() === PROXY_MANAGED_PLACEHOLDER;

const normalizeUserApiKey = (value: unknown): string =>
  typeof value === "string" && !isProxyManagedPlaceholder(value)
    ? value.trim()
    : "";

function catalogModelsEqual(
  left: CodexCatalogModel[],
  right: CodexCatalogModel[],
): boolean {
  if (left.length !== right.length) return false;
  return left.every((item, index) => {
    const other = right[index];
    return (
      item.model === other.model &&
      (item.displayName ?? "") === (other.displayName ?? "") &&
      String(item.contextWindow ?? "") === String(other.contextWindow ?? "") &&
      (item.supportsParallelToolCalls ?? null) ===
        (other.supportsParallelToolCalls ?? null) &&
      (item.baseInstructions ?? "") === (other.baseInstructions ?? "") &&
      JSON.stringify(item.inputModalities ?? []) ===
        JSON.stringify(other.inputModalities ?? []) &&
      JSON.stringify(item.reasoningLevels ?? []) ===
        JSON.stringify(other.reasoningLevels ?? []) &&
      (item.defaultReasoningLevel ?? "") === (other.defaultReasoningLevel ?? "")
    );
  });
}

function stringArraysEqual(left: string[], right: string[]): boolean {
  return (
    left.length === right.length &&
    left.every((item, index) => item === right[index])
  );
}

interface UseCodexConfigStateProps {
  initialData?: {
    settingsConfig?: Record<string, unknown>;
  };
}

// auth.json 缺 OPENAI_API_KEY 时回退到 config.toml 的 experimental_bearer_token
// (Mobile 兼容形态：保留 ChatGPT 登录态但用第三方 token)
function pickCodexApiKey(
  authObj: { OPENAI_API_KEY?: unknown } | null | undefined,
  configText: string,
): string {
  if (authObj && typeof authObj.OPENAI_API_KEY === "string") {
    const key = normalizeUserApiKey(authObj.OPENAI_API_KEY);
    if (key) return key;
  }
  return extractCodexExperimentalBearerToken(configText) || "";
}

// 目录行 load 映射（隐藏字段白名单重建）。抽成可导出的纯函数，与
// normalizeCodexCatalogModelsForSave 成对做 load→save 回环测试——
// 任一侧丢字段都会静默清空供应商的逐模型声明（reasoningLevels、
// baseInstructions 等），且 UI 无可察觉。
export const mapCodexCatalogModelForForm = (item: any): CodexCatalogModel => {
  // 隐藏字段（原生 Responses profile 用）不在行 UI 暴露，但必须 load→save
  // 原样保留，否则编辑保存 MiMo/MiniMax 等会丢官方 base_instructions、
  // 并行工具、图像模态。DB SSOT 为 camelCase、live 反解兜底可能为 snake_case，
  // 双格式兼容（与 displayName/contextWindow 一致）。
  const supportsParallelToolCalls =
    typeof item?.supportsParallelToolCalls === "boolean"
      ? item.supportsParallelToolCalls
      : typeof item?.supports_parallel_tool_calls === "boolean"
        ? item.supports_parallel_tool_calls
        : undefined;
  const inputModalities = Array.isArray(item?.inputModalities)
    ? item.inputModalities
    : Array.isArray(item?.input_modalities)
      ? item.input_modalities
      : undefined;
  const baseInstructions =
    typeof item?.baseInstructions === "string"
      ? item.baseInstructions
      : typeof item?.base_instructions === "string"
        ? item.base_instructions
        : undefined;
  const reasoningLevels = Array.isArray(item?.reasoningLevels)
    ? item.reasoningLevels
    : Array.isArray(item?.reasoning_levels)
      ? item.reasoning_levels
      : undefined;
  const defaultReasoningLevel =
    typeof item?.defaultReasoningLevel === "string"
      ? item.defaultReasoningLevel
      : typeof item?.default_reasoning_level === "string"
        ? item.default_reasoning_level
        : undefined;
  return {
    model: typeof item?.model === "string" ? item.model : "",
    displayName:
      typeof item?.displayName === "string"
        ? item.displayName
        : typeof item?.display_name === "string"
          ? item.display_name
          : "",
    contextWindow:
      typeof item?.contextWindow === "string" ||
      typeof item?.contextWindow === "number"
        ? item.contextWindow
        : typeof item?.context_window === "string" ||
            typeof item?.context_window === "number"
          ? item.context_window
          : "",
    ...(supportsParallelToolCalls !== undefined
      ? { supportsParallelToolCalls }
      : {}),
    ...(inputModalities ? { inputModalities } : {}),
    ...(baseInstructions ? { baseInstructions } : {}),
    ...(reasoningLevels && reasoningLevels.length > 0
      ? { reasoningLevels }
      : {}),
    ...(defaultReasoningLevel ? { defaultReasoningLevel } : {}),
  };
};

function normalizeCatalogModels(rawModels: unknown): CodexCatalogModel[] {
  return Array.isArray(rawModels)
    ? rawModels
        .map(mapCodexCatalogModelForForm)
        .filter((item: CodexCatalogModel) => item.model.trim())
    : [];
}

/** 编辑已有供应商时，从它的 settingsConfig 读出表单的初始值；没有可用的配置时为 null */
function readCodexInitialState(
  initialData: UseCodexConfigStateProps["initialData"],
) {
  const config = initialData?.settingsConfig;
  if (typeof config !== "object" || config === null) return null;
  const configStr =
    typeof (config as any).config === "string" ? (config as any).config : "";
  const auth = (config as any).auth || {};
  const modelCatalog = (config as any).modelCatalog;
  const activeInstructionsFile =
    extractCodexTopLevelString(
      configStr,
      MODEL_INSTRUCTIONS_FIELD,
    )?.trim() || "";
  const instructionFiles = normalizeCodexModelInstructionsFiles(
    (config as any).modelInstructionsFiles,
    activeInstructionsFile,
  );
  const hasIndependentReviewer = Object.prototype.hasOwnProperty.call(
    config,
    "autoReviewModel",
  );
  const hasLegacyReviewer = Boolean(
    modelCatalog &&
      typeof modelCatalog === "object" &&
      Object.prototype.hasOwnProperty.call(modelCatalog, "autoReviewModel"),
  );
  return {
    config: configStr as string,
    auth: JSON.stringify(auth, null, 2),
    catalogModels: normalizeCatalogModels(modelCatalog?.models),
    aliases: parseModelAliasesFromValue((config as any).modelAliases),
    instructionFiles,
    instructionsEnabled: Boolean(activeInstructionsFile),
    instructionsFile: activeInstructionsFile || instructionFiles[0] || "",
    hasStoredReviewer: hasIndependentReviewer || hasLegacyReviewer,
    autoReviewModel: hasIndependentReviewer
      ? typeof config.autoReviewModel === "string"
        ? config.autoReviewModel.trim()
        : ""
      : typeof modelCatalog?.autoReviewModel === "string"
        ? modelCatalog.autoReviewModel.trim()
        : "",
    officialSpeedTiers: modelCatalog?.officialSpeedTiers === true,
    baseUrl: extractCodexBaseUrl(configStr) || "",
    apiKey: pickCodexApiKey(auth, configStr),
  };
}

/**
 * 管理 Codex 配置状态
 * Codex 配置包含两部分：auth.json (JSON) 和 config.toml (TOML 字符串)
 */
export function useCodexConfigState({ initialData }: UseCodexConfigStateProps) {
  // 初始值直接按 initialData 算好，不在挂载后的 effect 里再设一遍：那样每次打开编辑页
  // 整张表单都要多渲染几轮。
  const [initial] = useState(() => readCodexInitialState(initialData));
  const [codexAuth, setCodexAuthState] = useState(initial?.auth ?? "");
  const [codexConfig, setCodexConfigState] = useState(initial?.config ?? "");
  const [codexApiKey, setCodexApiKey] = useState(initial?.apiKey ?? "");
  const [codexBaseUrl, setCodexBaseUrl] = useState(initial?.baseUrl ?? "");
  const [codexModel, setCodexModel] = useState(
    () => extractCodexModelName(initial?.config ?? "") || "",
  );
  const [codexCatalogModels, setCodexCatalogModels] = useState<
    CodexCatalogModel[]
  >(initial?.catalogModels ?? []);
  const [codexModelAliases, setCodexModelAliases] = useState<ModelAliasEntry[]>(
    initial?.aliases ?? [],
  );
  const [codexModelInstructionsEnabled, setCodexModelInstructionsEnabled] =
    useState(initial?.instructionsEnabled ?? false);
  const [codexModelInstructionsFile, setCodexModelInstructionsFile] =
    useState(initial?.instructionsFile ?? "");
  const [codexModelInstructionsFiles, setCodexModelInstructionsFilesState] =
    useState<string[]>(initial?.instructionFiles ?? []);
  // Independent reviewer model; older cards stored it in modelCatalog.
  // An explicit empty top-level value selects codex-auto-review, not the old value.
  const [codexAutoReviewModelDraft, setCodexAutoReviewModelDraft] =
    useState(initial?.autoReviewModel ?? "");
  const [hasStoredCodexAutoReviewModel, setHasStoredCodexAutoReviewModel] =
    useState(initial?.hasStoredReviewer ?? false);
  const [codexAutoReviewModelEdited, setCodexAutoReviewModelEdited] =
    useState(false);
  const shouldSaveCodexAutoReviewModel =
    hasStoredCodexAutoReviewModel || codexAutoReviewModelEdited;
  // Legacy alias-only cards keep their single-step mapping until this field is
  // edited. Display its target, but do not silently migrate it on an unrelated save.
  const codexAutoReviewModel = shouldSaveCodexAutoReviewModel
    ? codexAutoReviewModelDraft
    : (serializeModelAliases(codexModelAliases)?.["codex-auto-review"] ?? "");
  const setCodexAutoReviewModel = useCallback((model: string) => {
    setCodexAutoReviewModelDraft(model);
    setCodexAutoReviewModelEdited(true);
  }, []);
  // 保留官方 Fast 档：`modelCatalog.officialSpeedTiers`，默认关
  const [codexOfficialSpeedTiers, setCodexOfficialSpeedTiers] = useState(
    initial?.officialSpeedTiers ?? false,
  );
  const [codexAuthError, setCodexAuthError] = useState("");

  const isUpdatingCodexBaseUrlRef = useRef(false);
  const isUpdatingCodexModelRef = useRef(false);

  // initialData 之后换了（编辑模式）：重新读一遍。挂载时的那份已经用作初始值。
  const initializedFor = useRef(initialData);
  useEffect(() => {
    if (initializedFor.current === initialData) return;
    initializedFor.current = initialData;
    const next = readCodexInitialState(initialData);
    if (!next) return;
    setCodexConfigState(next.config);
    setCodexAuthState(next.auth);
    setCodexCatalogModels((current) =>
      catalogModelsEqual(current, next.catalogModels)
        ? current
        : next.catalogModels,
    );
    setCodexModelAliases((current) =>
      modelAliasesEqual(
        serializeModelAliases(current),
        serializeModelAliases(next.aliases),
      )
        ? current
        : next.aliases,
    );
    setCodexModelInstructionsEnabled(next.instructionsEnabled);
    setCodexModelInstructionsFile(next.instructionsFile);
    setCodexModelInstructionsFilesState((current) =>
      stringArraysEqual(current, next.instructionFiles)
        ? current
        : next.instructionFiles,
    );
    setHasStoredCodexAutoReviewModel(next.hasStoredReviewer);
    setCodexAutoReviewModelEdited(false);
    setCodexAutoReviewModelDraft(next.autoReviewModel);
    setCodexOfficialSpeedTiers(next.officialSpeedTiers);
    if (next.baseUrl) {
      setCodexBaseUrl(next.baseUrl);
    }
    setCodexApiKey(next.apiKey);
  }, [initialData]);

  // Keep the dedicated control synchronized when config.toml is edited by hand.
  // Removing the TOML line disables application but intentionally retains the
  // last selected path so the user can turn it back on with one switch.
  useEffect(() => {
    const activeInstructionsFile =
      extractCodexTopLevelString(
        codexConfig,
        MODEL_INSTRUCTIONS_FIELD,
      )?.trim() || "";
    setCodexModelInstructionsEnabled(Boolean(activeInstructionsFile));
    if (activeInstructionsFile) {
      setCodexModelInstructionsFile((current) =>
        current === activeInstructionsFile ? current : activeInstructionsFile,
      );
      setCodexModelInstructionsFilesState((current) => {
        const next = normalizeCodexModelInstructionsFiles(
          current,
          activeInstructionsFile,
        );
        return stringArraysEqual(current, next) ? current : next;
      });
    }
  }, [codexConfig]);

  // 与 TOML 配置保持基础 URL 同步
  useEffect(() => {
    if (isUpdatingCodexBaseUrlRef.current) {
      return;
    }
    const extracted = extractCodexBaseUrl(codexConfig) || "";
    setCodexBaseUrl((prev) => (prev === extracted ? prev : extracted));
  }, [codexConfig]);

  // 与 TOML 配置保持默认模型同步（顶层 model 键）
  useEffect(() => {
    if (isUpdatingCodexModelRef.current) {
      return;
    }
    const extracted = extractCodexModelName(codexConfig) || "";
    setCodexModel((prev) => (prev === extracted ? prev : extracted));
  }, [codexConfig]);

  // 获取 API Key（从 auth JSON）
  const getCodexAuthApiKey = useCallback((authString: string): string => {
    try {
      const auth = JSON.parse(authString || "{}");
      if (isProxyManagedPlaceholder(auth.OPENAI_API_KEY)) {
        return "";
      }
      return normalizeUserApiKey(auth.OPENAI_API_KEY);
    } catch {
      return "";
    }
  }, []);

  // 从 codexAuth 中提取并同步 API Key
  useEffect(() => {
    let parsed: { OPENAI_API_KEY?: unknown } | null = null;
    try {
      parsed = JSON.parse(codexAuth || "{}");
    } catch {
      parsed = null;
    }
    const extractedKey = pickCodexApiKey(parsed, codexConfig);
    setCodexApiKey((prev) => (prev === extractedKey ? prev : extractedKey));
  }, [codexAuth, codexConfig]);

  // 验证 Codex Auth JSON
  const validateCodexAuth = useCallback((value: string): string => {
    if (!value.trim()) return "";
    try {
      const parsed = JSON.parse(value);
      if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
        return "Auth JSON must be an object";
      }
      return "";
    } catch {
      return "Invalid JSON format";
    }
  }, []);

  // 设置 auth 并验证
  const setCodexAuth = useCallback(
    (value: string) => {
      setCodexAuthState(value);
      setCodexAuthError(validateCodexAuth(value));
    },
    [validateCodexAuth],
  );

  // 设置 config (支持函数更新)
  const setCodexConfig = useCallback(
    (value: string | ((prev: string) => string)) => {
      setCodexConfigState((prev) =>
        typeof value === "function"
          ? (value as (input: string) => string)(prev)
          : value,
      );
    },
    [],
  );

  // 处理 Codex API Key 输入并写回 auth.json
  // 同步: 若 config.toml 当前含 experimental_bearer_token (Mobile 兼容形态),
  // 也一并更新/清除——否则用户清空输入框会被 pickCodexApiKey 的 fallback 又填回去
  const handleCodexApiKeyChange = useCallback(
    (key: string) => {
      const trimmed = key.trim();
      setCodexApiKey(trimmed);
      try {
        const auth = JSON.parse(codexAuth || "{}");
        auth.OPENAI_API_KEY = trimmed;
        setCodexAuth(JSON.stringify(auth, null, 2));
      } catch {
        // ignore
      }
      setCodexConfig((prev) =>
        updateCodexExperimentalBearerToken(prev, trimmed),
      );
    },
    [codexAuth, setCodexAuth, setCodexConfig],
  );

  // 处理 Codex Base URL 变化
  const handleCodexBaseUrlChange = useCallback(
    (url: string) => {
      const sanitized = url.trim();
      setCodexBaseUrl(sanitized);

      isUpdatingCodexBaseUrlRef.current = true;
      setCodexConfig((prev) => setCodexBaseUrlInConfig(prev, sanitized));
      setTimeout(() => {
        isUpdatingCodexBaseUrlRef.current = false;
      }, 0);
    },
    [setCodexConfig],
  );

  // 处理默认模型变化（写回 TOML 顶层 model；清空则删掉该行，交回 Codex 内置默认）
  // 剥控制字符：值可能来自 /models 下拉（远端数据），换行等会破坏单行 TOML 语义
  const handleCodexModelChange = useCallback(
    (model: string) => {
      const sanitized = model.replace(/[\u0000-\u001f\u007f]/g, "").trim();
      setCodexModel(sanitized);

      isUpdatingCodexModelRef.current = true;
      setCodexConfig((prev) => setCodexModelNameInConfig(prev, sanitized));
      setTimeout(() => {
        isUpdatingCodexModelRef.current = false;
      }, 0);
    },
    [setCodexConfig],
  );

  const handleCodexModelInstructionsActiveFileChange = useCallback(
    (value: string | null) => {
      const path = value?.trim() || "";
      setCodexModelInstructionsEnabled(Boolean(path));
      if (path) setCodexModelInstructionsFile(path);

      setCodexConfig((prev) =>
        path
          ? setCodexTopLevelString(prev, MODEL_INSTRUCTIONS_FIELD, path)
          : removeCodexTopLevelField(prev, MODEL_INSTRUCTIONS_FIELD),
      );

      if (path) {
        setCodexModelInstructionsFilesState((current) => {
          const next = normalizeCodexModelInstructionsFiles(current, path);
          return stringArraysEqual(current, next) ? current : next;
        });
      }
    },
    [setCodexConfig],
  );

  const setCodexModelInstructionsFiles = useCallback((files: string[]) => {
    setCodexModelInstructionsFilesState((current) => {
      const next = normalizeCodexModelInstructionsFiles(files);
      return stringArraysEqual(current, next) ? current : next;
    });
  }, []);

  // 子代理的默认模型和推理强度（`[agents]` 表）：直接从 TOML 读，改动实时写回，清空则删键
  const codexSubagentModel = useMemo(
    () => extractCodexAgentsString(codexConfig, "default_subagent_model") || "",
    [codexConfig],
  );
  const codexSubagentReasoningEffort = useMemo(
    () =>
      extractCodexAgentsString(
        codexConfig,
        "default_subagent_reasoning_effort",
      ) || "",
    [codexConfig],
  );

  const handleCodexSubagentModelChange = useCallback(
    (model: string) => {
      const sanitized = model.replace(/[\u0000-\u001f\u007f]/g, "").trim();
      setCodexConfig((prev) =>
        setCodexAgentsString(prev, "default_subagent_model", sanitized),
      );
    },
    [setCodexConfig],
  );

  const handleCodexSubagentReasoningEffortChange = useCallback(
    (effort: string) => {
      setCodexConfig((prev) =>
        setCodexAgentsString(prev, "default_subagent_reasoning_effort", effort),
      );
    },
    [setCodexConfig],
  );

  // 子代理并发上限（`[agents] max_threads`）：Codex 的全局设置，保存时写进 live，不随供应商切换。
  // 只收数字，清空或 0 删键，回到 Codex 默认值
  const codexAgentMaxThreads = useMemo(
    () => extractCodexAgentsMaxThreads(codexConfig)?.toString() ?? "",
    [codexConfig],
  );
  const codexMultiAgentV2Total = useMemo(
    () => extractCodexMultiAgentV2Total(codexConfig),
    [codexConfig],
  );

  const handleCodexAgentMaxThreadsChange = useCallback(
    (value: string) => {
      const digits = value.replace(/\D/g, "").slice(0, 4);
      setCodexConfig((prev) =>
        setCodexAgentsMaxThreads(prev, digits ? Number(digits) : undefined),
      );
    },
    [setCodexConfig],
  );

  // 处理 config 变化（同步 Base URL）
  const handleCodexConfigChange = useCallback(
    (value: string) => {
      // 归一化中文/全角/弯引号，避免 TOML 解析报错
      const normalized = normalizeTomlText(value);
      setCodexConfig(normalized);

      if (!isUpdatingCodexBaseUrlRef.current) {
        const extracted = extractCodexBaseUrl(normalized) || "";
        if (extracted !== codexBaseUrl) {
          setCodexBaseUrl(extracted);
        }
      }
    },
    [setCodexConfig, codexBaseUrl],
  );

  // 重置配置（用于预设切换）
  const resetCodexConfig = useCallback(
    (
      auth: Record<string, unknown>,
      config: string,
      modelCatalogModels: CodexCatalogModel[] = [],
      modelInstructionsFiles: unknown = [],
      modelAliases: unknown = undefined,
    ) => {
      const authString = JSON.stringify(auth, null, 2);
      setCodexAuth(authString);
      setCodexConfig(config);
      setCodexCatalogModels(normalizeCatalogModels(modelCatalogModels));
      setCodexModelAliases(parseModelAliasesFromValue(modelAliases));

      const activeInstructionsFile =
        extractCodexTopLevelString(config, MODEL_INSTRUCTIONS_FIELD)?.trim() ||
        "";
      const savedInstructionsFiles = normalizeCodexModelInstructionsFiles(
        modelInstructionsFiles,
        activeInstructionsFile,
      );
      setCodexModelInstructionsEnabled(Boolean(activeInstructionsFile));
      setCodexModelInstructionsFile(
        activeInstructionsFile || savedInstructionsFiles[0] || "",
      );
      setCodexModelInstructionsFilesState(savedInstructionsFiles);
      setCodexAutoReviewModelDraft("");
      setHasStoredCodexAutoReviewModel(false);
      setCodexAutoReviewModelEdited(false);
      setCodexOfficialSpeedTiers(false);

      const baseUrl = extractCodexBaseUrl(config);
      setCodexBaseUrl(baseUrl || "");

      setCodexApiKey(pickCodexApiKey(auth, config));
    },
    [setCodexAuth, setCodexConfig, setCodexCatalogModels, setCodexModelAliases],
  );

  return {
    codexAuth,
    codexConfig,
    codexApiKey,
    codexBaseUrl,
    codexModel,
    codexSubagentModel,
    codexSubagentReasoningEffort,
    codexAgentMaxThreads,
    codexMultiAgentV2Total,
    codexCatalogModels,
    codexModelAliases,
    codexModelInstructionsEnabled,
    codexModelInstructionsFile,
    codexModelInstructionsFiles,
    codexAutoReviewModel,
    shouldSaveCodexAutoReviewModel,
    codexOfficialSpeedTiers,
    codexAuthError,
    setCodexAuth,
    setCodexConfig,
    setCodexCatalogModels,
    setCodexModelAliases,
    setCodexModelInstructionsFiles,
    setCodexAutoReviewModel,
    setCodexOfficialSpeedTiers,
    handleCodexApiKeyChange,
    handleCodexBaseUrlChange,
    handleCodexModelChange,
    handleCodexModelInstructionsActiveFileChange,
    handleCodexSubagentModelChange,
    handleCodexSubagentReasoningEffortChange,
    handleCodexAgentMaxThreadsChange,
    handleCodexConfigChange,
    resetCodexConfig,
    getCodexAuthApiKey,
    validateCodexAuth,
  };
}
