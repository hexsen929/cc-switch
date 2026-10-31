import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import {
  QueryClient,
  QueryClientProvider,
  useQuery,
  type QueryKey,
} from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";
import { RefreshAllQuotaButton } from "@/components/settings/auth/RefreshAllQuotaButton";

vi.mock("@/components/providers/DshAccount", () => ({
  DSH_ACCOUNT_STATUS_KEY: ["dsh-account-status"],
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));

function Probe({
  queryKey,
  queryFn,
  enabled = true,
}: {
  queryKey: QueryKey;
  queryFn: () => Promise<unknown>;
  enabled?: boolean;
}) {
  useQuery({ queryKey, queryFn, enabled, staleTime: Infinity });
  return null;
}

function client() {
  return new QueryClient({
    defaultOptions: {
      queries: { retry: false, refetchOnWindowFocus: false },
    },
  });
}

describe("RefreshAllQuotaButton", () => {
  it("refreshes active account quotas and DSH balance without querying unrelated or disabled sources", async () => {
    const queryClient = client();
    const quotas = [
      ["claude_official", "quota", "claude-a"],
      ["copilot", "quota", "copilot-a"],
      ["codex_oauth", "quota", "codex-a"],
      ["xai_oauth", "quota", "xai-a"],
      ["kimi_oauth", "quota", "kimi-a"],
      ["agy_oauth", "quota", "agy-a"],
      ["dsh-account-status"],
    ].map((queryKey) => ({
      queryKey,
      queryFn: vi.fn().mockResolvedValue({ success: true }),
    }));
    const unrelated = [
      ["managed-auth-status", "claude_official"],
      ["claude_official", "default_plan"],
      ["providers", "codex"],
      ["subscription", "quota", "claude"],
      ["other-service", "quota", "account"],
    ].map((queryKey) => ({
      queryKey,
      queryFn: vi.fn().mockResolvedValue({}),
    }));
    const disabled = vi.fn().mockResolvedValue({});
    const inactive = vi.fn().mockResolvedValue({});
    await queryClient.fetchQuery({
      queryKey: ["codex_oauth", "quota", "inactive-account"],
      queryFn: inactive,
    });
    inactive.mockClear();

    render(
      <QueryClientProvider client={queryClient}>
        {[...quotas, ...unrelated].map((probe) => (
          <Probe key={JSON.stringify(probe.queryKey)} {...probe} />
        ))}
        <Probe
          queryKey={["claude_official", "quota", "disabled-account"]}
          queryFn={disabled}
          enabled={false}
        />
        <RefreshAllQuotaButton />
      </QueryClientProvider>,
    );

    const button = screen.getByRole("button", {
      name: "authCenter.quota.refreshAll",
    });
    await waitFor(() => {
      expect(button).toBeEnabled();
      for (const probe of [...quotas, ...unrelated]) {
        expect(probe.queryFn).toHaveBeenCalledTimes(1);
      }
    });
    for (const probe of [...quotas, ...unrelated]) {
      probe.queryFn.mockClear();
    }

    fireEvent.click(button);
    await waitFor(() => {
      for (const probe of quotas) {
        expect(probe.queryFn).toHaveBeenCalledTimes(1);
      }
    });
    for (const probe of unrelated) {
      expect(probe.queryFn).not.toHaveBeenCalled();
    }
    expect(disabled).not.toHaveBeenCalled();
    expect(inactive).not.toHaveBeenCalled();
  });

  it("blocks duplicate refreshes while an account request is still running", async () => {
    let finish: (() => void) | undefined;
    const queryFn = vi.fn(
      () =>
        new Promise<{ success: boolean }>((resolve) => {
          finish = () => resolve({ success: true });
        }),
    );
    render(
      <QueryClientProvider client={client()}>
        <Probe
          queryKey={["claude_official", "quota", "account"]}
          queryFn={queryFn}
        />
        <RefreshAllQuotaButton />
      </QueryClientProvider>,
    );
    const button = screen.getByRole("button", {
      name: "authCenter.quota.refreshAll",
    });
    await waitFor(() => expect(button).toBeDisabled());
    fireEvent.click(button);
    expect(queryFn).toHaveBeenCalledTimes(1);
    finish?.();
    await waitFor(() => expect(button).toBeEnabled());
  });
});
