import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Loader2, Save } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { toast } from "@/lib/toast";
import { proxyApi } from "@/lib/api/proxy";
import type { AppId } from "@/lib/api";
import { EMPTY_RELIABILITY, type ReliabilityConfig } from "@/types/proxy";

export interface ReliabilityPanelProps {
  appType: AppId;
}

/**
 * 代理可靠性（Fork 扩展）面板：keep-alive / 死循环检测 / 非 API 响应失败转移。
 * 对应后端 reliability.rs。三项各自默认关闭、关闭态零行为变化。
 */
export function ReliabilityPanel({ appType }: ReliabilityPanelProps) {
  const { t } = useTranslation();
  const [cfg, setCfg] = useState<ReliabilityConfig>(EMPTY_RELIABILITY);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    let active = true;
    setLoading(true);
    proxyApi
      .getReliabilityConfig(appType)
      .then((c) => {
        if (active) setCfg({ ...EMPTY_RELIABILITY, ...c });
      })
      .catch(() => {
        /* 读取失败保持默认（全关） */
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [appType]);

  const busy = loading || saving;

  const handleSave = async () => {
    if (saving) return;
    setSaving(true);
    try {
      await proxyApi.setReliabilityConfig(appType, cfg);
      toast.success(t("proxy.reliability.saved", "可靠性配置已保存"), {
        closeButton: true,
      });
    } catch (e) {
      toast.error(
        t("proxy.reliability.saveFailed", "保存失败") + ":" + String(e),
      );
    } finally {
      setSaving(false);
    }
  };

  const toggleLabel = (on: boolean) =>
    on
      ? t("proxy.reliability.on", "已开启")
      : t("proxy.reliability.off", "已关闭");

  return (
    <div className="space-y-3">
      {/* keep-alive */}
      <div className="flex items-start justify-between gap-3 rounded-md border border-border bg-surface-1 p-3">
        <div className="min-w-0 flex-1">
          <p className="text-sm font-medium text-fg-1">
            {t("proxy.reliability.keepalive.title", "SSE keep-alive")}
          </p>
          <p className="mt-0.5 text-xs text-fg-2">
            {t(
              "proxy.reliability.keepalive.desc",
              "上游静默时周期性发送 SSE 注释行，避免中间层（Cloudflare/反代）~125s 空闲断连。注释被客户端忽略，无语义影响。",
            )}
          </p>
          {cfg.keepaliveEnabled && (
            <div className="mt-2 flex items-center gap-2">
              <span className="text-xs text-fg-3">
                {t("proxy.reliability.keepalive.interval", "间隔（秒）")}
              </span>
              <Input
                type="number"
                value={cfg.keepaliveIntervalSecs}
                onChange={(e) =>
                  setCfg((c) => ({
                    ...c,
                    keepaliveIntervalSecs: e.target.value
                      ? Number(e.target.value)
                      : 0,
                  }))
                }
                disabled={busy}
                className="w-24 font-mono text-xs"
              />
              <span className="text-xs text-fg-3">
                {t(
                  "proxy.reliability.keepalive.hint",
                  "留空/0 用 15；上限 300",
                )}
              </span>
            </div>
          )}
        </div>
        <Button
          variant={cfg.keepaliveEnabled ? "solid" : "neutral"}
          size="sm"
          onClick={() =>
            setCfg((c) => ({ ...c, keepaliveEnabled: !c.keepaliveEnabled }))
          }
          disabled={busy}
          className="shrink-0"
        >
          {toggleLabel(cfg.keepaliveEnabled)}
        </Button>
      </div>

      {/* loop guard */}
      <div className="flex items-start justify-between gap-3 rounded-md border border-border bg-surface-1 p-3">
        <div className="min-w-0 flex-1">
          <p className="text-sm font-medium text-fg-1">
            {t("proxy.reliability.loopGuard.title", "死循环检测")}
          </p>
          <p className="mt-0.5 text-xs text-fg-2">
            {t(
              "proxy.reliability.loopGuard.desc",
              "流式输出陷入短语级死循环时干净切断（跳过分隔线/空白等合法重复，阈值较高）。",
            )}
          </p>
        </div>
        <Button
          variant={cfg.loopGuard ? "solid" : "neutral"}
          size="sm"
          onClick={() => setCfg((c) => ({ ...c, loopGuard: !c.loopGuard }))}
          disabled={busy}
          className="shrink-0"
        >
          {toggleLabel(cfg.loopGuard)}
        </Button>
      </div>

      {/* non-API failover */}
      <div className="flex items-start justify-between gap-3 rounded-md border border-border bg-surface-1 p-3">
        <div className="min-w-0 flex-1">
          <p className="text-sm font-medium text-fg-1">
            {t("proxy.reliability.nonApi.title", "非 API 响应失败转移")}
          </p>
          <p className="mt-0.5 text-xs text-fg-2">
            {t(
              "proxy.reliability.nonApi.desc",
              "上游返回 2xx 却是 HTML/Cloudflare 挑战页时，转成可重试错误走失败转移，而不是把错误页当响应下发。",
            )}
          </p>
        </div>
        <Button
          variant={cfg.nonApiFailover ? "solid" : "neutral"}
          size="sm"
          onClick={() =>
            setCfg((c) => ({ ...c, nonApiFailover: !c.nonApiFailover }))
          }
          disabled={busy}
          className="shrink-0"
        >
          {toggleLabel(cfg.nonApiFailover)}
        </Button>
      </div>

      <div className="flex justify-end">
        <Button
          variant="solid"
          size="regular"
          onClick={handleSave}
          disabled={busy}
        >
          {saving ? (
            <Loader2 className="mr-2 h-4 w-4 animate-spin" />
          ) : (
            <Save className="mr-2 h-4 w-4" />
          )}
          {t("common.save", "保存")}
        </Button>
      </div>
    </div>
  );
}

export default ReliabilityPanel;
