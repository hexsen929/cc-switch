import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Loader2, Plus, Save, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { toast } from "@/lib/toast";
import { proxyApi } from "@/lib/api/proxy";
import type { AppId } from "@/lib/api";
import {
  EMPTY_PII_REDACTION,
  type PiiBuiltinToggles,
  type PiiCustomRule,
  type PiiRedactionConfig,
} from "@/types/proxy";

const BUILTIN_KEYS: (keyof PiiBuiltinToggles)[] = [
  "creditCard",
  "usSsn",
  "iban",
  "email",
  "awsAccessKeyId",
  "privateKeyBlock",
  "jwt",
];

export interface PiiRedactionPanelProps {
  appType: AppId;
}

/**
 * 出站请求脱敏（Fork 扩展）面板：转发给上游前把敏感信息打码。对应后端 pii_redaction.rs。
 * 默认关闭、fail-open：仅在开启且至少启用一条规则时生效；只改消息文本，不碰结构字段。
 */
export function PiiRedactionPanel({ appType }: PiiRedactionPanelProps) {
  const { t } = useTranslation();
  const [cfg, setCfg] = useState<PiiRedactionConfig>(EMPTY_PII_REDACTION);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    let active = true;
    setLoading(true);
    proxyApi
      .getPiiRedactionConfig(appType)
      .then((c) => {
        if (active) setCfg({ ...EMPTY_PII_REDACTION, ...c });
      })
      .catch(() => {
        /* 读取失败保持默认（关闭） */
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [appType]);

  // PII_PANEL_BODY
  const busy = loading || saving;

  const toggleBuiltin = (key: keyof PiiBuiltinToggles) =>
    setCfg((c) => ({
      ...c,
      builtins: { ...c.builtins, [key]: !c.builtins[key] },
    }));

  const updateRule = (i: number, patch: Partial<PiiCustomRule>) =>
    setCfg((c) => ({
      ...c,
      customRules: c.customRules.map((r, idx) =>
        idx === i ? { ...r, ...patch } : r,
      ),
    }));
  const removeRule = (i: number) =>
    setCfg((c) => ({
      ...c,
      customRules: c.customRules.filter((_, idx) => idx !== i),
    }));
  const addRule = () =>
    setCfg((c) => ({
      ...c,
      customRules: [...c.customRules, { name: "", pattern: "" }],
    }));

  const handleSave = async () => {
    if (saving) return;
    const cleaned: PiiRedactionConfig = {
      ...cfg,
      placeholder: cfg.placeholder.trim(),
      customRules: cfg.customRules
        .map((r) => ({ name: r.name.trim(), pattern: r.pattern.trim() }))
        .filter((r) => r.pattern.length > 0),
    };
    setSaving(true);
    try {
      await proxyApi.setPiiRedactionConfig(appType, cleaned);
      setCfg(cleaned);
      toast.success(t("proxy.pii.saved", "脱敏配置已保存"), {
        closeButton: true,
      });
    } catch (e) {
      toast.error(t("proxy.pii.saveFailed", "保存失败") + ":" + String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="space-y-4">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <p className="text-sm font-medium">
            {t("proxy.pii.title", "出站脱敏")}
          </p>
          <p className="mt-1 text-xs text-fg-2">
            {t(
              "proxy.pii.hint",
              "转发给上游之前，把消息文本里命中的敏感信息替换为占位符。只改消息文本，不碰模型名/角色/工具结构；默认关闭，正则出错时原样放行。",
            )}
          </p>
        </div>
        <Button
          variant={cfg.enabled ? "solid" : "neutral"}
          size="regular"
          onClick={() => setCfg((c) => ({ ...c, enabled: !c.enabled }))}
          disabled={busy}
          className="shrink-0"
        >
          {cfg.enabled
            ? t("proxy.pii.on", "已开启")
            : t("proxy.pii.off", "已关闭")}
        </Button>
      </div>

      <div className="space-y-2">
        <p className="text-xs font-medium text-fg-1">
          {t("proxy.pii.builtins", "内置模式（高置信度）")}
        </p>
        <div className="flex flex-wrap gap-1.5">
          {BUILTIN_KEYS.map((key) => (
            <Button
              key={key}
              variant={cfg.builtins[key] ? "solid" : "neutral"}
              size="sm"
              onClick={() => toggleBuiltin(key)}
              disabled={busy}
            >
              {t(`proxy.pii.builtin.${key}`, key)}
            </Button>
          ))}
        </div>
      </div>

      <div className="flex items-center justify-between">
        <p className="text-xs font-medium text-fg-1">
          {t("proxy.pii.customRules", "自定义正则")}
        </p>
        <Button variant="neutral" size="sm" onClick={addRule} disabled={busy}>
          <Plus className="mr-1 h-3.5 w-3.5" />
          {t("proxy.pii.addRule", "添加规则")}
        </Button>
      </div>
      {cfg.customRules.length === 0 ? (
        <p className="text-xs text-fg-3">
          {t("proxy.pii.noRules", "无自定义规则。可按需添加正则匹配。")}
        </p>
      ) : (
        <div className="space-y-2">
          {cfg.customRules.map((rule, i) => (
            <div key={i} className="flex items-center gap-2">
              <Input
                value={rule.name}
                onChange={(e) => updateRule(i, { name: e.target.value })}
                disabled={busy}
                placeholder={t("proxy.pii.ruleName", "标签（可选）")}
                className="w-32 shrink-0 text-xs"
              />
              <Input
                value={rule.pattern}
                onChange={(e) => updateRule(i, { pattern: e.target.value })}
                disabled={busy}
                placeholder={t(
                  "proxy.pii.rulePattern",
                  "正则，如 sk-[A-Za-z0-9]+",
                )}
                className="flex-1 font-mono text-xs"
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

      <Input
        value={cfg.placeholder}
        onChange={(e) => setCfg((c) => ({ ...c, placeholder: e.target.value }))}
        disabled={busy}
        placeholder={t(
          "proxy.pii.placeholder",
          "占位符模板，支持 {label}；留空用 [REDACTED:{label}]",
        )}
        className="font-mono text-xs"
      />

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
