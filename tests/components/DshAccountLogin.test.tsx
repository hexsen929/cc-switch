import { act, fireEvent, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { DshAccountLogin } from "@/components/providers/DshAccount";
import {
  dshApi,
  type DshAccountStatus,
  type DshLoginStart,
} from "@/lib/api/dsh";
import { settingsApi } from "@/lib/api";

vi.mock("@/lib/api/dsh", () => ({
  dshApi: {
    accountStatus: vi.fn(),
    accountSignOut: vi.fn(),
    loginStart: vi.fn(),
    loginPoll: vi.fn(),
    loginCancel: vi.fn(),
  },
}));
vi.mock("@/lib/api", () => ({ settingsApi: { openExternal: vi.fn() } }));
vi.mock("@/lib/toast", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));
vi.mock("@/components/ConfirmDialog", () => ({ ConfirmDialog: () => null }));
vi.mock("@/components/providers/ProviderCard", () => ({
  CardChipBadge: () => null,
}));
vi.mock("@/components/providers/ProviderCardActions", () => ({
  ProviderCardActions: () => null,
}));
vi.mock("@/components/settings/auth/AccountQuota", () => ({
  QuotaRefreshCell: () => null,
}));
vi.mock("@/components/settings/auth/ManagedAccountsGroup", () => ({
  ManagedAccountsGroup: ({
    onAdd,
    login,
  }: {
    onAdd: () => void;
    login: { error: string | null; onCancel: () => void };
  }) => (
    <div>
      <button onClick={onAdd}>start-login</button>
      <button onClick={login.onCancel}>cancel-login</button>
      <output data-testid="login-error">{login.error}</output>
    </div>
  ),
}));

const signedOut: DshAccountStatus = {
  signedIn: false,
  expired: false,
  issuer: null,
  profile: null,
  wallets: [],
  bonusWallets: [],
  usageUrl: null,
  topUpUrl: null,
  error: null,
};
const flow = (flowId: string): DshLoginStart => ({
  flowId,
  authUrl: `https://example.test/login/${flowId}`,
  expiresIn: 300,
});
function deferred<T>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((onResolve, onReject) => {
    resolve = onResolve;
    reject = onReject;
  });
  return { promise, resolve, reject };
}
function renderLogin() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: Infinity } },
  });
  return render(
    <QueryClientProvider client={client}>
      <DshAccountLogin />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.mocked(dshApi.accountStatus).mockReset().mockResolvedValue(signedOut);
  vi.mocked(dshApi.loginStart).mockReset();
  vi.mocked(dshApi.loginPoll).mockReset().mockResolvedValue(false);
  vi.mocked(dshApi.loginCancel).mockReset().mockResolvedValue(undefined);
  vi.mocked(settingsApi.openExternal).mockReset().mockResolvedValue(undefined);
});
afterEach(() => vi.useRealTimers());

describe("DeepSeek login lifecycle", () => {
  it("cancels a late start after unmount instead of opening its browser", async () => {
    const pending = deferred<DshLoginStart>();
    vi.mocked(dshApi.loginStart).mockReturnValue(pending.promise);
    const { unmount } = renderLogin();
    fireEvent.click(screen.getByText("start-login"));
    unmount();

    await act(async () => pending.resolve(flow("abandoned")));

    expect(dshApi.loginCancel).toHaveBeenCalledWith("abandoned");
    expect(settingsApi.openExternal).not.toHaveBeenCalled();
  });

  it("retires a superseded start without replacing the newer flow", async () => {
    const older = deferred<DshLoginStart>();
    const newer = deferred<DshLoginStart>();
    vi.mocked(dshApi.loginStart)
      .mockReturnValueOnce(older.promise)
      .mockReturnValueOnce(newer.promise);
    renderLogin();
    fireEvent.click(screen.getByText("start-login"));
    fireEvent.click(screen.getByText("start-login"));
    await act(async () => newer.resolve(flow("newer")));
    await act(async () => older.resolve(flow("older")));

    expect(dshApi.loginCancel).toHaveBeenCalledWith("older");
    expect(settingsApi.openExternal).toHaveBeenCalledTimes(1);
    expect(settingsApi.openExternal).toHaveBeenCalledWith(
      flow("newer").authUrl,
    );
    act(() => {
      vi.advanceTimersByTime(2_000);
    });
    expect(dshApi.loginPoll).toHaveBeenCalledWith("newer");
  });

  it("ignores a previous poll failure after cancellation and a new start", async () => {
    const previousPoll = deferred<boolean>();
    vi.mocked(dshApi.loginStart)
      .mockResolvedValueOnce(flow("first"))
      .mockResolvedValueOnce(flow("second"));
    vi.mocked(dshApi.loginPoll).mockReturnValueOnce(previousPoll.promise);
    renderLogin();
    await act(async () => {
      fireEvent.click(screen.getByText("start-login"));
    });
    act(() => {
      vi.advanceTimersByTime(2_000);
    });
    expect(dshApi.loginPoll).toHaveBeenCalledWith("first");
    fireEvent.click(screen.getByText("cancel-login"));
    await act(async () => {
      fireEvent.click(screen.getByText("start-login"));
    });

    await act(async () =>
      previousPoll.reject(new Error("obsolete poll failed")),
    );

    expect(screen.getByTestId("login-error")).toBeEmptyDOMElement();
    act(() => {
      vi.advanceTimersByTime(2_000);
    });
    expect(dshApi.loginPoll).toHaveBeenCalledWith("second");
  });
});
