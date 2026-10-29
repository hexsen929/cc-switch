import type { ReactNode } from "react";
import { act, renderHook, waitFor } from "@testing-library/react";
import {
  QueryClient,
  QueryClientProvider,
  useQuery,
} from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useSaveSettingsMutation } from "@/lib/query/mutations";
import type { Settings } from "@/types";

const apiMocks = vi.hoisted(() => ({
  save: vi.fn(),
  getClaudeLoginSwitch: vi.fn(),
}));

vi.mock("@/lib/api", () => ({
  settingsApi: { save: (...args: unknown[]) => apiMocks.save(...args) },
  providersApi: {},
  sessionsApi: {},
}));

beforeEach(() => {
  apiMocks.save.mockReset().mockResolvedValue(undefined);
  apiMocks.getClaudeLoginSwitch.mockReset();
});

function createHarness(initiallyWsl: boolean) {
  const client = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false },
    },
  });
  const initialStatus = {
    active: !initiallyWsl,
    blocked: initiallyWsl ? "wsl" : null,
    live: null,
  };
  client.setQueryData(["claude-login-switch"], initialStatus);
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  const hook = renderHook(
    () => ({
      save: useSaveSettingsMutation(),
      status: useQuery({
        queryKey: ["claude-login-switch"],
        queryFn: () => apiMocks.getClaudeLoginSwitch(),
        staleTime: 30_000,
      }),
    }),
    { wrapper },
  );
  return { ...hook, client, initialStatus };
}

describe("useSaveSettingsMutation", () => {
  it.each([true, false])(
    "refreshes cached Claude account capability after saving a directory (WSL=%s)",
    async (toWsl) => {
      const { result } = createHarness(!toWsl);
      expect(apiMocks.getClaudeLoginSwitch).not.toHaveBeenCalled();
      const nextStatus = {
        active: !toWsl,
        blocked: toWsl ? "wsl" : null,
        live: null,
      };
      apiMocks.getClaudeLoginSwitch.mockResolvedValue(nextStatus);
      const payload: Settings = {
        showInTray: true,
        minimizeToTrayOnClose: true,
        claudeConfigDir: toWsl
          ? "\\\\wsl.localhost\\Ubuntu\\home\\fixture\\.claude"
          : undefined,
      };
      await act(async () => result.current.save.mutateAsync(payload));
      await waitFor(() =>
        expect(result.current.status.data).toEqual(nextStatus),
      );
      expect(apiMocks.save).toHaveBeenCalledWith(payload);
      expect(apiMocks.getClaudeLoginSwitch).toHaveBeenCalledTimes(1);
    },
  );

  it("keeps the previous capability cache when saving fails", async () => {
    const { result, initialStatus } = createHarness(false);
    apiMocks.save.mockRejectedValueOnce(new Error("fixture save failure"));
    await act(async () => {
      await expect(
        result.current.save.mutateAsync({
          showInTray: true,
          minimizeToTrayOnClose: true,
          claudeConfigDir: "\\\\wsl.localhost\\Ubuntu\\home\\fixture\\.claude",
        }),
      ).rejects.toThrow("fixture save failure");
    });
    expect(result.current.status.data).toEqual(initialStatus);
    expect(apiMocks.getClaudeLoginSwitch).not.toHaveBeenCalled();
  });
});
