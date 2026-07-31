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
} from "@/lib/api/shellWrapper";

interface ShellWrapperCardProps {
  appType: string;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function ShellWrapperCard({ appType }: ShellWrapperCardProps) {
  const { t } = useTranslation();
  const [status, setStatus] = useState<WrapperStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [installing, setInstalling] = useState(false);
  const [instructions, setInstructions] = useState("");
  const [showInstructions, setShowInstructions] = useState(false);
  const [copied, setCopied] = useState(false);

  const checkStatus = useCallback(async () => {
    if (appType !== "claude") return;

    try {
      setLoading(true);
      setStatus(await checkShellWrapperStatus());
    } catch (error) {
      toast.error(
        t("prompts.shellWrapperStatusFailed", {
          error: errorMessage(error),
        }),
      );
    } finally {
      setLoading(false);
    }
  }, [appType, t]);

  useEffect(() => {
    void checkStatus();
  }, [checkStatus]);

  if (appType !== "claude") return null;

  const handleInstall = async () => {
    const upgrading = status?.needsUpgrade ?? false;
    try {
      setInstalling(true);
      const configPath = await installShellWrapper();
      toast.success(
        t(
          upgrading
            ? "prompts.shellWrapperUpgradeSuccess"
            : "prompts.shellWrapperInstallSuccess",
          { path: configPath },
        ),
        { closeButton: true },
      );
      await checkStatus();
    } catch (error) {
      toast.error(
        t("prompts.shellWrapperInstallFailed", {
          error: errorMessage(error),
        }),
      );
    } finally {
      setInstalling(false);
    }
  };

  const handleUninstall = async () => {
    if (!window.confirm(t("prompts.shellWrapperUninstallConfirm"))) return;

    try {
      setInstalling(true);
      const configPath = await uninstallShellWrapper();
      toast.success(
        t("prompts.shellWrapperUninstallSuccess", { path: configPath }),
        { closeButton: true },
      );
      await checkStatus();
    } catch (error) {
      toast.error(
        t("prompts.shellWrapperUninstallFailed", {
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
          t("prompts.shellWrapperInstructionsFailed", {
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
        t("prompts.shellWrapperCopyFailed", {
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
            {t("prompts.shellWrapperChecking")}
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
            {t("prompts.shellWrapperTitle")}
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
              {t("prompts.shellWrapperConflict")}
            </span>
          </div>
        ) : status?.needsUpgrade ? (
          <div className="flex shrink-0 items-center gap-1.5 text-amber-600 dark:text-amber-400">
            <AlertCircle className="h-4 w-4" />
            <span className="text-xs font-medium">
              {t("prompts.shellWrapperNeedsUpgrade")}
            </span>
          </div>
        ) : status?.installed ? (
          <div className="flex shrink-0 items-center gap-1.5 text-green-600 dark:text-green-400">
            <CheckCircle2 className="h-4 w-4" />
            <span className="text-xs font-medium">
              {t("prompts.shellWrapperInstalled")}
            </span>
          </div>
        ) : (
          <div className="flex shrink-0 items-center gap-1.5 text-amber-600 dark:text-amber-400">
            <AlertCircle className="h-4 w-4" />
            <span className="text-xs font-medium">
              {t("prompts.shellWrapperNotInstalled")}
            </span>
          </div>
        )}
      </div>

      <p className="text-xs leading-relaxed text-muted-foreground">
        {status?.conflictingWrapper
          ? t("prompts.shellWrapperConflictHint")
          : status?.needsUpgrade
            ? t("prompts.shellWrapperNeedsUpgradeHint")
            : status?.installed
              ? t("prompts.shellWrapperInstalledHint")
              : t("prompts.shellWrapperNotInstalledHint")}
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
                  ? t("prompts.shellWrapperUpdating")
                  : t("prompts.shellWrapperInstalling")
                : status?.needsUpgrade
                  ? t("prompts.shellWrapperUpgrade")
                  : t("prompts.shellWrapperInstall")}
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
              ? t("prompts.shellWrapperProcessing")
              : t("prompts.shellWrapperUninstall")}
          </Button>
        )}

        <Button
          type="button"
          variant="ghost"
          size="sm"
          onClick={handleShowInstructions}
        >
          {showInstructions
            ? t("prompts.shellWrapperHideInstructions")
            : t("prompts.shellWrapperShowInstructions")}
        </Button>
      </div>

      {showInstructions && instructions && (
        <div className="space-y-2 border-t border-border-default pt-3">
          <div className="flex items-center justify-between gap-3">
            <span className="text-xs font-medium text-foreground">
              {t("prompts.shellWrapperInstructions")}
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
              {copied ? t("prompts.shellWrapperCopied") : t("common.copy")}
            </Button>
          </div>
          <pre className="max-h-64 overflow-auto rounded bg-gray-950 p-3 text-xs text-gray-100">
            {instructions}
          </pre>
        </div>
      )}

      {status?.configFile && (
        <div className="break-all text-xs text-muted-foreground">
          {t("prompts.shellWrapperConfigFile")}: {status.configFile}
        </div>
      )}
    </div>
  );
}
