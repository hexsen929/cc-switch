import type { QueryClient } from "@tanstack/react-query";

export const AGY_CURRENT_SCOPE_KEY = [
  "subscription",
  "agy-current-scope",
] as const;
export const AGY_CURRENT_QUOTA_PREFIX = [
  "subscription",
  "quota",
  "agy",
] as const;
export const agyCurrentQuotaKey = (scope: number) =>
  [...AGY_CURRENT_QUOTA_PREFIX, scope] as const;

/** Keep this helper independent of hooks/API modules so auth mutations can reuse it. */
export async function resetAgyCurrentQuota(
  queryClient: QueryClient,
): Promise<void> {
  await queryClient.cancelQueries({ queryKey: AGY_CURRENT_QUOTA_PREFIX });
  queryClient.setQueryData<number>(
    AGY_CURRENT_SCOPE_KEY,
    (scope = 0) => scope + 1,
  );
  queryClient.removeQueries({ queryKey: AGY_CURRENT_QUOTA_PREFIX });
}
