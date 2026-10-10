import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Loader2, Plus, Save, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { SegmentedControl } from "@/components/ui/segmented-control";
import { toast } from "@/lib/toast";
import { proxyApi } from "@/lib/api/proxy";
import {
  type ModelMapRule,
  type RequestRewriteConfig,
  type SystemPromptMode,
} from "@/types/proxy";

export interface RequestRewritePanelProps {
  appType: string;
}

/**
 * 声明式「现成请求中间件」（Fork 扩展）面板：无代码改模型名 / 覆盖参数 / 注入 system。
 * 对应后端 request_rewrite.rs。默认关闭、fail-safe：空规则或无法识别的结构一律原样放行。
 * system 注入 v1 仅对 Claude（Anthropic 原生 `system`）生效。
 */
export function RequestRewritePanel({ appType }: RequestRewritePanelProps) {
  const { t } = useTranslation();
  const [enabled, setEnabled] = useState(false);
  const [modelMap, setModelMap] = useState<ModelMapRule[]>([]);
  const [sysMode, setSysMode] = useState<SystemPromptMode>("off");
  const [sysText, setSysText] = useState("");
  const [paramsText, setParamsText] = useState("");
  const [paramsError, setParamsError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    let active = true;
    setLoading(true);
    proxyApi
      .getRequestRewriteConfig(appType)
      .then((cfg) => {
        if (!active) return;
        setEnabled(cfg.enabled);
        setModelMap(cfg.modelMap ?? []);
        setSysMode(cfg.systemPrompt?.mode ?? "off");
        setSysText(cfg.systemPrompt?.text ?? "");
        const po = cfg.paramOverrides ?? {};
        setParamsText(
          Object.keys(po).length ? JSON.stringify(po, null, 2) : "",
        );
        setParamsError(null);
      })
      .catch(() => {
        /* 读取失败时保持默认（关闭 + 空规则） */
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [appType]);

  const busy = loading || saving;

  const parseParams = (): Record<string, unknown> | null => {
    const raw = paramsText.trim();
    if (!raw) return {};
    try {
      const parsed = JSON.parse(raw);
      if (
        typeof parsed !== "object" ||
        parsed === null ||
        Array.isArray(parsed)
      ) {
        return null;
      }
      return parsed as Record<string, unknown>;
    } catch {
      return null;
    }
  };

  const handleSave = async () => {
    if (saving) return;
    const params = parseParams();
    if (params === null) {
      const msg = t(
        "proxy.rewrite.paramsInvalid",
        "参数覆盖必须是合法的 JSON 对象",
      );
      setParamsError(msg);
      toast.error(msg);
      return;
    }
    setParamsError(null);
    const cfg: RequestRewriteConfig = {
      enabled,
      modelMap: modelMap.filter((r) => r.from.trim() && r.to.trim()),
      paramOverrides: params,
      systemPrompt: { mode: sysMode, text: sysText },
    };
    setSaving(true);
    try {
      await proxyApi.setRequestRewriteConfig(appType, cfg);
      toast.success(t("proxy.rewrite.saved", "现成中间件已保存"), {
        closeButton: true,
      });
    } catch (e) {
      toast.error(t("proxy.rewrite.saveFailed", "保存失败") + ":" + String(e));
    } finally {
      setSaving(false);
    }
  };

  const addRule = () => setModelMap((rows) => [...rows, { from: "", to: "" }]);
  const removeRule = (i: number) =>
    setModelMap((rows) => rows.filter((_, idx) => idx !== i));
  const updateRule = (i: number, patch: Partial<ModelMapRule>) =>
    setModelMap((rows) =>
      rows.map((r, idx) => (idx === i ? { ...r, ...patch } : r)),
    );

  return (
    <div className="space-y-4">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <p className="text-sm font-medium">
            {t("proxy.rewrite.title", "现成请求中间件（无代码）")}
          </p>
          <p className="mt-1 text-xs text-fg-2">
            {t(
              "proxy.rewrite.hint",
              "用结构化配置完成最常见的三类改写：改模型名、覆盖参数、注入 system 提示——不必写脚本。先于可编程脚本执行，默认关闭、失败放行。",
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
            ? t("proxy.rewrite.on", "已开启")
            : t("proxy.rewrite.off", "已关闭")}
        </Button>
      </div>

      {/* 模型名映射 */}
      <div className="space-y-2">
        <div className="flex items-center justify-between">
          <p className="text-xs font-medium text-fg-1">
            {t("proxy.rewrite.modelMap.title", "模型名映射")}
          </p>
          <Button variant="neutral" size="sm" onClick={addRule} disabled={busy}>
            <Plus className="mr-1 h-3.5 w-3.5" />
            {t("proxy.rewrite.modelMap.add", "添加规则")}
          </Button>
        </div>
        {modelMap.length === 0 ? (
          <p className="text-xs text-fg-3">
            {t(
              "proxy.rewrite.modelMap.empty",
              "无规则。from 用 * 可作为兜底，匹配所有未命中的模型。",
            )}
          </p>
        ) : (
          <div className="space-y-2">
            {modelMap.map((rule, i) => (
              <div key={i} className="flex items-center gap-2">
                <Input
                  value={rule.from}
                  onChange={(e) => updateRule(i, { from: e.target.value })}
                  disabled={busy}
                  placeholder={t("proxy.rewrite.modelMap.from", "原模型名 / *")}
                  className="font-mono text-xs"
                />
                <span className="text-fg-3">→</span>
                <Input
                  value={rule.to}
                  onChange={(e) => updateRule(i, { to: e.target.value })}
                  disabled={busy}
                  placeholder={t("proxy.rewrite.modelMap.to", "目标模型名")}
                  className="font-mono text-xs"
                />
                <Button
                  variant="neutral"
                  size="sm"
                  onClick={() => removeRule(i)}
                  disabled={busy}
                  className="shrink-0"
                  aria-label={t("common.delete", "删除")}
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </Button>
              </div>
            ))}
          </div>
        )}
      </div>

      {/* 参数覆盖 */}
      <div className="space-y-2">
        <p className="text-xs font-medium text-fg-1">
          {t("proxy.rewrite.params.title", "参数覆盖")}
        </p>
        <Textarea
          value={paramsText}
          onChange={(e) => {
            setParamsText(e.target.value);
            if (paramsError) setParamsError(null);
          }}
          disabled={busy}
          rows={5}
          placeholder={
            '{\n  "temperature": 0.3,\n  "max_tokens": 4096,\n  "top_p": null\n}'
          }
          className="font-mono text-xs"
          aria-invalid={paramsError ? true : undefined}
        />
        {paramsError ? (
          <p className="text-xs text-danger">{paramsError}</p>
        ) : (
          <p className="text-xs text-fg-3">
            {t(
              "proxy.rewrite.params.hint",
              "顶层 JSON 对象，覆盖/新增出站参数；值设为 null 表示删除该参数。",
            )}
          </p>
        )}
      </div>

      {/* system 提示注入 */}
      <div className="space-y-2">
        <div className="flex items-center justify-between gap-2">
          <p className="text-xs font-medium text-fg-1">
            {t("proxy.rewrite.system.title", "system 提示注入")}
          </p>
          <SegmentedControl
            size="sm"
            aria-label={t("proxy.rewrite.system.title", "system 提示注入")}
            value={sysMode}
            onValueChange={setSysMode}
            items={[
              { value: "off", label: t("proxy.rewrite.system.off", "关闭") },
              {
                value: "prepend",
                label: t("proxy.rewrite.system.prepend", "前置"),
              },
              {
                value: "append",
                label: t("proxy.rewrite.system.append", "追加"),
              },
              {
                value: "replace",
                label: t("proxy.rewrite.system.replace", "替换"),
              },
            ]}
          />
        </div>
        {sysMode !== "off" && (
          <Textarea
            value={sysText}
            onChange={(e) => setSysText(e.target.value)}
            disabled={busy}
            rows={4}
            placeholder={t(
              "proxy.rewrite.system.placeholder",
              "要注入的 system 文本",
            )}
            className="text-xs"
          />
        )}
        <p className="text-xs text-fg-3">
          {t(
            "proxy.rewrite.system.claudeOnly",
            "仅对 Claude（Anthropic 原生 system 字段）生效；其它应用请用可编程脚本注入。",
          )}
        </p>
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
