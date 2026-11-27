import { useTranslation } from "react-i18next";
import { Plus, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import type { IntentTarget, SmartIntentConfig } from "@/types/proxy";
import type { Provider } from "@/types";

const blank = (): IntentTarget => ({ providerId: "", model: "" });
const selectClass =
  "min-w-0 rounded-md border border-border bg-surface px-2 py-1 text-xs";

type Props = {
  providers: Provider[];
  value?: SmartIntentConfig;
  onChange: (next: SmartIntentConfig) => void;
  disabled: boolean;
};

export function SmartIntentEditor({
  providers,
  value,
  onChange,
  disabled,
}: Props) {
  const { t } = useTranslation();
  const cfg: SmartIntentConfig = value ?? {
    enabled: false,
    classifiers: [blank()],
    categories: [
      { description: t("smartIntent.easy"), targets: [blank()] },
      { description: t("smartIntent.hard"), targets: [blank()] },
    ],
    prompt: "",
    timeoutMs: 1800,
  };
  const update = (patch: Partial<SmartIntentConfig>) =>
    onChange({ ...cfg, ...patch });
  const category = (
    i: number,
    patch: Partial<SmartIntentConfig["categories"][number]>,
  ) =>
    update({
      categories: cfg.categories.map((c, j) =>
        i === j ? { ...c, ...patch } : c,
      ),
    });
  const rows = (
    list: IntentTarget[],
    change: (next: IntentTarget[]) => void,
    classifier: boolean,
  ) => (
    <div className="space-y-1">
      {list.map((row, i) => {
        const edit = (patch: Partial<IntentTarget>) =>
          change(list.map((r, j) => (i === j ? { ...r, ...patch } : r)));
        return (
          <div key={i} className="flex flex-wrap items-center gap-2">
            <span className="text-caption text-fg-3">{i + 1}.</span>
            <select
              className={selectClass}
              aria-label={t("smartIntent.provider")}
              value={row.providerId}
              onChange={(e) => edit({ providerId: e.target.value })}
            >
              <option value="">{t("smartIntent.provider")}</option>
              {row.providerId &&
                !providers.some((p) => p.id === row.providerId) && (
                  <option value={row.providerId}>
                    {t("smartIntent.missing")}
                  </option>
                )}
              {providers.map((p) => (
                <option value={p.id} key={p.id}>
                  {p.name}
                </option>
              ))}
            </select>
            <Input
              className="min-w-32 flex-1 font-mono text-xs"
              aria-label={t("smartIntent.model")}
              placeholder={t("smartIntent.model")}
              maxLength={256}
              value={row.model}
              onChange={(e) => edit({ model: e.target.value.trim() })}
            />
            {!classifier && (
              <select
                aria-label={t("smartIntent.effort")}
                className={selectClass}
                value={row.effort ?? ""}
                onChange={(e) => edit({ effort: e.target.value || null })}
              >
                <option value="">{t("smartIntent.effort")}</option>
                {["low", "medium", "high", "xhigh"].map((v) => (
                  <option key={v}>{v}</option>
                ))}
              </select>
            )}
            {list.length > 1 && (
              <Button
                type="button"
                variant="ghost"
                size="sm"
                aria-label={t("smartIntent.remove")}
                onClick={() => change(list.filter((_, j) => j !== i))}
              >
                <Trash2 size={14} />
              </Button>
            )}
          </div>
        );
      })}
      {list.length < (classifier ? 3 : 8) && (
        <Button
          type="button"
          variant="neutral"
          size="sm"
          onClick={() => change([...list, blank()])}
        >
          <Plus size={14} />
          {t("smartIntent.addBackup")}
        </Button>
      )}
    </div>
  );
  return (
    <fieldset
      disabled={disabled}
      className="space-y-3 border-t border-border pt-3"
    >
      <label className="flex items-center justify-between gap-2 text-sm font-medium">
        {t("smartIntent.title")}
        <Switch
          checked={cfg.enabled}
          aria-label={t("smartIntent.title")}
          onCheckedChange={(v) => update({ enabled: v })}
        />
      </label>
      {cfg.enabled && (
        <>
          <p className="text-caption text-fg-2">{t("smartIntent.consent")}</p>
          <div className="text-caption font-semibold">
            {t("smartIntent.classifier")}
          </div>
          {rows(
            cfg.classifiers,
            (classifiers) => update({ classifiers }),
            true,
          )}
          {cfg.categories.map((c, i) => (
            <div key={i} className="space-y-2 border-t border-border pt-2">
              <div className="flex items-center gap-2">
                <span className="text-caption">{i + 1}.</span>
                <Input
                  maxLength={512}
                  aria-label={t("smartIntent.category")}
                  value={c.description}
                  onChange={(e) => category(i, { description: e.target.value })}
                />
                {cfg.categories.length > 2 && (
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    aria-label={t("smartIntent.remove")}
                    onClick={() =>
                      update({
                        categories: cfg.categories.filter((_, j) => j !== i),
                      })
                    }
                  >
                    <Trash2 size={14} />
                  </Button>
                )}
              </div>
              {rows(c.targets, (targets) => category(i, { targets }), false)}
            </div>
          ))}
          {cfg.categories.length < 16 && (
            <Button
              type="button"
              variant="neutral"
              size="sm"
              onClick={() =>
                update({
                  categories: [
                    ...cfg.categories,
                    { description: "", targets: [blank()] },
                  ],
                })
              }
            >
              <Plus size={14} />
              {t("smartIntent.addCategory")}
            </Button>
          )}
          <label className="block text-caption">
            {t("smartIntent.prompt")}
            <Input
              maxLength={2048}
              value={cfg.prompt}
              onChange={(e) => update({ prompt: e.target.value })}
            />
          </label>
          <label className="block text-caption">
            {t("smartIntent.timeout")}
            <Input
              className="w-28"
              type="number"
              min={200}
              max={8000}
              value={cfg.timeoutMs}
              onChange={(e) => update({ timeoutMs: Number(e.target.value) })}
            />
          </label>
        </>
      )}
    </fieldset>
  );
}
