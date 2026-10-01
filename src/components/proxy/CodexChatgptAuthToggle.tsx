/** Shortcut for the canonical Codex direct-switch login preservation setting.
 * Proxy routing keeps native login independently, matching upstream behavior.
 */

import { KeyRound, Loader2 } from "lucide-react";
import { toast } from "sonner";
import { useTranslation } from "react-i18next";
import { Switch } from "@/components/ui/switch";
import { useSaveSettingsMutation, useSettingsQuery } from "@/lib/query";
import { cn } from "@/lib/utils";

interface CodexChatgptAuthToggleProps {
  className?: string;
}

export function CodexChatgptAuthToggle({
  className,
}: CodexChatgptAuthToggleProps) {
  const { t } = useTranslation();
  const { data: settings, isLoading } = useSettingsQuery();
  const saveSettings = useSaveSettingsMutation();
  const enabled = settings?.preserveCodexOfficialAuthOnSwitch ?? false;
  const isBusy = isLoading || saveSettings.isPending;

  const handleToggle = (checked: boolean) => {
    if (!settings) return;
    saveSettings.mutate(
      { ...settings, preserveCodexOfficialAuthOnSwitch: checked },
      { onError: (error) => toast.error(String(error)) },
    );
  };

  const tooltipText = t("proxy.takeover.codexChatgptAuth.directSwitchHint", {
    defaultValue:
      "控制下次直连切换到第三方供应商时是否保留官方登录；代理模式始终保留已有原生登录。此开关不会自动登录。",
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
        aria-label={t("settings.preserveCodexOfficialAuthOnSwitch")}
        checked={enabled}
        onCheckedChange={handleToggle}
        disabled={isBusy || !settings}
      />
    </div>
  );
}
