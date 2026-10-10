import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  AlertTriangle,
  Copy,
  Loader2,
  Plus,
  RefreshCw,
  Shield,
  Trash2,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { SettingsSwitchRow } from "@/components/settings/SettingsLayout";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { toast } from "@/lib/toast";
import { proxyApi } from "@/lib/api/proxy";
import { PROXY_APP_IDS } from "@/config/appConfig";
import { APP_DISPLAY_NAME } from "@/components/shell/AppGlyph";
import {
  GATEWAY_LIMIT_WINDOWS,
  type GatewayKey,
  type GatewayKeyView,
} from "@/types/proxy";

/** 把逗号/换行分隔的输入解析为去空白、去空项的字符串数组。 */
function parseList(raw: string): string[] {
  return raw
    .split(/[,\n]/)
    .map((x) => x.trim())
    .filter(Boolean);
}

async function copyToClipboard(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

/**
 * 局域网网关（Fork 扩展）面板：总开关 + per-key 鉴权密钥管理。
 * 默认关闭；仅在开启「局域网共享」后，非环回入站才被允许且强制携带某个 enabled 的 key。
 * 对应后端 gateway_auth.rs。环回（本地 CLI）始终免 key，行为与改造前一致。
 */
export function GatewayKeysPanel() {
  const { t } = useTranslation();
  const [shareEnabled, setShareEnabled] = useState(false);
  const [keys, setKeys] = useState<GatewayKeyView[]>([]);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [newName, setNewName] = useState("");
  const [reveal, setReveal] = useState<GatewayKey | null>(null);
  const [confirmRemove, setConfirmRemove] = useState<GatewayKeyView | null>(
    null,
  );
  const [confirmRotate, setConfirmRotate] = useState<GatewayKeyView | null>(
    null,
  );

  useEffect(() => {
    let active = true;
    setLoading(true);
    Promise.all([proxyApi.getLanShareEnabled(), proxyApi.listGatewayKeys()])
      .then(([en, ks]) => {
        if (!active) return;
        setShareEnabled(en);
        setKeys(ks);
      })
      .catch(() => {
        /* 读取失败保持默认（关闭 + 空列表） */
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, []);

  const refreshKeys = async () => {
    try {
      setKeys(await proxyApi.listGatewayKeys());
    } catch {
      /* 忽略：下次操作会再拉 */
    }
  };

  const toggleShare = async (v: boolean) => {
    setBusy(true);
    try {
      await proxyApi.setLanShareEnabled(v);
      setShareEnabled(v);
    } catch (e) {
      toast.error(
        t("proxy.lanGateway.toggleFailed", "切换局域网共享失败") +
          ":" +
          String(e),
      );
    } finally {
      setBusy(false);
    }
  };

  const addKey = async () => {
    setBusy(true);
    try {
      const k = await proxyApi.addGatewayKey(newName.trim() || "");
      setNewName("");
      setReveal(k);
      await refreshKeys();
    } catch (e) {
      toast.error(
        t("proxy.lanGateway.addFailed", "新增密钥失败") + ":" + String(e),
      );
    } finally {
      setBusy(false);
    }
  };

  const doRotate = async (id: string) => {
    setBusy(true);
    try {
      const k = await proxyApi.rotateGatewayKey(id);
      if (k) setReveal(k);
      await refreshKeys();
    } catch (e) {
      toast.error(
        t("proxy.lanGateway.rotateFailed", "轮换失败") + ":" + String(e),
      );
    } finally {
      setBusy(false);
    }
  };

  const doRemove = async (id: string) => {
    setBusy(true);
    try {
      await proxyApi.removeGatewayKey(id);
      await refreshKeys();
    } catch (e) {
      toast.error(
        t("proxy.lanGateway.removeFailed", "删除失败") + ":" + String(e),
      );
    } finally {
      setBusy(false);
    }
  };

  const toggleKey = async (id: string, enabled: boolean) => {
    try {
      await proxyApi.setGatewayKeyEnabled(id, enabled);
      await refreshKeys();
    } catch (e) {
      toast.error(
        t("proxy.lanGateway.toggleKeyFailed", "切换密钥状态失败") +
          ":" +
          String(e),
      );
    }
  };

  const renameKey = async (id: string, name: string) => {
    try {
      await proxyApi.renameGatewayKey(id, name);
    } catch {
      /* 静默：重命名失败不致命，下次刷新会回到真实值 */
      await refreshKeys();
    }
  };

  const copyToken = async (text: string) => {
    const ok = await copyToClipboard(text);
    if (ok) toast.success(t("proxy.lanGateway.copied", "已复制"));
    else toast.error(t("proxy.lanGateway.copyFailed", "复制失败"));
  };

  const saveCaps = async (
    id: string,
    allowedApps: string[],
    allowedProviders: string[],
    allowedModels: string[],
  ) => {
    try {
      await proxyApi.setGatewayKeyCaps(
        id,
        allowedApps,
        allowedProviders,
        allowedModels,
      );
      await refreshKeys();
      toast.success(t("proxy.lanGateway.capsSaved", "权限已保存"));
    } catch (e) {
      toast.error(
        t("proxy.lanGateway.capsSaveFailed", "权限保存失败") + ":" + String(e),
      );
    }
  };

  const saveLimits = async (
    id: string,
    window: string,
    limitTokens: number | null,
    limitCostUsd: number | null,
  ) => {
    try {
      await proxyApi.setGatewayKeyLimits(id, window, limitTokens, limitCostUsd);
      await refreshKeys();
      toast.success(t("proxy.lanGateway.limitsSaved", "配额已保存"));
    } catch (e) {
      toast.error(
        t("proxy.lanGateway.limitsSaveFailed", "配额保存失败") +
          ":" +
          String(e),
      );
    }
  };

  // PANEL_RENDER_PLACEHOLDER
  return (
    <div className="space-y-4">
      <SettingsSwitchRow
        label={t("proxy.lanGateway.shareToggle", "允许局域网共享")}
        help={{
          title: t("proxy.lanGateway.shareToggle", "允许局域网共享"),
          body: t(
            "proxy.lanGateway.shareHelp",
            "关闭时：仅本机（环回）可访问代理，非环回入站一律 403。开启后：非环回入站必须携带下方某个「已启用」的网关密钥（Bearer / x-api-key / x-goog-api-key / ?key=），否则 401。本机 CLI 始终免密钥。",
          ),
        }}
        checked={shareEnabled}
        disabled={loading || busy}
        onCheckedChange={(v) => void toggleShare(v)}
      />

      {shareEnabled && (
        <div className="flex gap-2 rounded-md border border-warning bg-warning-soft p-3 text-xs text-warning-text">
          <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />
          <p>
            {t(
              "proxy.lanGateway.securityWarning",
              "开启共享即把携带你上游 API 密钥的代理暴露到局域网。请仅在可信网络中开启，并为每个客户端单独创建密钥，便于按客户端启停或吊销。",
            )}
          </p>
        </div>
      )}

      {reveal && (
        <div className="space-y-2 rounded-md border border-border bg-surface p-3">
          <p className="text-xs font-medium text-fg-1">
            {t(
              "proxy.lanGateway.revealTitle",
              "新密钥（完整 token 仅此一次显示，请立即复制保存）",
            )}
          </p>
          <div className="flex items-center gap-2">
            <code className="flex-1 overflow-x-auto rounded border border-border bg-subtle px-2 py-1 font-mono text-xs text-fg-1">
              {reveal.token}
            </code>
            <Button
              variant="neutral"
              size="sm"
              onClick={() => void copyToken(reveal.token)}
            >
              <Copy className="mr-1 h-3.5 w-3.5" />
              {t("common.copy", "复制")}
            </Button>
            <Button
              variant="neutral"
              size="sm"
              onClick={() => setReveal(null)}
              aria-label={t("common.close", "关闭")}
            >
              {t("common.close", "关闭")}
            </Button>
          </div>
        </div>
      )}

      {shareEnabled && (
        <div className="space-y-3">
          <div className="flex items-center gap-2">
            <Input
              value={newName}
              onChange={(e) => setNewName(e.target.value)}
              disabled={busy}
              placeholder={t(
                "proxy.lanGateway.newKeyName",
                "新密钥名称（可选，建议按客户端命名）",
              )}
              className="text-xs"
            />
            <Button
              variant="solid"
              size="sm"
              onClick={() => void addKey()}
              disabled={busy}
              className="shrink-0"
            >
              {busy ? (
                <Loader2 className="mr-1 h-3.5 w-3.5 animate-spin" />
              ) : (
                <Plus className="mr-1 h-3.5 w-3.5" />
              )}
              {t("proxy.lanGateway.addKey", "新增密钥")}
            </Button>
          </div>

          {keys.length === 0 ? (
            <p className="text-xs text-fg-3">
              {t(
                "proxy.lanGateway.noKeys",
                "暂无密钥。开启共享后，必须至少创建一个密钥，远程客户端才能访问。",
              )}
            </p>
          ) : (
            <div className="space-y-2">
              {keys.map((k) => (
                <KeyRow
                  key={k.id}
                  item={k}
                  disabled={busy}
                  onToggle={(en) => void toggleKey(k.id, en)}
                  onRename={(name) => void renameKey(k.id, name)}
                  onRotate={() => setConfirmRotate(k)}
                  onRemove={() => setConfirmRemove(k)}
                  onCopyMasked={() => void copyToken(k.tokenMasked)}
                  onSaveCaps={(apps, providers, models) =>
                    void saveCaps(k.id, apps, providers, models)
                  }
                  onSaveLimits={(window, tokens, cost) =>
                    void saveLimits(k.id, window, tokens, cost)
                  }
                />
              ))}
            </div>
          )}
        </div>
      )}

      {/* DIALOGS_PLACEHOLDER */}
      <ConfirmDialog
        isOpen={!!confirmRotate}
        variant="destructive"
        title={t("proxy.lanGateway.rotateTitle", "轮换此密钥的 token？")}
        message={t(
          "proxy.lanGateway.rotateMessage",
          "旧 token 会立即失效，使用它的客户端需要更换为新 token。此操作不可撤销。",
        )}
        confirmText={t("proxy.lanGateway.rotateConfirm", "轮换")}
        onConfirm={() => {
          const target = confirmRotate;
          setConfirmRotate(null);
          if (target) void doRotate(target.id);
        }}
        onCancel={() => setConfirmRotate(null)}
      />
      <ConfirmDialog
        isOpen={!!confirmRemove}
        variant="destructive"
        title={t("proxy.lanGateway.removeTitle", "删除此密钥？")}
        message={t(
          "proxy.lanGateway.removeMessage",
          "删除后使用此 token 的客户端将无法访问，且无法恢复。",
        )}
        confirmText={t("common.delete", "删除")}
        onConfirm={() => {
          const target = confirmRemove;
          setConfirmRemove(null);
          if (target) void doRemove(target.id);
        }}
        onCancel={() => setConfirmRemove(null)}
      />
    </div>
  );
}

interface KeyRowProps {
  item: GatewayKeyView;
  disabled: boolean;
  onToggle: (enabled: boolean) => void;
  onRename: (name: string) => void;
  onRotate: () => void;
  onRemove: () => void;
  onCopyMasked: () => void;
  onSaveCaps: (apps: string[], providers: string[], models: string[]) => void;
  onSaveLimits: (
    window: string,
    tokens: number | null,
    cost: number | null,
  ) => void;
}

function KeyRow({
  item,
  disabled,
  onToggle,
  onRename,
  onRotate,
  onRemove,
  onCopyMasked,
  onSaveCaps,
  onSaveLimits,
}: KeyRowProps) {
  const { t } = useTranslation();
  const [name, setName] = useState(item.name);
  const [showCaps, setShowCaps] = useState(false);
  const [apps, setApps] = useState<string[]>(item.allowedApps);
  const [providers, setProviders] = useState(item.allowedProviders.join(", "));
  const [models, setModels] = useState(item.allowedModels.join(", "));
  const [limitWindow, setLimitWindow] = useState(item.limitWindow || "none");
  const [limitTokens, setLimitTokens] = useState(
    item.limitTokens == null ? "" : String(item.limitTokens),
  );
  const [limitCost, setLimitCost] = useState(
    item.limitCostUsd == null ? "" : String(item.limitCostUsd),
  );

  const commitName = () => {
    const trimmed = name.trim();
    if (trimmed !== item.name) onRename(trimmed);
  };

  const toggleApp = (app: string) =>
    setApps((cur) =>
      cur.includes(app) ? cur.filter((a) => a !== app) : [...cur, app],
    );

  const saveLimits = () => {
    const tokens = limitTokens.trim() ? Number(limitTokens.trim()) : null;
    const cost = limitCost.trim() ? Number(limitCost.trim()) : null;
    onSaveLimits(
      limitWindow,
      tokens != null && Number.isFinite(tokens) ? tokens : null,
      cost != null && Number.isFinite(cost) ? cost : null,
    );
  };

  const hasCaps =
    item.allowedApps.length > 0 ||
    item.allowedProviders.length > 0 ||
    item.allowedModels.length > 0 ||
    (item.limitWindow && item.limitWindow !== "none") ||
    item.limitTokens != null ||
    item.limitCostUsd != null;

  return (
    <div className="space-y-2 rounded-md border border-border bg-surface p-2">
      <div className="flex items-center gap-2">
        <Button
          variant={item.enabled ? "solid" : "neutral"}
          size="sm"
          onClick={() => onToggle(!item.enabled)}
          disabled={disabled}
          className="shrink-0"
        >
          {item.enabled
            ? t("proxy.lanGateway.enabled", "已启用")
            : t("proxy.lanGateway.disabled", "已停用")}
        </Button>
        <Input
          value={name}
          onChange={(e) => setName(e.target.value)}
          onBlur={commitName}
          disabled={disabled}
          placeholder={t(
            "proxy.lanGateway.keyNamePlaceholder",
            "密钥名（可选）",
          )}
          className="text-xs"
        />
        <code
          className="shrink-0 cursor-pointer rounded border border-border bg-subtle px-2 py-1 font-mono text-xs text-fg-2"
          title={t("proxy.lanGateway.copyMasked", "复制脱敏值")}
          onClick={onCopyMasked}
        >
          {item.tokenMasked}
        </code>
        <Button
          variant={showCaps || hasCaps ? "solid" : "neutral"}
          size="sm"
          onClick={() => setShowCaps((v) => !v)}
          disabled={disabled}
          className="shrink-0"
          aria-label={t("proxy.lanGateway.caps", "权限")}
          title={t("proxy.lanGateway.caps", "权限")}
        >
          <Shield className="h-3.5 w-3.5" />
        </Button>
        <Button
          variant="neutral"
          size="sm"
          onClick={onRotate}
          disabled={disabled}
          className="shrink-0"
          aria-label={t("proxy.lanGateway.rotate", "轮换")}
        >
          <RefreshCw className="h-3.5 w-3.5" />
        </Button>
        <Button
          variant="neutral"
          size="sm"
          onClick={onRemove}
          disabled={disabled}
          className="shrink-0"
          aria-label={t("common.delete", "删除")}
        >
          <Trash2 className="h-3.5 w-3.5" />
        </Button>
      </div>

      {showCaps && (
        <div className="space-y-2 border-t border-border pt-2">
          <p className="text-xs text-fg-2">
            {t(
              "proxy.lanGateway.capsHint",
              "限制此密钥可用的范围；三项各自「留空=不限」。",
            )}
          </p>
          <div className="flex flex-wrap items-center gap-1.5">
            <span className="text-xs text-fg-3">
              {t("proxy.lanGateway.capApps", "应用")}
            </span>
            {PROXY_APP_IDS.map((app) => (
              <Button
                key={app}
                variant={apps.includes(app) ? "solid" : "neutral"}
                size="sm"
                onClick={() => toggleApp(app)}
                disabled={disabled}
              >
                {APP_DISPLAY_NAME[app]}
              </Button>
            ))}
          </div>
          <Input
            value={providers}
            onChange={(e) => setProviders(e.target.value)}
            disabled={disabled}
            placeholder={t(
              "proxy.lanGateway.capProviders",
              "允许的供应商 ID（逗号分隔，留空不限）",
            )}
            className="font-mono text-xs"
          />
          <Input
            value={models}
            onChange={(e) => setModels(e.target.value)}
            disabled={disabled}
            placeholder={t(
              "proxy.lanGateway.capModels",
              "允许的模型 glob（逗号分隔，如 anthropic/*，留空不限）",
            )}
            className="font-mono text-xs"
          />
          <div className="flex justify-end">
            <Button
              variant="solid"
              size="sm"
              disabled={disabled}
              onClick={() =>
                onSaveCaps(apps, parseList(providers), parseList(models))
              }
            >
              {t("proxy.lanGateway.saveCaps", "保存权限")}
            </Button>
          </div>

          <div className="space-y-2 border-t border-border pt-2">
            <p className="text-xs text-fg-2">
              {t(
                "proxy.lanGateway.limitsHint",
                "按本地日历窗口限制用量，超限返回 429；两个上限各自「留空=不限」。",
              )}
            </p>
            <div className="flex items-center gap-2">
              <span className="shrink-0 text-xs text-fg-3">
                {t("proxy.lanGateway.limitWindow", "窗口")}
              </span>
              <Select
                value={limitWindow}
                disabled={disabled}
                onValueChange={setLimitWindow}
              >
                <SelectTrigger className="text-xs">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {GATEWAY_LIMIT_WINDOWS.map((w) => (
                    <SelectItem key={w} value={w}>
                      {t(`proxy.lanGateway.window.${w}`, w)}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            <Input
              type="number"
              value={limitTokens}
              onChange={(e) => setLimitTokens(e.target.value)}
              disabled={disabled || limitWindow === "none"}
              placeholder={t(
                "proxy.lanGateway.limitTokens",
                "token 上限（留空不限）",
              )}
              className="font-mono text-xs"
            />
            <Input
              type="number"
              value={limitCost}
              onChange={(e) => setLimitCost(e.target.value)}
              disabled={disabled || limitWindow === "none"}
              placeholder={t(
                "proxy.lanGateway.limitCost",
                "花费上限（美元，留空不限）",
              )}
              className="font-mono text-xs"
            />
            <div className="flex justify-end">
              <Button
                variant="solid"
                size="sm"
                disabled={disabled}
                onClick={saveLimits}
              >
                {t("proxy.lanGateway.saveLimits", "保存配额")}
              </Button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
