import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Save } from "lucide-react";
import { Button } from "@/components/ui/button";
import { FullScreenPanel } from "@/components/common/FullScreenPanel";
import type { Provider } from "@/types";
import {
  ProviderForm,
  type ProviderFormValues,
} from "@/components/providers/forms/ProviderForm";
import { AuthSettingsPanel } from "@/components/providers/AuthSettingsPanel";
import {
  openclawApi,
  providersApi,
  vscodeApi,
  type AppId,
  type ManagedAuthProvider,
} from "@/lib/api";
import { extractCodexBaseUrl } from "@/utils/providerConfigUtils";
import { CodexToolStripPanel } from "@/components/providers/CodexToolStripPanel";

interface EditProviderDialogProps {
  open: boolean;
  provider: Provider | null;
  onOpenChange: (open: boolean) => void;
  onSubmit: (payload: {
    provider: Provider;
    originalId?: string;
  }) => Promise<void> | void;
  appId: AppId;
  isProxyTakeover?: boolean; // 代理接管模式下不读取 live（避免显示被接管后的代理配置）
}

const PROXY_MANAGED_PLACEHOLDER = "PROXY_MANAGED";

function isLocalProxyUrl(url: string): boolean {
  const value = url.trim();
  if (!value.startsWith("http://")) return false;
  const rest = value.slice("http://".length);
  return (
    rest.startsWith("127.0.0.1") ||
    rest.startsWith("localhost") ||
    rest.startsWith("0.0.0.0") ||
    rest.startsWith("[::1]") ||
    rest.startsWith("[::]") ||
    rest.startsWith("::1") ||
    rest.startsWith("::")
  );
}

function shouldUseCodexLiveSettings(live: Record<string, unknown>): boolean {
  const auth =
    live.auth && typeof live.auth === "object"
      ? (live.auth as Record<string, unknown>)
      : {};

  if (
    typeof auth.OPENAI_API_KEY === "string" &&
    auth.OPENAI_API_KEY.trim() === PROXY_MANAGED_PLACEHOLDER
  ) {
    return false;
  }

  if (
    auth.auth_mode === "chatgpt" ||
    auth.preferred_auth_method === "chatgpt" ||
    (auth.tokens && typeof auth.tokens === "object")
  ) {
    return false;
  }

  const configText = typeof live.config === "string" ? live.config : "";
  const baseUrl = extractCodexBaseUrl(configText);
  return !baseUrl || !isLocalProxyUrl(baseUrl);
}

