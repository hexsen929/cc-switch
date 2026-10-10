import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Loader2, Plus, Save, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { SegmentedControl } from "@/components/ui/segmented-control";
import { toast } from "@/lib/toast";
import { proxyApi } from "@/lib/api/proxy";
import type { AppId } from "@/lib/api";
import type { Provider } from "@/types";
import { useProvidersQuery } from "@/lib/query/queries";
import {
  type IntentKind,
  type IntentMatcher,
  type IntentRoutingConfig,
  type IntentRule,
} from "@/types/proxy";

const INTENT_KINDS: IntentKind[] = [
  "background",
  "thinking",
  "long-context",
  "web-search",
  "subagent",
];

function newId(): string {
  try {
    return crypto.randomUUID();
  } catch {
    return `r-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
  }
}

function newRule(): IntentRule {
  return {
    id: newId(),
    name: "",
    enabled: true,
    matcher: { type: "intent", intent: "background" },
    targetProviderId: "",
    model: "",
  };
}

export interface IntentRoutingPanelProps {
  appType: AppId;
}

/**
 * 意图路由（Fork 扩展）面板：按请求「意图」把目标 provider 软置顶为失败转移链 P1，
 * 可选改写上游模型。对应后端 intent_routing.rs。默认关闭、fail-safe：
 * 未命中 / 关闭 / 目标 provider 不存在 → 原样走默认路由，零行为变化。
 */
export function IntentRoutingPanel({ appType }: IntentRoutingPanelProps) {
  const { t } = useTranslation();
  const [enabled, setEnabled] = useState(false);
  const [rules, setRules] = useState<IntentRule[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);

  const { data: providersData } = useProvidersQuery(appType);
  const providers = useMemo(
    () => Object.values(providersData?.providers ?? {}),
    [providersData?.providers],
  );

  useEffect(() => {
    let active = true;
    setLoading(true);
    proxyApi
      .getIntentRoutingConfig(appType)
      .then((cfg) => {
        if (!active) return;
        setEnabled(cfg.enabled);
        setRules(cfg.rules ?? []);
      })
      .catch(() => {
        /* 读取失败保持默认（关闭 + 空规则） */
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [appType]);

  const busy = loading || saving;

  const updateRule = (i: number, patch: Partial<IntentRule>) =>
    setRules((rows) =>
      rows.map((r, idx) => (idx === i ? { ...r, ...patch } : r)),
    );
  const removeRule = (i: number) =>
    setRules((rows) => rows.filter((_, idx) => idx !== i));
  const addRule = () => setRules((rows) => [...rows, newRule()]);

  const setMatcherType = (i: number, type: "intent" | "predicate") => {
    const matcher: IntentMatcher =
      type === "intent"
        ? { type: "intent", intent: "background" }
        : { type: "predicate", predicate: {} };
    updateRule(i, { matcher });
  };

  const handleSave = async () => {
    if (saving) return;
    const cleaned: IntentRule[] = rules.map((r) => ({
      ...r,
      name: r.name.trim(),
      model: r.model?.trim() ? r.model.trim() : null,
    }));
    const cfg: IntentRoutingConfig = { enabled, rules: cleaned };
    setSaving(true);
    try {
      await proxyApi.setIntentRoutingConfig(appType, cfg);
      toast.success(t("proxy.intentRouting.saved", "意图路由已保存"), {
        closeButton: true,
      });
    } catch (e) {
      toast.error(
        t("proxy.intentRouting.saveFailed", "保存失败") + ":" + String(e),
      );
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="space-y-4">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <p className="text-sm font-medium">
            {t("proxy.intentRouting.title", "意图路由")}
          </p>
          <p className="mt-1 text-xs text-fg-2">
            {t(
              "proxy.intentRouting.hint",
              "按请求「意图」把目标供应商置顶为故障转移链的第一家，可选改写上游模型。按顺序取第一条命中的规则；默认关闭、未命中或目标不存在时原样放行。",
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
            ? t("proxy.intentRouting.on", "已开启")
            : t("proxy.intentRouting.off", "已关闭")}
        </Button>
      </div>

      <div className="flex items-center justify-between">
        <p className="text-xs font-medium text-fg-1">
          {t("proxy.intentRouting.rules", "规则（按顺序取首个命中）")}
        </p>
        <Button variant="neutral" size="sm" onClick={addRule} disabled={busy}>
          <Plus className="mr-1 h-3.5 w-3.5" />
          {t("proxy.intentRouting.addRule", "添加规则")}
        </Button>
      </div>

      {rules.length === 0 ? (
        <p className="text-xs text-fg-3">
          {t(
            "proxy.intentRouting.empty",
            "无规则。添加规则后，命中请求会被路由到指定供应商。",
          )}
        </p>
      ) : (
        <div className="space-y-3">
          {rules.map((rule, i) => (
            <RuleCard
              key={rule.id || i}
              rule={rule}
              providers={providers}
              disabled={busy}
              onChange={(patch) => updateRule(i, patch)}
              onChangeMatcherType={(type) => setMatcherType(i, type)}
              onRemove={() => removeRule(i)}
            />
          ))}
        </div>
      )}

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

interface RuleCardProps {
  rule: IntentRule;
  providers: Provider[];
  disabled: boolean;
  onChange: (patch: Partial<IntentRule>) => void;
  onChangeMatcherType: (type: "intent" | "predicate") => void;
  onRemove: () => void;
}

function RuleCard({
  rule,
  providers,
  disabled,
  onChange,
  onChangeMatcherType,
  onRemove,
}: RuleCardProps) {
  const { t } = useTranslation();
  const matcherType = rule.matcher.type;
  const predicate =
    rule.matcher.type === "predicate" ? rule.matcher.predicate : null;
  const intentKind =
    rule.matcher.type === "intent" ? rule.matcher.intent : null;

  const updatePredicate = (patch: Record<string, unknown>) => {
    if (rule.matcher.type !== "predicate") return;
    onChange({
      matcher: {
        type: "predicate",
        predicate: { ...rule.matcher.predicate, ...patch },
      },
    });
  };
  const updateBodyField = (patch: Record<string, unknown>) => {
    const cur = predicate?.bodyField ?? { path: "" };
    updatePredicate({ bodyField: { ...cur, ...patch } });
  };
  const intentLabel = (k: IntentKind) =>
    t(`proxy.intentRouting.intent.${k}`, k);

  return (
    <div className="space-y-3 rounded-md border border-border bg-surface-1 p-3">
      <div className="flex items-center gap-2">
        <Button
          variant={rule.enabled ? "solid" : "neutral"}
          size="sm"
          onClick={() => onChange({ enabled: !rule.enabled })}
          disabled={disabled}
          className="shrink-0"
        >
          {rule.enabled
            ? t("proxy.intentRouting.on", "已开启")
            : t("proxy.intentRouting.off", "已关闭")}
        </Button>
        <Input
          value={rule.name}
          onChange={(e) => onChange({ name: e.target.value })}
          disabled={disabled}
          placeholder={t("proxy.intentRouting.ruleName", "规则名（可选）")}
          className="text-xs"
        />
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

      <div className="flex items-center justify-between gap-2">
        <span className="text-xs text-fg-2">
          {t("proxy.intentRouting.matcher", "匹配方式")}
        </span>
        <SegmentedControl
          size="sm"
          aria-label={t("proxy.intentRouting.matcher", "匹配方式")}
          value={matcherType}
          onValueChange={(v) =>
            onChangeMatcherType(v as "intent" | "predicate")
          }
          items={[
            {
              value: "intent",
              label: t("proxy.intentRouting.matcherIntent", "语义意图"),
            },
            {
              value: "predicate",
              label: t("proxy.intentRouting.matcherPredicate", "自定义谓词"),
            },
          ]}
        />
      </div>
      {matcherType === "intent" ? (
        <Select
          value={intentKind ?? "background"}
          disabled={disabled}
          onValueChange={(v) =>
            onChange({ matcher: { type: "intent", intent: v as IntentKind } })
          }
        >
          <SelectTrigger className="text-xs">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {INTENT_KINDS.map((k) => (
              <SelectItem key={k} value={k}>
                {intentLabel(k)}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      ) : (
        <div className="space-y-2">
          <Input
            value={predicate?.modelGlob ?? ""}
            onChange={(e) => updatePredicate({ modelGlob: e.target.value })}
            disabled={disabled}
            placeholder={t(
              "proxy.intentRouting.pred.modelGlob",
              "模型名 glob，如 *haiku*（留空不限）",
            )}
            className="font-mono text-xs"
          />
          <Input
            value={predicate?.hasTool ?? ""}
            onChange={(e) => updatePredicate({ hasTool: e.target.value })}
            disabled={disabled}
            placeholder={t(
              "proxy.intentRouting.pred.hasTool",
              "带某工具（type/name 含该子串，留空不限）",
            )}
            className="font-mono text-xs"
          />
          <Input
            type="number"
            value={predicate?.minInputTokens ?? ""}
            onChange={(e) =>
              updatePredicate({
                minInputTokens: e.target.value ? Number(e.target.value) : null,
              })
            }
            disabled={disabled}
            placeholder={t(
              "proxy.intentRouting.pred.minTokens",
              "最小输入 token 估算（留空不限）",
            )}
            className="font-mono text-xs"
          />
          <div className="flex items-center gap-2">
            <Input
              value={predicate?.bodyField?.path ?? ""}
              onChange={(e) => updateBodyField({ path: e.target.value })}
              disabled={disabled}
              placeholder={t(
                "proxy.intentRouting.pred.fieldPath",
                "body 字段路径，如 metadata.user_id",
              )}
              className="font-mono text-xs"
            />
            <Input
              value={predicate?.bodyField?.contains ?? ""}
              onChange={(e) => updateBodyField({ contains: e.target.value })}
              disabled={disabled}
              placeholder={t(
                "proxy.intentRouting.pred.fieldContains",
                "包含子串",
              )}
              className="font-mono text-xs"
            />
          </div>
        </div>
      )}
      <div className="flex items-center gap-2">
        <Select
          value={rule.targetProviderId}
          disabled={disabled}
          onValueChange={(v) => onChange({ targetProviderId: v })}
        >
          <SelectTrigger className="flex-1 text-xs">
            <SelectValue
              placeholder={t(
                "proxy.intentRouting.selectProvider",
                "选择目标供应商",
              )}
            />
          </SelectTrigger>
          <SelectContent>
            {providers.length === 0 ? (
              <div className="px-2 py-4 text-center text-xs text-fg-3">
                {t("proxy.intentRouting.noProviders", "暂无供应商")}
              </div>
            ) : (
              providers.map((p) => (
                <SelectItem key={p.id} value={p.id}>
                  {p.name}
                </SelectItem>
              ))
            )}
          </SelectContent>
        </Select>
        <Input
          value={rule.model ?? ""}
          onChange={(e) => onChange({ model: e.target.value })}
          disabled={disabled}
          placeholder={t(
            "proxy.intentRouting.modelOverride",
            "改写模型（可选）",
          )}
          className="flex-1 font-mono text-xs"
        />
      </div>
    </div>
  );
}
