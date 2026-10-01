import { toast } from "sonner";
import { useTranslation } from "react-i18next";
import { providersApi } from "@/lib/api/providers";
import type { AppId } from "@/lib/api/types";
import { useTauriEvent } from "./useTauriEvent";

interface ResourceWarning {
  appType: AppId;
  error: string;
}

/** Secondary sync failures must not be displayed as failed provider saves. */
export function useProviderResourceWarnings() {
  const { t } = useTranslation();
  useTauriEvent<ResourceWarning>("provider-resources-warning", (payload) => {
    const id = `provider-resources-${payload.appType}`;
    const retry = async () => {
      try {
        await providersApi.retryResourceSync(payload.appType);
        toast.success(t("notifications.providerResourcesSynced"), { id });
      } catch (error) {
        showWarning(String(error));
      }
    };
    function showWarning(error: string) {
      toast.warning(t("notifications.providerResourcesWarning"), {
        id,
        description: `${payload.appType}: ${error}`,
        duration: 12000,
        action: { label: t("common.retry"), onClick: () => void retry() },
      });
    }
    showWarning(payload.error);
  });
}
