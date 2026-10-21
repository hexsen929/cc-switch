import type { ReactNode } from "react";
import { act, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  useSubscriptionQuota,
  subscriptionKeys,
} from "@/lib/query/subscription";
import { resetAgyCurrentQuota } from "@/lib/query/agyCurrentQuota";
import { useUsageCacheBridge } from "@/hooks/useUsageCacheBridge";
import type { SubscriptionQuota } from "@/types/subscription";
import { emitTauriEvent } from "../msw/tauriMocks";

const api = vi.hoisted(() => ({ getQuota: vi.fn() }));
vi.mock("@/lib/api/subscription", () => ({ subscriptionApi: api }));

const quota = (utilization: number, queriedAt: number): SubscriptionQuota => ({
  tool: "agy",
  credentialStatus: "valid",
  credentialMessage: null,
  success: true,
  tiers: [{ name: "agy_gemini_five_hour", utilization, resetsAt: null }],
  extraUsage: null,
  error: null,
  queriedAt,
});
const first = quota(10, Date.parse("2026-10-22T00:00:00Z"));
const second = quota(80, Date.parse("2026-10-22T00:01:00Z"));
const createClient = () =>
  new QueryClient({
    defaultOptions: { queries: { retry: false, retryDelay: 0 } },
  });
const wrapper =
  (client: QueryClient) =>
  ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );

beforeEach(() => {
  api.getQuota.mockReset();
});

describe("AGY current-login quota generations", () => {
  it("clears mounted keep-last-good data when the next account query fails", async () => {
    const client = createClient();
    api.getQuota.mockResolvedValue(first);
    const { result, unmount } = renderHook(
      () => useSubscriptionQuota("agy", true),
      { wrapper: wrapper(client) },
    );
    await waitFor(() => expect(result.current.data).toEqual(first));
    api.getQuota.mockRejectedValue(new Error("network timeout"));
    await act(async () => {
      await resetAgyCurrentQuota(client);
    });
    await waitFor(() => expect(result.current.isError).toBe(true));
    expect(result.current.data?.success).toBe(false);
    expect(result.current.data?.tiers).toEqual([]);
    expect(
      client.getQueryData(subscriptionKeys.agyCurrentQuota(0)),
    ).toBeUndefined();
    expect(client.getQueryData(subscriptionKeys.agyCurrentScope)).toBe(1);
    unmount();
    api.getQuota.mockResolvedValue(second);
    const remount = renderHook(() => useSubscriptionQuota("agy", true), {
      wrapper: wrapper(client),
    });
    await waitFor(() => expect(remount.result.current.data).toEqual(second));
    expect(client.getQueryData(subscriptionKeys.agyCurrentQuota(1))).toEqual(
      second,
    );
  });

  it("does not resurrect a cancelled old-account response", async () => {
    let resolveOld!: (value: SubscriptionQuota) => void;
    const oldRequest = new Promise<SubscriptionQuota>((resolve) => {
      resolveOld = resolve;
    });
    const client = createClient();
    api.getQuota
      .mockResolvedValueOnce(first)
      .mockReturnValueOnce(oldRequest)
      .mockResolvedValue(second);
    const { result } = renderHook(() => useSubscriptionQuota("agy", true), {
      wrapper: wrapper(client),
    });
    await waitFor(() => expect(result.current.data).toEqual(first));
    act(() => {
      void result.current.refetch();
    });
    await waitFor(() => expect(api.getQuota).toHaveBeenCalledTimes(2));
    await act(async () => {
      await resetAgyCurrentQuota(client);
    });
    await waitFor(() => expect(result.current.data).toEqual(second));
    await act(async () => {
      resolveOld(first);
      await oldRequest;
    });
    expect(result.current.data).toEqual(second);
    expect(
      client.getQueryData(subscriptionKeys.agyCurrentQuota(0)),
    ).toBeUndefined();
  });

  it("does not reset other applications' query identity or data", async () => {
    const client = createClient();
    api.getQuota.mockResolvedValue({ ...first, tool: "codex" });
    const { result } = renderHook(() => useSubscriptionQuota("codex", true), {
      wrapper: wrapper(client),
    });
    await waitFor(() => expect(result.current.data?.tool).toBe("codex"));
    const saved = result.current.data;
    await act(async () => {
      await resetAgyCurrentQuota(client);
    });
    expect(result.current.data).toEqual(saved);
    expect(client.getQueryData(subscriptionKeys.quota("codex"))).toEqual(saved);
    expect(api.getQuota).toHaveBeenCalledTimes(1);
  });

  it("re-reads AGY events rather than copying an unscoped payload and avoids echo loops", async () => {
    const client = createClient();
    api.getQuota.mockResolvedValue(first);
    const { result } = renderHook(
      () => {
        useUsageCacheBridge();
        return useSubscriptionQuota("agy", true);
      },
      { wrapper: wrapper(client) },
    );
    await waitFor(() => expect(result.current.data).toEqual(first));
    let resolveRefresh!: (value: SubscriptionQuota) => void;
    const refresh = new Promise<SubscriptionQuota>((resolve) => {
      resolveRefresh = resolve;
    });
    api.getQuota.mockReturnValue(refresh);
    act(() =>
      emitTauriEvent("usage-cache-updated", {
        kind: "subscription",
        appType: "agy",
        data: quota(99, Date.parse("2026-10-21T23:58:00Z")),
      }),
    );
    await waitFor(() => expect(api.getQuota).toHaveBeenCalledTimes(2));
    await act(async () =>
      emitTauriEvent("usage-cache-updated", {
        kind: "subscription",
        appType: "agy",
        data: quota(95, Date.parse("2026-10-21T23:59:00Z")),
      }),
    );
    expect(api.getQuota).toHaveBeenCalledTimes(2);
    await act(async () => {
      resolveRefresh(second);
      await refresh;
    });
    await waitFor(() => expect(result.current.data).toEqual(second));
    expect(client.getQueryData(subscriptionKeys.quota("agy"))).toBeUndefined();
    const calls = api.getQuota.mock.calls.length;
    await act(async () =>
      emitTauriEvent("usage-cache-updated", {
        kind: "subscription",
        appType: "agy",
        data: second,
      }),
    );
    expect(api.getQuota).toHaveBeenCalledTimes(calls);
    act(() =>
      emitTauriEvent("usage-cache-updated", {
        kind: "subscription",
        appType: "codex",
        data: { ...first, tool: "codex" },
      }),
    );
    expect(client.getQueryData(subscriptionKeys.quota("codex"))).toEqual({
      ...first,
      tool: "codex",
    });
  });
});
