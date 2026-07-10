import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { Checkbox } from "@/components/ui/checkbox";
import { Label } from "@/components/ui/label";
import { AlertTriangle } from "lucide-react";

/**
 * Codex 中转兼容性 Panel
 *
 * 让用户精细勾选要从 Codex 请求体工具声明中剥除的 OpenAI 内置工具。
 *
 * ## 背景
 * Codex CLI 在 ChatGPT 登录态或加载官方 plugin 时，会自动在 `tools` 或
 * `input[].additional_tools.tools` 中注入内置工具。许多
 * 第三方中转对这些工具不开放权限，会返回 `403 Image generation is not enabled
 * for this group`。本 Panel 让用户按 provider 维度勾选剥除哪些。
 *
 * ## 行为
 * - 仅在 cc-switch 代理转发时生效
 * - 默认全部不勾 = 完全透传，不破坏现有行为
 * - 写入 provider.settings_config.codex_strip_tools 数组
 */

export interface CodexToolStripPanelProps {
  value: string[];
  onChange: (next: string[]) => void;
}

/** OpenAI 官方内置工具清单（按 codex CLI 当前已知会注入的范围）。 */
interface ToolEntry {
  type: string;
  /** 描述 key 或文本（兼容无 i18n key 时直接渲染英文）。 */
  label: string;
  hint?: string;
}

const KNOWN_BUILTIN_TOOLS: ToolEntry[] = [
  {
    type: "image_generation",
    label: "image_generation",
    hint: "图像生成。兼容 hosted 工具和新版 image_gen/imagegen 扩展",
  },
  {
    type: "web_search_preview",
    label: "web_search_preview",
    hint: "网页搜索预览。多数中转不支持",
  },
  {
    type: "web_search",
    label: "web_search",
    hint: "网页搜索（正式版）",
  },
  {
    type: "computer_use_preview",
    label: "computer_use_preview",
    hint: "Computer Use 自动化",
  },
  {
    type: "file_search",
    label: "file_search",
    hint: "文件搜索（向量库）",
  },
  {
    type: "code_interpreter",
    label: "code_interpreter",
    hint: "代码解释器",
  },
  {
    type: "mcp",
    label: "mcp",
    hint: "OpenAI 内置 MCP 桥（不影响用户自定义 MCP server）",
  },
];

export function CodexToolStripPanel({
  value,
  onChange,
}: CodexToolStripPanelProps) {
  const { t } = useTranslation();
  const valueSet = useMemo(() => new Set(value), [value]);

  const toggle = (toolType: string, checked: boolean) => {
    const next = new Set(valueSet);
    if (checked) {
      next.add(toolType);
    } else {
      next.delete(toolType);
    }
    onChange(Array.from(next));
  };

  return (
    <details className="rounded-lg border border-border bg-muted/20 px-4 py-3">
      <summary className="cursor-pointer select-none text-sm font-medium flex items-center gap-2">
        <AlertTriangle className="h-4 w-4 text-amber-500" />
        {t("provider.codexToolStrip.title", "中转兼容性：剥除内置工具")}
        {value.length > 0 && (
          <span className="text-xs text-muted-foreground">
            ({value.length} {t("provider.codexToolStrip.selected", "项已启用")})
          </span>
        )}
      </summary>

      <div className="mt-3 space-y-3">
        <p className="text-xs text-muted-foreground leading-relaxed">
          {t(
            "provider.codexToolStrip.description",
            'Codex CLI 在 ChatGPT 登录态或加载 plugin 时会在请求中自动注入 OpenAI 内置工具。多数第三方中转对这些工具不开放权限，会返回 403 "Image generation is not enabled for this group" 等错误。在此勾选要从转发请求中剥除的工具。仅在 cc-switch 代理转发时生效，默认不勾 = 完全透传。',
          )}
        </p>

        <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
          {KNOWN_BUILTIN_TOOLS.map((tool) => {
            const checked = valueSet.has(tool.type);
            const id = `codex-strip-${tool.type}`;
            return (
              <label
                key={tool.type}
                htmlFor={id}
                className="flex items-start gap-2 rounded-md border border-border/60 bg-background px-3 py-2 cursor-pointer hover:bg-accent/30 transition-colors"
              >
                <Checkbox
                  id={id}
                  checked={checked}
                  onCheckedChange={(state) => toggle(tool.type, !!state)}
                  className="mt-0.5"
                />
                <div className="flex-1 min-w-0">
                  <Label
                    htmlFor={id}
                    className="text-sm font-mono cursor-pointer"
                  >
                    {tool.label}
                  </Label>
                  {tool.hint && (
                    <p className="text-xs text-muted-foreground mt-0.5">
                      {tool.hint}
                    </p>
                  )}
                </div>
              </label>
            );
          })}
        </div>

        <div className="rounded-md bg-amber-500/10 border border-amber-500/30 px-3 py-2 text-xs text-amber-700 dark:text-amber-400">
          {t(
            "provider.codexToolStrip.note",
            "提示：勾选后仅在该 provider 走 cc-switch 代理转发时生效。本地路由关闭时（codex CLI 直连）不起作用。",
          )}
        </div>
      </div>
    </details>
  );
}
