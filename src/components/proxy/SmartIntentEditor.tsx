import { useState } from "react";
import { useTranslation } from "react-i18next";
import { ArrowUp, Plus, Trash2 } from "lucide-react";
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
  const [selected, select] = useState("");
  const [name, setName] = useState("");
  const initial: SmartIntentConfig = {
    enabled: false,
    classifiers: [blank()],
    categories: [
      { description: t("smartIntent.easy"), targets: [blank()], tier: "easy" },
      { description: t("smartIntent.hard"), targets: [blank()], tier: "hard" },
    ],
    prompt: "",
    timeoutMs: 1800,
    keepWarm: false,
  };
  const root = value ?? initial;
  const groups = root.groups ?? {};
  const id = selected && Object.hasOwn(groups, selected) ? selected : "";
  const cfg = id ? groups[id] : root;
  const update = (patch: Partial<SmartIntentConfig>) =>
    id
      ? onChange({ ...root, groups: { ...groups, [id]: { ...cfg, ...patch } } })
      : onChange({ ...root, ...patch });
  const groupName = name.trim();
  const canAdd =
    groupName.length > 0 &&
    !groupName.startsWith("$") &&
    !Object.hasOwn(Object.prototype, groupName) &&
    !Object.hasOwn(groups, groupName) &&
    new TextEncoder().encode(groupName).length <= 96 &&
    Object.keys(groups).length < 16;
  const inUse =
    !!id &&
    [root, ...Object.values(groups)].some((g) =>
      [...(g.members ?? []), ...g.categories.flatMap((c) => c.targets)].some(
        (t) => t.groupId === id,
      ),
    );
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
    optional = false,
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
              value={
                row.groupId
                  ? "group:" + row.groupId
                  : row.providerId
                    ? "card:" + row.providerId
                    : ""
              }
              onChange={(e) => {
                const v = e.target.value;
                if (v.startsWith("group:"))
                  change(
                    list.map((r, j) =>
                      j === i ? { ...blank(), groupId: v.slice(6) } : r,
                    ),
                  );
                else edit({ providerId: v.slice(5), groupId: null });
              }}
            >
              <option value="">{t("smartIntent.provider")}</option>
              {row.providerId &&
                !providers.some((p) => p.id === row.providerId) && (
                  <option value={"card:" + row.providerId}>
                    {t("smartIntent.missing")}
                  </option>
                )}
              {providers.map((p) => (
                <option value={"card:" + p.id} key={p.id}>
                  {p.name}
                </option>
              ))}
              {!classifier &&
                Object.keys(groups)
                  .filter((k) => k !== id)
                  .map((k) => (
                    <option key={"g:" + k} value={"group:" + k}>
                      {t("smartIntent.group")} · {k}
                    </option>
                  ))}
            </select>
            {!row.groupId && (
              <Input
                className="min-w-32 flex-1 font-mono text-xs"
                aria-label={t("smartIntent.model")}
                placeholder={t("smartIntent.model")}
                maxLength={256}
                value={row.model}
                onChange={(e) => edit({ model: e.target.value.trim() })}
              />
            )}
            {classifier && (
              <select
                className={selectClass}
                value={row.protocol ?? "chat"}
                aria-label={t("smartIntent.protocol")}
                onChange={(e) =>
                  edit({ protocol: e.target.value as IntentTarget["protocol"] })
                }
              >
                <option value="chat">{t("smartIntent.chat")}</option>
                <option value="openaiDecisions">OpenAI Decisions</option>
                <option value="systemOne">System One</option>
                <option value="googleDecisions">Google Decisions</option>
              </select>
            )}
            {!classifier && !row.groupId && (
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
            {i > 0 && (
              <Button
                type="button"
                variant="ghost"
                size="sm"
                aria-label={t("smartIntent.moveUp")}
                title={t("smartIntent.moveUp")}
                onClick={() => {
                  const a = [...list];
                  [a[i - 1], a[i]] = [a[i], a[i - 1]];
                  change(a);
                }}
              >
                <ArrowUp size={14} />
              </Button>
            )}
            {(list.length > 1 || optional) && (
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
          {t(
            optional && !list.length
              ? "smartIntent.addMember"
              : "smartIntent.addBackup",
          )}
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
          checked={root.enabled}
          aria-label={t("smartIntent.title")}
          onCheckedChange={(v) => onChange({ ...root, enabled: v })}
        />
      </label>
      {root.enabled && (
        <>
          <p className="text-caption text-fg-2">{t("smartIntent.consent")}</p>
          <div className="flex flex-wrap items-center gap-2 border-b border-border pb-2">
            <label className="text-caption">
              {t("smartIntent.group")}{" "}
              <select
                className={selectClass}
                aria-label={t("smartIntent.group")}
                value={id}
                onChange={(e) => select(e.target.value)}
              >
                <option value="">{t("smartIntent.root")}</option>
                {Object.keys(groups).map((k) => (
                  <option key={k}>{k}</option>
                ))}
              </select>
            </label>
            <Input
              className="w-36"
              value={name}
              aria-label={t("smartIntent.groupName")}
              placeholder={t("smartIntent.groupName")}
              onChange={(e) => setName(e.target.value)}
            />
            <Button
              variant="neutral"
              size="sm"
              type="button"
              disabled={!canAdd}
              onClick={() => {
                onChange({
                  ...root,
                  groups: {
                    ...groups,
                    [groupName]: {
                      ...initial,
                      enabled: true,
                      orderedOnly: true,
                      members: [blank()],
                    },
                  },
                });
                select(groupName);
                setName("");
              }}
            >
              <Plus size={14} />
              {t("smartIntent.addGroup")}
            </Button>
            {id && (
              <Button
                variant="ghost"
                size="sm"
                type="button"
                disabled={inUse}
                aria-label={t("smartIntent.removeGroup")}
                title={t(
                  inUse ? "smartIntent.groupInUse" : "smartIntent.removeGroup",
                )}
                onClick={() => {
                  const next = { ...groups };
                  delete next[id];
                  onChange({ ...root, groups: next });
                  select("");
                }}
              >
                <Trash2 size={14} />
              </Button>
            )}
          </div>
          <label className="flex items-center justify-between gap-2 text-caption">
            {t("smartIntent.ordered")}
            <Switch
              checked={!!cfg.orderedOnly}
              onCheckedChange={(orderedOnly) =>
                update({
                  orderedOnly,
                  members:
                    orderedOnly && !cfg.members?.length
                      ? [blank()]
                      : cfg.members,
                })
              }
            />
          </label>
          {!cfg.orderedOnly && (
            <>
              <label className="flex items-center justify-between gap-2 text-caption">
                {t("smartIntent.keepWarm")}
                <Switch
                  checked={!!cfg.keepWarm}
                  onCheckedChange={(keepWarm) => update({ keepWarm })}
                />
              </label>
              <div className="text-caption font-semibold">
                {t("smartIntent.classifier")}
              </div>
              {rows(
                cfg.classifiers,
                (classifiers) => update({ classifiers }),
                true,
              )}
              {cfg.classifiers.some(
                (c) => c.protocol && c.protocol !== "chat",
              ) && (
                <label className="block text-caption">
                  {t("smartIntent.confidence")}
                  <Input
                    type="number"
                    className="w-28"
                    min={0}
                    max={100}
                    value={cfg.minConfidence ?? 40}
                    onChange={(e) =>
                      update({ minConfidence: Number(e.target.value) })
                    }
                  />
                </label>
              )}
              {cfg.categories.map((c, i) => (
                <div key={i} className="space-y-2 border-t border-border pt-2">
                  <div className="flex items-center gap-2">
                    <span className="text-caption">{i + 1}.</span>
                    <Input
                      maxLength={512}
                      aria-label={t("smartIntent.category")}
                      value={c.description}
                      onChange={(e) =>
                        category(i, { description: e.target.value })
                      }
                    />
                    {cfg.categories.length > 2 && (
                      <Button
                        type="button"
                        variant="ghost"
                        size="sm"
                        aria-label={t("smartIntent.remove")}
                        onClick={() =>
                          update({
                            categories: cfg.categories.filter(
                              (_, j) => j !== i,
                            ),
                          })
                        }
                      >
                        <Trash2 size={14} />
                      </Button>
                    )}
                  </div>
                  {cfg.keepWarm && (
                    <select
                      className={selectClass}
                      aria-label={t("smartIntent.tier")}
                      value={c.tier ?? ""}
                      onChange={(e) =>
                        category(i, {
                          tier:
                            e.target.value === "hard"
                              ? "hard"
                              : e.target.value === "easy"
                                ? "easy"
                                : null,
                        })
                      }
                    >
                      <option value="">{t("smartIntent.normalTier")}</option>
                      <option value="easy">{t("smartIntent.easyTier")}</option>
                      <option value="hard">{t("smartIntent.hardTier")}</option>
                    </select>
                  )}
                  {rows(
                    c.targets,
                    (targets) => category(i, { targets }),
                    false,
                  )}
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
            </>
          )}
          <div className="text-caption font-semibold">
            {t("smartIntent.members")}
          </div>
          {rows(
            cfg.members ?? [],
            (members) => update({ members }),
            false,
            true,
          )}
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