export function EditProviderDialog({
  open,
  provider,
  onOpenChange,
  onSubmit,
  appId,
  isProxyTakeover = false,
}: EditProviderDialogProps) {
  const { t } = useTranslation();
  const [isFormSubmitting, setIsFormSubmitting] = useState(false);
  const [authSettingsTarget, setAuthSettingsTarget] =
    useState<ManagedAuthProvider | null>(null);

  useEffect(() => {
    setAuthSettingsTarget(null);
  }, [appId, open, provider?.id]);

  const formReadyToken = useMemo(
    () => Symbol("provider-form-ready"),
    [appId, open, provider?.id],
  );
  const currentFormReadyToken = useRef(formReadyToken);
  currentFormReadyToken.current = formReadyToken;
  const [formReadyState, setFormReadyState] = useState({
    token: formReadyToken,
    ready: appId !== "pi",
  });
  const isFormReady =
    formReadyState.token === formReadyToken
      ? formReadyState.ready
      : appId !== "pi";
  const handleSubmitReadyChange = useCallback(
    (ready: boolean) => {
      if (currentFormReadyToken.current === formReadyToken) {
        setFormReadyState({ token: formReadyToken, ready });
      }
    },
    [formReadyToken],
  );

  // 默认使用传入的 provider.settingsConfig，若当前编辑对象是"当前生效供应商"，则尝试读取实时配置替换初始值
  const [liveSettings, setLiveSettings] = useState<Record<
    string,
    unknown
  > | null>(null);

  // 使用 ref 标记是否已经加载过，防止重复读取覆盖用户编辑
  const [hasLoadedLive, setHasLoadedLive] = useState(false);

  const closeDialog = useCallback(() => {
    setAuthSettingsTarget(null);
    onOpenChange(false);
  }, [onOpenChange]);

  const handlePanelClose = useCallback(() => {
    if (authSettingsTarget) {
      setAuthSettingsTarget(null);
      return;
    }
    closeDialog();
  }, [authSettingsTarget, closeDialog]);

  useEffect(() => {
    let cancelled = false;
    const load = async () => {
      if (!open || !provider) {
        setLiveSettings(null);
        setHasLoadedLive(false);
        return;
      }

      // 关键修复：只在首次打开时加载一次
      if (hasLoadedLive) {
        return;
      }

      // 代理接管模式：Live 配置已被代理改写，读取 live 会导致编辑界面展示代理地址/占位符等内容
      // 因此直接回退到 SSOT（数据库）配置，避免用户困惑与误保存
      if (isProxyTakeover) {
        if (!cancelled) {
          setLiveSettings(null);
          setHasLoadedLive(true);
        }
        return;
      }

      // OpenCode uses additive mode, while Pi's shared models.json is owned by
      // the catalog coordinator. Neither has a per-provider generic live
      // snapshot that may replace the DB aggregate in this form.
      if (appId === "opencode" || appId === "pi") {
        if (!cancelled) {
          setLiveSettings(null);
          setHasLoadedLive(true);
        }
        return;
      }

      if (appId === "openclaw") {
        try {
          const live = await openclawApi.getLiveProvider(provider.id);
          if (!cancelled && live && typeof live === "object") {
            setLiveSettings(live);
          } else if (!cancelled) {
            setLiveSettings(null);
          }
        } catch {
          if (!cancelled) {
            setLiveSettings(null);
          }
        } finally {
          if (!cancelled) {
            setHasLoadedLive(true);
          }
        }
        return;
      }

      try {
        const currentId = await providersApi.getCurrent(appId);
        if (currentId && provider.id === currentId) {
          try {
            const live = (await vscodeApi.getLiveProviderSettings(
              appId,
            )) as Record<string, unknown>;
            if (!cancelled && live && typeof live === "object") {
              setLiveSettings(
                appId !== "codex" || shouldUseCodexLiveSettings(live)
                  ? live
                  : null,
              );
              setHasLoadedLive(true);
            }
          } catch {
            // 读取实时配置失败则回退到 SSOT（不打断编辑流程）
            if (!cancelled) {
              setLiveSettings(null);
              setHasLoadedLive(true);
            }
          }
        } else {
          if (!cancelled) {
            setLiveSettings(null);
            setHasLoadedLive(true);
          }
        }
      } finally {
        // no-op
      }
    };
    void load();
    return () => {
      cancelled = true;
    };
  }, [open, provider?.id, appId, hasLoadedLive, isProxyTakeover]); // 只依赖 provider.id，不依赖整个 provider 对象

  const initialSettingsConfig = useMemo(() => {
    const base = (liveSettings ?? provider?.settingsConfig ?? {}) as Record<
      string,
      unknown
    >;

    // Codex 的 modelCatalog / modelInstructionsFiles / codex_strip_tools 是
    // cc-switch 私有字段，SSOT 在数据库。Live 只包含投影后的 config.toml；
    // 若放任 Live 整体覆盖，编辑当前供应商并保存就会清空这些私有字段。
    if (
      appId === "codex" &&
      liveSettings &&
      provider?.settingsConfig &&
      typeof provider.settingsConfig === "object"
    ) {
      const dbSettings = provider.settingsConfig as Record<string, unknown>;
      const privateSettings: Record<string, unknown> = {};
      for (const key of [
        "modelCatalog",
        "modelInstructionsFiles",
        "codex_strip_tools",
      ] as const) {
        if (dbSettings[key] !== undefined) {
          privateSettings[key] = dbSettings[key];
        }
      }
      if (Object.keys(privateSettings).length > 0) {
        return { ...base, ...privateSettings };
      }
    }

    return base;
  }, [liveSettings, provider?.settingsConfig, appId]); // 只依赖 settingsConfig，不依赖整个 provider

  // Codex 中转兼容性：tools 剥除清单
  // 数据源：provider.settings_config.codex_strip_tools（数组，元素是工具 type 字符串）
  // 仅 codex provider 用到。Save 时合并回 settings_config 顶层
  const initialCodexStripTools = useMemo<string[]>(() => {
    if (appId !== "codex") return [];
    const raw = (initialSettingsConfig as Record<string, unknown>)
      .codex_strip_tools;
    if (!Array.isArray(raw)) return [];
    return raw.filter((v): v is string => typeof v === "string");
  }, [appId, initialSettingsConfig]);

  const [codexStripTools, setCodexStripTools] = useState<string[]>(
    initialCodexStripTools,
  );

  // 当 dialog 重新打开 / provider 切换时，重置剥除清单
  useEffect(() => {
    setCodexStripTools(initialCodexStripTools);
  }, [initialCodexStripTools]);

  // 固定 initialData，防止 provider 对象更新时重置表单
  const initialData = useMemo(() => {
    if (!provider) return null;
    return {
      name: provider.name,
      notes: provider.notes,
      websiteUrl: provider.websiteUrl,
      settingsConfig: initialSettingsConfig,
      category: provider.category,
      meta: provider.meta,
      icon: provider.icon,
      iconColor: provider.iconColor,
    };
  }, [
    open, // 修复：编辑保存后再次打开显示旧数据，依赖 open 确保每次打开时重新读取最新 provider 数据
    provider?.id, // 只依赖 ID，provider 对象更新不会触发重新计算
    provider?.meta, // 供应商元数据变化时重新初始化表单
    initialSettingsConfig,
  ]);

  const handleSubmit = useCallback(
    async (values: ProviderFormValues) => {
      if (!provider) return;

      // 注意：values.settingsConfig 已经是最终的配置字符串
      // ProviderForm 已经为不同的 app 类型（Claude/Codex/Gemini）正确组装了配置
      const parsedConfig = JSON.parse(values.settingsConfig) as Record<
        string,
        unknown
      >;
      // Codex 中转兼容性：把"剥除工具清单"合并进 settings_config 顶层
      // 写空数组也保留为字段（明确表达"无剥除"语义）；用户可手动改 JSON 删该字段
      if (appId === "codex") {
        if (codexStripTools.length > 0) {
          parsedConfig.codex_strip_tools = codexStripTools;
        } else {
          // 数组为空时移除字段，保持配置干净
          delete parsedConfig.codex_strip_tools;
        }
      }
      const nextProviderId =
        (appId === "opencode" || appId === "openclaw" || appId === "pi") &&
        values.providerKey?.trim()
          ? values.providerKey.trim()
          : provider.id;

      const updatedProvider: Provider = {
        ...provider,
        id: nextProviderId,
        name: values.name.trim(),
        notes: values.notes?.trim() || undefined,
        websiteUrl: values.websiteUrl?.trim() || undefined,
        settingsConfig: parsedConfig,
        icon: values.icon?.trim() || undefined,
        iconColor: values.iconColor?.trim() || undefined,
        ...(values.presetCategory ? { category: values.presetCategory } : {}),
        // 保留或更新 meta 字段
        ...(values.meta ? { meta: values.meta } : {}),
      };

      await onSubmit({
        provider: updatedProvider,
        originalId: provider.id,
      });
      closeDialog();
    },
    [appId, codexStripTools, onSubmit, closeDialog, provider],
  );

  if (!provider || !initialData) {
    return null;
  }

  return (
    <FullScreenPanel
      isOpen={open}
      title={t("provider.editProvider")}
      onClose={handlePanelClose}
      contentClassName={appId === "pi" ? "pb-0" : undefined}
      footer={
        <Button
          type="submit"
          form="provider-form"
          disabled={isFormSubmitting || !isFormReady}
          className="bg-primary text-primary-foreground hover:bg-primary/90"
        >
          <Save className="h-4 w-4 mr-2" />
          {t("common.save")}
        </Button>
      }
    >
      <ProviderForm
        appId={appId}
        providerId={provider.id}
        submitLabel={t("common.save")}
        onSubmit={handleSubmit}
        onCancel={closeDialog}
        onManageAuthAccounts={setAuthSettingsTarget}
        onSubmittingChange={setIsFormSubmitting}
        onSubmitReadyChange={handleSubmitReadyChange}
        initialData={initialData}
        showButtons={false}
        isProxyTakeover={isProxyTakeover}
      />
      {appId === "codex" && (
        <div className="mt-4">
          <CodexToolStripPanel
            value={codexStripTools}
            onChange={setCodexStripTools}
          />
        </div>
      )}
      <AuthSettingsPanel
        target={authSettingsTarget}
        onClose={() => setAuthSettingsTarget(null)}
      />
    </FullScreenPanel>
  );
}
