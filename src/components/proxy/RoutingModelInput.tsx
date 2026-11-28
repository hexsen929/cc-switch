import { useLayoutEffect, useMemo, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Download, Loader2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ModelDropdown } from "@/components/providers/forms/shared/ModelDropdown";
import { showFetchModelsError, type FetchedModel } from "@/lib/api/model-fetch";
import { queryClient } from "@/lib/query/queryClient";
import {
  configuredRouteModels,
  fetchRoutingModels,
  CatalogConfigError,
} from "@/lib/routingModelCatalog";
import { toast } from "@/lib/toast";
import type { AppId } from "@/lib/api";
import type { Provider } from "@/types";

interface Props {
  app: AppId;
  provider?: Provider;
  value: string;
  onChange: (id: string) => void;
  label: string;
  disabled: boolean;
  maxLength?: number;
}

// Share catalogs across rows of the same saved card without putting endpoint
// credentials into query keys. A replaced card/account gets a fresh key.
const cardVersions = new WeakMap<Provider, number>();
let nextVersion = 1;
function versionOf(provider?: Provider): number {
  if (!provider) return 0;
  const known = cardVersions.get(provider);
  if (known !== undefined) return known;
  const version = nextVersion++;
  cardVersions.set(provider, version);
  return version;
}

export function RoutingModelInput({
  app,
  provider,
  value,
  onChange,
  label,
  disabled,
  maxLength,
}: Props) {
  const { t } = useTranslation();
  const {
    data: fetched,
    isFetching: loading,
    refetch,
  } = useQuery(
    {
      queryKey: ["routingModelCatalog", app, provider?.id, versionOf(provider)],
      enabled: false,
      retry: false,
      queryFn: () =>
        provider ? fetchRoutingModels(app, provider) : Promise.resolve([]),
    },
    queryClient,
  );
  const [open, setOpen] = useState(false);
  const sequence = useRef(0);
  useLayoutEffect(() => {
    setOpen(false);
    return () => {
      sequence.current++;
    };
  }, [app, provider]);
  const models = useMemo(() => {
    const byId = new Map<string, FetchedModel>();
    for (const m of [
      ...(provider ? configuredRouteModels(app, provider) : []),
      ...(fetched ?? []),
    ]) {
      const id = m.id.trim();
      if (id) byId.set(id, { ...m, id });
    }
    return [...byId.values()];
  }, [app, provider, fetched]);

  async function fetchList() {
    if (!provider || disabled || loading) return;
    const request = ++sequence.current;
    try {
      const { data: rows } = await refetch({
        cancelRefetch: false,
        throwOnError: true,
      });
      if (request !== sequence.current) return;
      if (rows?.some((m) => m.id.trim())) setOpen(true);
      else toast.info(t("providerForm.fetchModelsEmpty"));
    } catch (error) {
      if (request !== sequence.current) return;
      if (error instanceof CatalogConfigError) toast.error(t(error.messageKey));
      else showFetchModelsError(error, t);
    }
  }
  return (
    <div className="flex min-w-0 flex-1 flex-wrap items-center gap-1">
      <Input
        className="min-w-24 flex-1 font-mono text-xs"
        aria-label={label}
        placeholder={label}
        maxLength={maxLength}
        autoComplete="off"
        value={value}
        disabled={disabled}
        onChange={(e) => onChange(e.target.value)}
      />
      {models.length > 0 && (
        <ModelDropdown
          models={models}
          onSelect={onChange}
          open={open && !disabled}
          onOpenChange={setOpen}
          disabled={disabled}
        />
      )}
      <Button
        type="button"
        variant="outline"
        size="sm"
        disabled={disabled || !provider || loading}
        onClick={() => void fetchList()}
        aria-busy={loading}
        className="shrink-0 gap-1"
      >
        {loading ? (
          <Loader2 className="size-3.5 animate-spin" />
        ) : (
          <Download className="size-3.5" />
        )}
        {t("providerForm.fetchModels")}
      </Button>
    </div>
  );
}
