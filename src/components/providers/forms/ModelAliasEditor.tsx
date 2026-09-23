import { useCallback } from "react";
import { useTranslation } from "react-i18next";
import { Plus, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { FormLabel } from "@/components/ui/form";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";
import {
  createModelAliasEntry,
  type ModelAliasEntry,
} from "@/utils/modelAliases";

interface ModelAliasEditorProps {
  entries: ModelAliasEntry[];
  onChange: (entries: ModelAliasEntry[]) => void;
  className?: string;
}

/**
 * 精确模型别名编辑器（Codex / Claude 供应商共用）。
 *
 * 每行一条“源模型 → 目标模型”的等值替换。与 modelCatalog / 家族映射独立：
 * 转发前对请求模型做严格等值改写。典型用途：把 Codex 内部审批模型
 * codex-auto-review 改写成真实上游模型 gpt-5.6-sol，使中转不再收到未知模型名。
 */
export function ModelAliasEditor({
  entries,
  onChange,
  className,
}: ModelAliasEditorProps) {
  const { t } = useTranslation();

  const handleAdd = useCallback(() => {
    onChange([...entries, createModelAliasEntry()]);
  }, [entries, onChange]);

  const handleUpdate = useCallback(
    (id: string, patch: Partial<Pick<ModelAliasEntry, "source" | "target">>) => {
      onChange(
        entries.map((entry) =>
          entry.id === id ? { ...entry, ...patch } : entry,
        ),
      );
    },
    [entries, onChange],
  );

  const handleRemove = useCallback(
    (id: string) => {
      onChange(entries.filter((entry) => entry.id !== id));
    },
    [entries, onChange],
  );

  return (
    <div className={cn("space-y-4", className)}>
      <div className="space-y-1">
        <div className="flex items-center justify-between gap-3">
          <FormLabel>
            {t("modelAlias.title", { defaultValue: "精确模型别名" })}
          </FormLabel>
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={handleAdd}
            className="h-8 gap-1"
          >
            <Plus className="h-4 w-4" />
            {t("modelAlias.add", { defaultValue: "添加别名" })}
          </Button>
        </div>
        <p className="text-xs leading-relaxed text-muted-foreground">
          {t("modelAlias.hint", {
            defaultValue:
              "转发前把“源模型”严格等值改写为“目标模型”，独立于模型映射与菜单目录，覆盖原生 Responses 直通路径。常见用途：把 codex-auto-review 改写为你的真实模型。",
          })}
        </p>
      </div>

      {entries.length > 0 && (
        <div className="space-y-2">
          <div className="hidden grid-cols-[1fr_1fr_36px] gap-2 px-1 text-xs font-medium text-muted-foreground md:grid">
            <span>
              {t("modelAlias.sourceColumn", { defaultValue: "源模型" })}
            </span>
            <span>
              {t("modelAlias.targetColumn", { defaultValue: "目标模型" })}
            </span>
            <span />
          </div>

          {entries.map((entry) => (
            <div
              key={entry.id}
              className="grid grid-cols-1 gap-2 md:grid-cols-[1fr_1fr_36px]"
            >
              <Input
                value={entry.source}
                onChange={(event) =>
                  handleUpdate(entry.id, { source: event.target.value })
                }
                placeholder={t("modelAlias.sourcePlaceholder", {
                  defaultValue: "例如: codex-auto-review",
                })}
                aria-label={t("modelAlias.sourceColumn", {
                  defaultValue: "源模型",
                })}
              />
              <Input
                value={entry.target}
                onChange={(event) =>
                  handleUpdate(entry.id, { target: event.target.value })
                }
                placeholder={t("modelAlias.targetPlaceholder", {
                  defaultValue: "例如: gpt-5.6-sol",
                })}
                aria-label={t("modelAlias.targetColumn", {
                  defaultValue: "目标模型",
                })}
              />
              <Button
                type="button"
                variant="ghost"
                size="icon"
                className="h-9 w-9 text-muted-foreground hover:text-destructive"
                onClick={() => handleRemove(entry.id)}
                title={t("common.delete", { defaultValue: "删除" })}
              >
                <Trash2 className="h-4 w-4" />
              </Button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
