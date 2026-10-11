import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Download, Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { toast } from "@/lib/toast";
import { authApi, type NativeCodexLoginInfo } from "@/lib/api/auth";

/**
 * 授权中心 ChatGPT 卡片顶部的「检测到本机 ChatGPT 登录 → 一键导入」横幅（Fork 扩展）。
 * 对齐 magpie：静态读取 ~/.codex/auth.json 识别已有的 codex login 登录态；仅在检测到
 * 且尚未被 cc-switch 托管时显示。点「导入」把它采纳为受管账号（之后配额/切换/代理可用）。
 *
 * 刻意用普通 useState/useEffect（而非 react-query）：横幅可能被渲染在没有
 * QueryClientProvider 的上下文/测试里，自带数据获取更稳；导入后通过 onImported
 * 复用卡片既有的 refetchStatus 刷新账号列表。
 */
export function NativeCodexLoginBanner({
  onImported,
}: {
  /** 导入成功后回调：由父级刷新受管账号列表（通常是 refetchStatus）。 */
  onImported?: () => void;
}) {
  const { t } = useTranslation();
  const [info, setInfo] = useState<NativeCodexLoginInfo | null>(null);
  const [importing, setImporting] = useState(false);
  const [done, setDone] = useState(false);

  useEffect(() => {
    let active = true;
    authApi
      .detectNativeCodexLogin()
      .then((detected) => {
        if (active) setInfo(detected);
      })
      .catch(() => {
        /* 读取失败：当作未检测到，不显示横幅 */
      });
    return () => {
      active = false;
    };
  }, []);

  if (done || !info || info.alreadyManaged) return null;

  const who = info.email || info.chatgptAccountId || "ChatGPT";
  const label = info.plan ? `${who} · ${info.plan}` : who;

  const onImport = async () => {
    setImporting(true);
    try {
      await authApi.adoptNativeCodexLogin();
      toast.success(
        t("authCenter.nativeCodex.imported", "已导入本机 ChatGPT 登录"),
      );
      setDone(true);
      onImported?.();
    } catch (error) {
      toast.error(
        t("authCenter.nativeCodex.importFailed", "导入失败") +
          ":" +
          String(error),
      );
    } finally {
      setImporting(false);
    }
  };

  return (
    <div className="flex items-center gap-3 rounded-md border border-border bg-surface p-3">
      <div className="min-w-0 flex-1">
        <p className="text-sm font-medium text-fg-1">
          {t("authCenter.nativeCodex.title", "检测到本机 ChatGPT 登录")}
        </p>
        <p className="mt-0.5 truncate text-xs text-fg-2">
          {t("authCenter.nativeCodex.hint", {
            defaultValue:
              "你已通过 Codex 登录 ChatGPT（{{who}}）。导入后即可在 cc-switch 直接使用，无需再登录一次。",
            who: label,
          })}
        </p>
      </div>
      <Button
        variant="solid"
        size="sm"
        disabled={importing}
        onClick={() => void onImport()}
        className="shrink-0"
      >
        {importing ? (
          <Loader2 className="mr-1 h-3.5 w-3.5 animate-spin" />
        ) : (
          <Download className="mr-1 h-3.5 w-3.5" />
        )}
        {t("authCenter.nativeCodex.import", "导入")}
      </Button>
    </div>
  );
}

export default NativeCodexLoginBanner;
