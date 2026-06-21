/**
 * Codex ChatGPT auth takeover mode toggle.
 *
 * This is a Codex auth writing mode flag. It can be changed while routing is
 * off or on, and live Codex config is rewritten immediately with the selected
 * auth mode.
 */

import { KeyRound, Loader2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Switch } from "@/components/ui/switch";
import { useSettingsQuery } from "@/lib/query";
import { useAppProxyConfig, useUpdateAppProxyConfig } from "@/lib/query/proxy";
import { cn } from "@/lib/utils";

interface CodexChatgptAuthToggleProps {
  className?: string;
}

export function CodexChatgptAuthToggle({
  className,
}: CodexChatgptAuthToggleProps) {
  const { t } = useTranslation();
  const { data: codexProxyConfig, isLoading } = useAppProxyConfig("codex");
  const { data: settings, isLoading: isSettingsLoading } = useSettingsQuery();
  const updateAppProxyConfig = useUpdateAppProxyConfig();

  const officialAuthPreservationEnabled =
    settings?.preserveCodexOfficialAuthOnSwitch ?? false;
  const enabled = codexProxyConfig?.codexChatgptAuthTakeover ?? false;
  const isBusy =
    isLoading || isSettingsLoading || updateAppProxyConfig.isPending;

  const handleToggle = async (checked: boolean) => {
    if (!codexProxyConfig) return;

    await updateAppProxyConfig.mutateAsync({
      ...codexProxyConfig,
      codexChatgptAuthTakeover: checked,
    });
  };

  const tooltipText = enabled
    ? officialAuthPreservationEnabled
      ? t("proxy.takeover.codexChatgptAuth.enabledTooltip", {
          defaultValue:
            "Codex 将保留 ChatGPT 登录态；路由开启或关闭都会写入 chatgpt 模式",
        })
      : t("proxy.takeover.codexChatgptAuth.enablingTooltip", {
          defaultValue: "Codex 将开启 ChatGPT 登录态保留模式",
        })
    : t("proxy.takeover.codexChatgptAuth.disabledTooltip", {
        defaultValue:
          "Codex 使用默认 API 认证写入逻辑；不会保留 ChatGPT 登录态",
      });

  return (
    <div
      className={cn(
        "flex items-center gap-1 px-1.5 h-8 rounded-lg bg-muted/50 transition-all",
        className,
      )}
      title={tooltipText}
    >
      {isBusy ? (
        <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
      ) : (
        <KeyRound
          className={cn(
            "h-4 w-4 transition-colors",
            enabled ? "text-emerald-500" : "text-muted-foreground",
          )}
        />
      )}
      <span className="hidden lg:inline text-xs font-medium text-muted-foreground whitespace-nowrap">
        ChatGPT
      </span>
      <Switch
        checked={enabled}
        onCheckedChange={handleToggle}
        disabled={isBusy || !codexProxyConfig}
      />
    </div>
  );
}
