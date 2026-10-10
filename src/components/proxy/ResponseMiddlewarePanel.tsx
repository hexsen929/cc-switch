import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Loader2, Save } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { toast } from "@/lib/toast";
import { proxyApi } from "@/lib/api/proxy";
import {
  EMPTY_RESPONSE_MIDDLEWARE,
  type ResponseHookConfig,
  type ResponseMiddlewareConfig,
} from "@/types/proxy";

const ON_RESPONSE_EXAMPLE = `(response, meta) => {
  // meta: { appType, providerId, providerName, model }
  // 仅作用于非流式 JSON 响应；返回对象替换响应体，返回非对象则放行
  if (response && response.usage) response.usage.audited = true;
  return response;
}`;

const ON_EVENT_EXAMPLE = `(event, meta) => {
  // 流式 SSE 的每个事件（event 是该事件 data: 行里的 JSON）
  // 返回对象替换该事件 data，返回非对象/报错则该事件原样透传
  return event;
}`;

export interface ResponseMiddlewarePanelProps {
  appType: string;
}

/**
 * 可编程响应中间件（Fork 扩展）面板：按应用编辑 onResponse / onEvent JS 脚本并各自开关。
 * 脚本在后端 QuickJS 沙箱内执行（限内存/栈、CPU 上限、无网络），失败即逐事件/整体放行。
 * - onResponse 仅作用于非流式 JSON 响应；onEvent 作用于流式 SSE 的每个事件。
 */
export function ResponseMiddlewarePanel({
  appType,
}: ResponseMiddlewarePanelProps) {
  const { t } = useTranslation();
  const [cfg, setCfg] = useState<ResponseMiddlewareConfig>(
    EMPTY_RESPONSE_MIDDLEWARE,
  );
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const busy = loading || saving;

  useEffect(() => {
    let active = true;
    setLoading(true);
    proxyApi
      .getResponseMiddlewareConfig(appType)
      .then((c) => {
        if (active) setCfg(c);
      })
      .catch(() => {
        /* 读取失败时保持默认（两钩子关闭 + 空脚本） */
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [appType]);

  const handleSave = async () => {
    if (saving) return;
    setSaving(true);
    try {
      await proxyApi.setResponseMiddlewareConfig(appType, cfg);
      toast.success(t("proxy.respMiddleware.saved", "响应中间件已保存"), {
        closeButton: true,
      });
    } catch (e) {
      toast.error(
        t("proxy.respMiddleware.saveFailed", "保存失败") + ":" + String(e),
      );
    } finally {
      setSaving(false);
    }
  };

  const patchResponse = (patch: Partial<ResponseHookConfig>) =>
    setCfg((c) => ({ ...c, onResponse: { ...c.onResponse, ...patch } }));
  const patchEvent = (patch: Partial<ResponseHookConfig>) =>
    setCfg((c) => ({ ...c, onEvent: { ...c.onEvent, ...patch } }));

  return (
    <div className="space-y-4">
      <div className="min-w-0">
        <p className="text-sm font-medium">
          {t("proxy.respMiddleware.title", "可编程响应中间件")}
        </p>
        <p className="mt-1 text-xs text-fg-2">
          {t(
            "proxy.respMiddleware.hint",
            "对上游响应运行你自己的脚本：onResponse 改写非流式 JSON 响应体，onEvent 改写流式 SSE 的每个事件。两个钩子各自独立开关，默认关闭、失败放行。",
          )}
        </p>
      </div>

      {/* onResponse（非流式） */}
      <div className="space-y-2">
        <div className="flex items-center justify-between gap-2">
          <p className="text-xs font-medium text-fg-1">
            {t("proxy.respMiddleware.onResponse.title", "onResponse（非流式）")}
          </p>
          <Button
            variant={cfg.onResponse.enabled ? "solid" : "neutral"}
            size="sm"
            onClick={() => patchResponse({ enabled: !cfg.onResponse.enabled })}
            disabled={busy}
            className="shrink-0"
          >
            {cfg.onResponse.enabled
              ? t("proxy.respMiddleware.on", "已开启")
              : t("proxy.respMiddleware.off", "已关闭")}
          </Button>
        </div>
        <Textarea
          value={cfg.onResponse.script}
          onChange={(e) => patchResponse({ script: e.target.value })}
          disabled={busy}
          rows={8}
          placeholder={ON_RESPONSE_EXAMPLE}
          className="font-mono text-xs"
        />
      </div>
      {/* onEvent（流式） */}
      <div className="space-y-2">
        <div className="flex items-center justify-between gap-2">
          <p className="text-xs font-medium text-fg-1">
            {t("proxy.respMiddleware.onEvent.title", "onEvent（流式）")}
          </p>
          <Button
            variant={cfg.onEvent.enabled ? "solid" : "neutral"}
            size="sm"
            onClick={() => patchEvent({ enabled: !cfg.onEvent.enabled })}
            disabled={busy}
            className="shrink-0"
          >
            {cfg.onEvent.enabled
              ? t("proxy.respMiddleware.on", "已开启")
              : t("proxy.respMiddleware.off", "已关闭")}
          </Button>
        </div>
        <Textarea
          value={cfg.onEvent.script}
          onChange={(e) => patchEvent({ script: e.target.value })}
          disabled={busy}
          rows={8}
          placeholder={ON_EVENT_EXAMPLE}
          className="font-mono text-xs"
        />
        <p className="text-xs text-warning">
          {t(
            "proxy.respMiddleware.onEvent.warn",
            "每个 SSE 事件都会执行一次脚本（50ms CPU 上限）；逻辑务必精简。任何报错/超时/非对象返回都会原样放行该事件，绝不中断你的流。",
          )}
        </p>
      </div>

      <p className="text-xs text-fg-2">
        {t(
          "proxy.respMiddleware.contract",
          "脚本必须是求值得到函数的表达式：onResponse 为 (response, meta) => any，onEvent 为 (event, meta) => any。返回 JSON 对象替换，返回非对象按原样放行。",
        )}
      </p>
      <p className="text-xs text-fg-2">
        {t(
          "proxy.respMiddleware.security",
          "脚本运行在隔离沙箱中：无网络、无文件。onResponse 仅作用于非流式 JSON，非 JSON（含压缩/二进制）响应一律放行、不触碰。",
        )}
      </p>

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
