import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { AlertCircle, Check, CheckCircle2, Copy, Terminal } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  checkShellWrapperStatus,
  getShellWrapperInstructions,
  installShellWrapper,
  type WrapperStatus,
  uninstallShellWrapper,
} from "@/lib/api/claudeShellWrapper";

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function ClaudeShellWrapperCard() {
  const { t } = useTranslation();
  const [status, setStatus] = useState<WrapperStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [installing, setInstalling] = useState(false);
  const [instructions, setInstructions] = useState("");
  const [showInstructions, setShowInstructions] = useState(false);
  const [copied, setCopied] = useState(false);

  const checkStatus = useCallback(async () => {
    try {
      setLoading(true);
      setStatus(await checkShellWrapperStatus());
    } catch (error) {
      toast.error(
        t("claudeAppendInstructions.shellWrapperStatusFailed", {
          defaultValue: "无法检查 Shell 集成：{{error}}",
          error: errorMessage(error),
        }),
      );
    } finally {
      setLoading(false);
    }
  }, [t]);

  useEffect(() => {
    void checkStatus();
  }, [checkStatus]);

  const handleInstall = async () => {
    const upgrading = status?.needsUpgrade ?? false;
    try {
      setInstalling(true);
      const configPath = await installShellWrapper();
      toast.success(
        t(
          upgrading
            ? "claudeAppendInstructions.shellWrapperUpgradeSuccess"
            : "claudeAppendInstructions.shellWrapperInstallSuccess",
          {
            defaultValue: upgrading
              ? "Shell 集成已在 {{path}} 完成升级，重启终端后生效。"
              : "Shell 集成已安装到 {{path}}，重启终端后生效。",
            path: configPath,
          },
        ),
        { closeButton: true },
      );
      await checkStatus();
    } catch (error) {
      toast.error(
        t("claudeAppendInstructions.shellWrapperInstallFailed", {
          defaultValue: "无法安装 Shell 集成：{{error}}",
          error: errorMessage(error),
        }),
      );
    } finally {
      setInstalling(false);
    }
  };

  const handleUninstall = async () => {
    if (
      !window.confirm(
        t("claudeAppendInstructions.shellWrapperUninstallConfirm", {
          defaultValue: "确定卸载 Claude Shell 集成吗？",
        }),
      )
    )
      return;

    try {
      setInstalling(true);
      const configPath = await uninstallShellWrapper();
      toast.success(
        t("claudeAppendInstructions.shellWrapperUninstallSuccess", {
          defaultValue: "Shell 集成已从 {{path}} 移除，重启终端后生效。",
          path: configPath,
        }),
        { closeButton: true },
      );
      await checkStatus();
    } catch (error) {
      toast.error(
        t("claudeAppendInstructions.shellWrapperUninstallFailed", {
          defaultValue: "无法卸载 Shell 集成：{{error}}",
          error: errorMessage(error),
        }),
      );
    } finally {
      setInstalling(false);
    }
  };

  const handleShowInstructions = async () => {
    if (!instructions) {
      try {
        setInstructions(await getShellWrapperInstructions());
      } catch (error) {
        toast.error(
          t("claudeAppendInstructions.shellWrapperInstructionsFailed", {
            defaultValue: "无法加载手动配置：{{error}}",
            error: errorMessage(error),
          }),
        );
        return;
      }
    }
    setShowInstructions((visible) => !visible);
  };

  const handleCopyInstructions = async () => {
    try {
      await navigator.clipboard.writeText(instructions);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 2000);
    } catch (error) {
      toast.error(
        t("claudeAppendInstructions.shellWrapperCopyFailed", {
          defaultValue: "无法复制配置：{{error}}",
          error: errorMessage(error),
        }),
      );
    }
  };

  if (loading) {
    return (
      <div className="rounded-lg border border-border-default bg-muted/50 p-4">
        <div className="flex items-center gap-2">
          <Terminal className="h-4 w-4 animate-pulse text-muted-foreground" />
          <span className="text-sm text-muted-foreground">
            {t("claudeAppendInstructions.shellWrapperChecking", {
              defaultValue: "正在检查 Shell 集成...",
            })}
          </span>
        </div>
      </div>
    );
  }

  return (
    <div className="space-y-3 rounded-lg border border-border-default bg-muted/50 p-4">
      <div className="flex items-center justify-between gap-4">
        <div className="flex min-w-0 items-center gap-2">
          <Terminal className="h-4 w-4 shrink-0 text-muted-foreground" />
          <span className="truncate text-sm font-medium text-foreground">
            {t("claudeAppendInstructions.shellWrapperTitle", {
              defaultValue: "Claude Shell 集成",
            })}
          </span>
          {status?.shellType && (
            <span className="shrink-0 text-xs text-muted-foreground">
              ({status.shellType})
            </span>
          )}
        </div>

        {status?.conflictingWrapper ? (
          <div className="flex shrink-0 items-center gap-1.5 text-amber-600 dark:text-amber-400">
            <AlertCircle className="h-4 w-4" />
            <span className="text-xs font-medium">
              {t("claudeAppendInstructions.shellWrapperConflict", {
                defaultValue: "检测到外部 Wrapper",
              })}
            </span>
          </div>
        ) : status?.needsUpgrade ? (
          <div className="flex shrink-0 items-center gap-1.5 text-amber-600 dark:text-amber-400">
            <AlertCircle className="h-4 w-4" />
            <span className="text-xs font-medium">
              {t("claudeAppendInstructions.shellWrapperNeedsUpgrade", {
                defaultValue: "需要升级",
              })}
            </span>
          </div>
        ) : status?.installed ? (
          <div className="flex shrink-0 items-center gap-1.5 text-green-600 dark:text-green-400">
            <CheckCircle2 className="h-4 w-4" />
            <span className="text-xs font-medium">
              {t("claudeAppendInstructions.shellWrapperInstalled", {
                defaultValue: "已安装",
              })}
            </span>
          </div>
        ) : (
          <div className="flex shrink-0 items-center gap-1.5 text-amber-600 dark:text-amber-400">
            <AlertCircle className="h-4 w-4" />
            <span className="text-xs font-medium">
              {t("claudeAppendInstructions.shellWrapperNotInstalled", {
                defaultValue: "未安装",
              })}
            </span>
          </div>
        )}
      </div>

      <p className="text-xs leading-relaxed text-muted-foreground">
        {status?.conflictingWrapper
          ? t("claudeAppendInstructions.shellWrapperConflictHint", {
              defaultValue:
                "claude-keysmith Wrapper 已在管理 Claude Code，CC Switch 不会覆盖它的运行时指令配置。",
            })
          : status?.needsUpgrade
            ? t("claudeAppendInstructions.shellWrapperNeedsUpgradeHint", {
                defaultValue:
                  "升级旧版 CC Switch Wrapper，避免重复或过时的启动行为。",
              })
            : status?.installed
              ? t("claudeAppendInstructions.shellWrapperInstalledHint", {
                  defaultValue:
                    "从终端启动 Claude Code 时会加载当前供应商已启用的系统指令和追加指令文件。",
                })
              : t("claudeAppendInstructions.shellWrapperNotInstalledHint", {
                  defaultValue:
                    "安装 Shell 集成后，Claude Code 启动时会应用当前供应商已启用的系统指令和追加指令。",
                })}
      </p>

      <div className="flex flex-wrap items-center gap-2">
        {!status?.conflictingWrapper &&
          (!status?.installed || status.needsUpgrade) && (
            <Button
              type="button"
              size="sm"
              onClick={handleInstall}
              disabled={installing}
            >
              {installing
                ? status?.needsUpgrade
                  ? t("claudeAppendInstructions.shellWrapperUpdating", {
                      defaultValue: "升级中...",
                    })
                  : t("claudeAppendInstructions.shellWrapperInstalling", {
                      defaultValue: "安装中...",
                    })
                : status?.needsUpgrade
                  ? t("claudeAppendInstructions.shellWrapperUpgrade", {
                      defaultValue: "升级",
                    })
                  : t("claudeAppendInstructions.shellWrapperInstall", {
                      defaultValue: "安装",
                    })}
            </Button>
          )}

        {status?.installed && (
          <Button
            type="button"
            variant="ghost"
            size="sm"
            onClick={handleUninstall}
            disabled={installing}
            className="text-red-600 hover:text-red-600 dark:text-red-400"
          >
            {installing
              ? t("claudeAppendInstructions.shellWrapperProcessing", {
                  defaultValue: "处理中...",
                })
              : t("claudeAppendInstructions.shellWrapperUninstall", {
                  defaultValue: "卸载",
                })}
          </Button>
        )}

        <Button
          type="button"
          variant="ghost"
          size="sm"
          onClick={handleShowInstructions}
        >
          {showInstructions
            ? t("claudeAppendInstructions.shellWrapperHideInstructions", {
                defaultValue: "隐藏配置",
              })
            : t("claudeAppendInstructions.shellWrapperShowInstructions", {
                defaultValue: "查看配置",
              })}
        </Button>
      </div>

      {showInstructions && instructions && (
        <div className="space-y-2 border-t border-border-default pt-3">
          <div className="flex items-center justify-between gap-3">
            <span className="text-xs font-medium text-foreground">
              {t("claudeAppendInstructions.shellWrapperInstructions", {
                defaultValue: "手动配置",
              })}
            </span>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={handleCopyInstructions}
            >
              {copied ? (
                <Check className="mr-1 h-3 w-3" />
              ) : (
                <Copy className="mr-1 h-3 w-3" />
              )}
              {copied
                ? t("claudeAppendInstructions.shellWrapperCopied", {
                    defaultValue: "已复制",
                  })
                : t("common.copy")}
            </Button>
          </div>
          <pre className="max-h-64 overflow-auto rounded bg-gray-950 p-3 text-xs text-gray-100">
            {instructions}
          </pre>
        </div>
      )}

      {status?.configFile && (
        <div className="break-all text-xs text-muted-foreground">
          {t("claudeAppendInstructions.shellWrapperConfigFile", {
            defaultValue: "配置文件",
          })}
          : {status.configFile}
        </div>
      )}
    </div>
  );
}
