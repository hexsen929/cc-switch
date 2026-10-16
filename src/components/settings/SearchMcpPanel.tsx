import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import { ArrowDown, ArrowUp, Copy, Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { toast } from "@/lib/toast";

interface SearchSource {
  engine: string;
  enabled: boolean;
  baseUrl: string;
  keys: string[];
  orderedKeys: boolean;
  keyCount: number;
  clearKeys: boolean;
}

interface SearchMcpConfig {
  enabled: boolean;
  sources: SearchSource[];
  keyBackoff: boolean;
  timeoutSeconds: number;
  maxResultChars: number;
  dailyCallLimit: number;
}

interface SearchMcpView {
  config: SearchMcpConfig;
  endpoint: string;
  running: boolean;
}

interface SearchQuota {
  engine: string;
  keyNumber: number;
  known: boolean;
  used?: number;
  limit?: number | null;
}

const ENGINES: Record<string, string> = {
  tavily: "Tavily",
  brave: "Brave Search",
  exa: "Exa",
  firecrawl: "Firecrawl",
  you: "You.com",
  searxng: "SearXNG",
};
const CLIENTS = [
  "claude",
  "codex",
  "gemini",
  "grokbuild",
  "opencode",
  "hermes",
  "pi",
  "mcode",
];

export function SearchMcpPanel() {
  const { t } = useTranslation();
  const [view, setView] = useState<SearchMcpView | null>(null);
  const [savedEnabled, setSavedEnabled] = useState(false);
  const [busy, setBusy] = useState(false);
  const [clients, setClients] = useState(["claude", "codex", "gemini"]);
  const [error, setError] = useState("");
  const [keyDrafts, setKeyDrafts] = useState<Record<string, string>>({});
  const [quotas, setQuotas] = useState<SearchQuota[] | null>(null);

  const load = async () => {
    const next = await invoke<SearchMcpView>("get_search_mcp_config");
    setView(next);
    setSavedEnabled(next.config.enabled);
    setKeyDrafts({});
    setQuotas(null);
    setError("");
  };
  useEffect(() => {
    let active = true;
    invoke<SearchMcpView>("get_search_mcp_config")
      .then((next) => {
        if (active) {
          setView(next);
          setSavedEnabled(next.config.enabled);
        }
      })
      .catch((reason) => {
        if (active) setError(String(reason));
      });
    return () => {
      active = false;
    };
  }, []);

  const update = (patch: Partial<SearchMcpConfig>) => {
    setView((current) =>
      current
        ? { ...current, config: { ...current.config, ...patch } }
        : current,
    );
  };
  const updateSource = (index: number, patch: Partial<SearchSource>) => {
    if (!view) return;
    setQuotas(null);
    update({
      sources: view.config.sources.map((source, position) =>
        position === index ? { ...source, ...patch } : source,
      ),
    });
  };
  const move = (index: number, direction: number) => {
    if (!view) return;
    const sources = [...view.config.sources];
    [sources[index], sources[index + direction]] = [
      sources[index + direction],
      sources[index],
    ];
    update({ sources });
  };
  const save = async (install: boolean) => {
    if (!view) return;
    setBusy(true);
    try {
      await invoke("set_search_mcp_config", { config: view.config });
      setSavedEnabled(view.config.enabled);
      try {
        if (install) await invoke("add_search_mcp_to_apps", { apps: clients });
      } catch (reason) {
        toast.error(
          t("settings.searchMcp.installFailed", {
            defaultValue: "搜索设置已保存，但添加到客户端失败：{{error}}",
            error: String(reason),
          }),
        );
        return;
      } finally {
        await load();
      }
      toast.success(
        t("settings.searchMcp.saved", { defaultValue: "联网搜索设置已保存" }),
      );
    } catch (reason) {
      toast.error(String(reason));
    } finally {
      setBusy(false);
    }
  };

  if (!view)
    return (
      <p className="text-sm text-fg-2">
        {error || t("common.loading", { defaultValue: "加载中…" })}
      </p>
    );
  const { config } = view;

  return (
    <div className="space-y-5">
      <div className="flex items-start justify-between gap-4">
        <div className="space-y-1">
          <Label>
            {t("settings.searchMcp.enabled", {
              defaultValue: "启用统一联网搜索 MCP",
            })}
          </Label>
          <p className="text-xs text-fg-2">
            {t("settings.searchMcp.description", {
              defaultValue:
                "让各客户端共用 web_search 与 web_fetch，按下列顺序尝试搜索源。默认关闭，不改动客户端原有联网功能。",
            })}
          </p>
        </div>
        <Switch
          aria-label={t("settings.searchMcp.enabled", {
            defaultValue: "启用统一联网搜索 MCP",
          })}
          checked={config.enabled}
          disabled={busy}
          onCheckedChange={(enabled) => update({ enabled })}
        />
      </div>

      <div className="rounded-lg border border-border-default p-3 space-y-2 text-xs">
        <div className="flex flex-wrap items-start justify-between gap-2">
          <span className="text-fg-2">
            {t("settings.searchMcp.endpoint", {
              defaultValue: "统一 MCP 地址",
            })}
          </span>
          <div className="flex flex-col gap-1 text-end" role="status">
            <span>
              {!savedEnabled
                ? t("settings.searchMcp.serviceDisabled", {
                    defaultValue: "搜索 MCP 未启用",
                  })
                : view.running
                  ? t("settings.searchMcp.serviceEnabled", {
                      defaultValue: "搜索 MCP 已启用",
                    })
                  : t("settings.searchMcp.serviceUnavailable", {
                      defaultValue: "搜索 MCP 已启用，代理未运行",
                    })}
            </span>
            <span className="text-fg-2">
              {view.running
                ? t("settings.searchMcp.running", {
                    defaultValue: "共享本地代理运行中",
                  })
                : t("settings.searchMcp.stopped", {
                    defaultValue: "共享本地代理未运行",
                  })}
            </span>
          </div>
        </div>
        {config.enabled !== savedEnabled && (
          <p className="text-fg-2">
            {t("settings.searchMcp.pendingEnabled", {
              defaultValue: "启用状态尚未保存，点击保存后生效。",
            })}
          </p>
        )}
        <div className="flex items-center gap-2">
          <code className="break-all flex-1">{view.endpoint}</code>
          <Button
            variant="ghost"
            size="sm"
            aria-label={t("common.copy", { defaultValue: "复制" })}
            onClick={() => {
              void navigator.clipboard
                .writeText(view.endpoint)
                .catch((reason) => toast.error(String(reason)));
            }}
          >
            <Copy className="h-3.5 w-3.5" />
          </Button>
        </div>
        <p className="text-fg-2">
          {t("settings.searchMcp.gatewayHint", {
            defaultValue:
              "搜索 MCP 与模型路由共用本地代理。代理运行不代表搜索已启用；关闭搜索不会停止模型路由。保存启用后会启动代理。局域网访问须使用允许 web-search 的网关密钥；网页抓取不访问内网。",
          })}
        </p>
      </div>

      {config.sources.map((source, index) => (
        <div
          key={source.engine}
          className="rounded-lg border border-border-default p-3 space-y-3"
        >
          <div className="flex items-center gap-2">
            <Switch
              aria-label={ENGINES[source.engine]}
              checked={source.enabled}
              disabled={busy}
              onCheckedChange={(enabled) => updateSource(index, { enabled })}
            />
            <span className="text-sm font-medium flex-1">
              {index + 1}. {ENGINES[source.engine] || source.engine}
            </span>
            <Button
              variant="ghost"
              size="sm"
              disabled={busy || index === 0}
              aria-label={t("settings.searchMcp.moveUp", {
                defaultValue: "提高优先级",
              })}
              onClick={() => move(index, -1)}
            >
              <ArrowUp className="h-3.5 w-3.5" />
            </Button>
            <Button
              variant="ghost"
              size="sm"
              disabled={busy || index === config.sources.length - 1}
              aria-label={t("settings.searchMcp.moveDown", {
                defaultValue: "降低优先级",
              })}
              onClick={() => move(index, 1)}
            >
              <ArrowDown className="h-3.5 w-3.5" />
            </Button>
          </div>
          <Input
            disabled={busy}
            value={source.baseUrl}
            placeholder={
              source.engine === "searxng"
                ? t("settings.searchMcp.instanceUrl", {
                    defaultValue: "SearXNG 实例地址（必填）",
                  })
                : t("settings.searchMcp.baseUrl", {
                    defaultValue: "自定义 API 地址（留空使用官方地址）",
                  })
            }
            aria-label={`${ENGINES[source.engine]} ${t("settings.searchMcp.baseUrlLabel", { defaultValue: "搜索源地址" })}`}
            onChange={(event) =>
              updateSource(index, { baseUrl: event.target.value })
            }
          />
          <Input
            type="password"
            autoComplete="new-password"
            disabled={busy || source.clearKeys}
            value={keyDrafts[source.engine] || ""}
            aria-label={`${ENGINES[source.engine]} ${t("settings.searchMcp.keys", { defaultValue: "API 密钥" })}`}
            placeholder={t("settings.searchMcp.keysPlaceholder", {
              defaultValue: "已保存 {{count}} 个密钥；留空保持，多个用逗号分隔",
              count: source.keyCount,
            })}
            onChange={(event) => {
              const raw = event.target.value;
              setKeyDrafts((current) => ({ ...current, [source.engine]: raw }));
              updateSource(index, {
                keys: raw.split(/[\s,]+/).filter(Boolean),
              });
            }}
          />
          <div className="flex flex-wrap gap-x-5 gap-y-2 text-xs text-fg-2">
            <label className="flex items-center gap-2">
              <input
                type="checkbox"
                checked={source.orderedKeys}
                disabled={busy}
                onChange={(event) =>
                  updateSource(index, { orderedKeys: event.target.checked })
                }
              />
              {t("settings.searchMcp.orderedKeys", {
                defaultValue: "按顺序使用密钥（关闭则轮询）",
              })}
            </label>
            <label className="flex items-center gap-2">
              <input
                type="checkbox"
                checked={source.clearKeys}
                disabled={busy}
                onChange={(event) => {
                  setKeyDrafts((current) => ({
                    ...current,
                    [source.engine]: "",
                  }));
                  updateSource(index, {
                    clearKeys: event.target.checked,
                    keys: [],
                  });
                }}
              />
              {t("settings.searchMcp.clearKeys", {
                defaultValue: "清除已保存密钥",
              })}
            </label>
          </div>
        </div>
      ))}

      <div className="flex items-center justify-between gap-4">
        <Label>
          {t("settings.searchMcp.backoff", {
            defaultValue: "被拒绝或限流的密钥暂时后置",
          })}
        </Label>
        <Switch
          aria-label={t("settings.searchMcp.backoff", {
            defaultValue: "被拒绝或限流的密钥暂时后置",
          })}
          checked={config.keyBackoff}
          disabled={busy}
          onCheckedChange={(keyBackoff) => update({ keyBackoff })}
        />
      </div>
      <div className="grid grid-cols-1 sm:grid-cols-3 gap-3">
        <label className="space-y-1 text-xs text-fg-2">
          {t("settings.searchMcp.timeout", {
            defaultValue: "总超时（5–120 秒）",
          })}
          <Input
            type="number"
            min={5}
            max={120}
            value={config.timeoutSeconds}
            disabled={busy}
            onChange={(event) =>
              update({ timeoutSeconds: Number(event.target.value) })
            }
          />
        </label>
        <label className="space-y-1 text-xs text-fg-2">
          {t("settings.searchMcp.maxChars", { defaultValue: "最多返回字符数" })}
          <Input
            type="number"
            min={1000}
            max={100000}
            value={config.maxResultChars}
            disabled={busy}
            onChange={(event) =>
              update({ maxResultChars: Number(event.target.value) })
            }
          />
        </label>
        <label className="space-y-1 text-xs text-fg-2">
          {t("settings.searchMcp.dailyLimit", {
            defaultValue: "每个网关密钥每日调用上限（0 不限）",
          })}
          <Input
            type="number"
            min={0}
            max={1000000}
            value={config.dailyCallLimit}
            disabled={busy}
            onChange={(event) =>
              update({ dailyCallLimit: Number(event.target.value) })
            }
          />
        </label>
      </div>
      <div className="space-y-2">
        <Label>
          {t("settings.searchMcp.clients", {
            defaultValue: "添加到这些客户端的 MCP 列表",
          })}
        </Label>
        <div className="flex flex-wrap gap-x-4 gap-y-2 text-xs">
          {CLIENTS.map((client) => (
            <label key={client} className="flex items-center gap-1.5">
              <input
                type="checkbox"
                disabled={busy}
                checked={clients.includes(client)}
                onChange={(event) =>
                  setClients((current) =>
                    event.target.checked
                      ? [...current, client]
                      : current.filter((value) => value !== client),
                  )
                }
              />
              {client === "mcode" ? "MCode" : client === "pi" ? "Pi" : client}
            </label>
          ))}
        </div>
      </div>
      <div className="flex flex-wrap gap-2">
        <Button
          disabled={busy}
          onClick={() => {
            void save(false);
          }}
        >
          {busy && <Loader2 className="h-4 w-4 mr-2 animate-spin" />}
          {t("common.save", { defaultValue: "保存" })}
        </Button>
        <Button
          variant="outline"
          disabled={busy || !config.enabled || clients.length === 0}
          onClick={() => {
            void save(true);
          }}
        >
          {t("settings.searchMcp.install", {
            defaultValue: "保存并添加到 MCP",
          })}
        </Button>
        <Button
          variant="outline"
          disabled={busy}
          onClick={() => {
            setBusy(true);
            invoke<SearchQuota[]>("get_search_mcp_quotas", { fresh: true })
              .then(setQuotas)
              .catch((reason) => toast.error(String(reason)))
              .finally(() => setBusy(false));
          }}
        >
          {t("settings.searchMcp.readQuota", {
            defaultValue: "查询已保存密钥的额度",
          })}
        </Button>
      </div>
      {quotas && (
        <div className="rounded-lg bg-subtle p-3 text-xs space-y-1">
          <p className="text-fg-2">
            {t("settings.searchMcp.quotaHint", {
              defaultValue:
                "目前支持读取 Tavily 额度；其它搜索源或查询失败显示未知。不会把未知额度当作 0。",
            })}
          </p>
          {quotas.map((quota) => (
            <p key={`${quota.engine}-${quota.keyNumber}`}>
              {ENGINES[quota.engine]} ·{" "}
              {t("settings.searchMcp.keyNumber", {
                defaultValue: "密钥 {{number}}",
                number: quota.keyNumber,
              })}
              :{" "}
              {quota.known
                ? `${quota.used} / ${quota.limit ?? t("settings.searchMcp.unlimited", { defaultValue: "不限" })}`
                : t("settings.searchMcp.unknown", { defaultValue: "未知" })}
            </p>
          ))}
        </div>
      )}
    </div>
  );
}
