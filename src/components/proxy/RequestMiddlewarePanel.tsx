import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Loader2, Save } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { toast } from "@/lib/toast";
import { proxyApi } from "@/lib/api/proxy";

const EXAMPLE_SCRIPT = `(request, meta) => {
  // meta: { appType, providerId, providerName, model }
  // 可就地修改 request 后不返回，或返回一个新的请求对象

  // 示例 1：模型映射
  if (request.model === "gpt-4o") request.model = "gpt-4o-mini";

  // 示例 2：统一压低温度
  if (typeof request.temperature === "number") request.temperature = 0.3;

  return request;
}`;

export interface RequestMiddlewarePanelProps {
  appType: string;
}

/**
 * 可编程请求中间件（Fork 扩展）面板：按应用编辑 onRequest JS 脚本并开关。
 * 脚本在后端 QuickJS 沙箱内执行（限内存/栈、1s CPU 上限、无网络），失败即放行。
 */
export function RequestMiddlewarePanel({
  appType,
}: RequestMiddlewarePanelProps) {
  const { t } = useTranslation();
  const [enabled, setEnabled] = useState(false);
  const [script, setScript] = useState("");
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    let active = true;
    setLoading(true);
    Promise.all([
      proxyApi.getRequestMiddlewareEnabled(appType),
      proxyApi.getRequestMiddlewareScript(appType),
    ])
      .then(([en, sc]) => {
        if (!active) return;
        setEnabled(en);
        setScript(sc);
      })
      .catch(() => {
        /* 读取失败时保持默认（关闭 + 空脚本） */
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
      await proxyApi.setRequestMiddlewareScript(appType, script);
      await proxyApi.setRequestMiddlewareEnabled(appType, enabled);
      toast.success(t("proxy.middleware.saved", "中间件已保存"), {
        closeButton: true,
      });
    } catch (e) {
      toast.error(
        t("proxy.middleware.saveFailed", "中间件保存失败") + ":" + String(e),
      );
    } finally {
      setSaving(false);
    }
  };

  const busy = loading || saving;

  return (
    <div className="space-y-3">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <p className="text-sm font-medium">
            {t("proxy.middleware.title", "可编程请求中间件")}
          </p>
          <p className="mt-1 text-xs text-fg-2">
            {t(
              "proxy.middleware.hint",
              "对出站请求体运行你自己的 onRequest 脚本，用于参数覆盖、模型映射、system 提示注入等。开关与脚本在「保存」后一起生效。",
            )}
          </p>
        </div>
        <Button
          variant={enabled ? "solid" : "neutral"}
          size="regular"
          onClick={() => setEnabled((v) => !v)}
          disabled={busy}
          className="shrink-0"
        >
          {enabled
            ? t("proxy.middleware.on", "已开启")
            : t("proxy.middleware.off", "已关闭")}
        </Button>
      </div>

      <Textarea
        value={script}
        onChange={(e) => setScript(e.target.value)}
        disabled={busy}
        rows={12}
        placeholder={EXAMPLE_SCRIPT}
        className="font-mono text-xs"
      />

      <p className="text-xs text-fg-2">
        {t(
          "proxy.middleware.contract",
          "脚本必须是一个求值得到函数的表达式，签名 (request, meta) => any。返回对象将作为新的请求体；返回非对象则按原样放行。",
        )}
      </p>
      <p className="text-xs text-fg-2">
        {t(
          "proxy.middleware.security",
          "脚本运行在隔离沙箱中：无网络、无文件、1 秒 CPU 上限；任何报错或超时都会放行原始请求，不会中断转发。",
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
