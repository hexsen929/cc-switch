import { act, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useProviderResourceWarnings } from "@/hooks/useProviderResourceWarnings";

const mocks = vi.hoisted(() => ({
  handler: undefined as
    | undefined
    | ((payload: { appType: string; error: string }) => void),
  retry: vi.fn(),
  warning: vi.fn(),
  success: vi.fn(),
}));
vi.mock("@/hooks/useTauriEvent", () => ({
  useTauriEvent: (name: string, handler: typeof mocks.handler) => {
    expect(name).toBe("provider-resources-warning");
    mocks.handler = handler;
  },
}));
vi.mock("@/lib/api/providers", () => ({
  providersApi: { retryResourceSync: mocks.retry },
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
vi.mock("sonner", () => ({
  toast: { warning: mocks.warning, success: mocks.success },
}));

describe("useProviderResourceWarnings", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.retry.mockResolvedValue(undefined);
  });

  it("reports a nonfatal warning and retries only the affected app's resources", async () => {
    renderHook(() => useProviderResourceWarnings());
    act(() =>
      mocks.handler?.({
        appType: "claude",
        error: "Instructions: file missing",
      }),
    );
    const [title, options] = mocks.warning.mock.calls[0];
    expect(title).toBe("notifications.providerResourcesWarning");
    expect(options.description).toBe("claude: Instructions: file missing");
    await act(async () => {
      options.action.onClick();
    });
    expect(mocks.retry).toHaveBeenCalledTimes(1);
    expect(mocks.retry).toHaveBeenCalledWith("claude");
    expect(mocks.success).toHaveBeenCalledWith(
      "notifications.providerResourcesSynced",
      { id: options.id },
    );
  });

  it("keeps the retry action after failure without pretending that configuration failed to save", async () => {
    mocks.retry.mockRejectedValue(new Error("still unreadable"));
    renderHook(() => useProviderResourceWarnings());
    act(() => mocks.handler?.({ appType: "codex", error: "Mcp: denied" }));
    await act(async () => {
      mocks.warning.mock.calls[0][1].action.onClick();
    });
    expect(mocks.warning).toHaveBeenCalledTimes(2);
    expect(mocks.warning.mock.lastCall?.[1].description).toContain(
      "still unreadable",
    );
    expect(mocks.warning.mock.lastCall?.[1].action.onClick).toBeTypeOf(
      "function",
    );
    expect(mocks.success).not.toHaveBeenCalled();
  });
});
