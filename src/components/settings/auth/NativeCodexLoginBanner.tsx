import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Download, Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { toast } from "@/lib/toast";
import { authApi } from "@/lib/api/auth";

/**
 * 授权中心 ChatGPT 卡片顶部的「检测到本机 ChatGPT 登录 → 一键导入」横幅（Fork 扩展）。
 * 对齐 magpie：静态读取 ~/.codex/auth.json 识别已有的 codex login 登录态；仅在检测到
 * 且尚未被 cc-switch 托管时显示。点「导入」把它采纳为受管账号（之后配额/切换/代理可用）。
 */
export function NativeCodexLoginBanner() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();

  const { data, isLoading } = useQuery({
    queryKey: ["native-codex-login"],
    queryFn: () => authApi.detectNativeCodexLogin(),
    staleTime: 10_000,
  });

  const adopt = useMutation({
    mutationFn: () => authApi.adoptNativeCodexLogin(),
    onSuccess: async () => {
      toast.success(
        t("authCenter.nativeCodex.imported", "已导入本机 ChatGPT 登录"),
      );
      await queryClient.invalidateQueries({
        queryKey: ["managed-auth-status", "codex_oauth"],
      });
      await queryClient.invalidateQueries({
        queryKey: ["native-codex-login"],
      });
    },
    onError: (error) => {
      toast.error(
        t("authCenter.nativeCodex.importFailed", "导入失败") +
          ":" +
          String(error),
      );
    },
  });

  if (isLoading || !data || data.alreadyManaged) return null;

  const who = data.email || data.chatgptAccountId || "ChatGPT";
  const label = data.plan ? `${who} · ${data.plan}` : who;

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
        disabled={adopt.isPending}
        onClick={() => adopt.mutate()}
        className="shrink-0"
      >
        {adopt.isPending ? (
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
