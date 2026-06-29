import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { CodexChatgptAuthToggle } from "@/components/proxy/CodexChatgptAuthToggle";

const saveSettingsMutateAsync = vi.fn();
const updateAppProxyConfigMutateAsync = vi.fn();

let settingsState: Record<string, unknown> | undefined;
let proxyConfigState: Record<string, unknown> | undefined;

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (_key: string, options?: { defaultValue?: string }) =>
      options?.defaultValue ?? _key,
  }),
}));

vi.mock("@/lib/query", () => ({
  useSettingsQuery: () => ({
    data: settingsState,
    isLoading: false,
  }),
  useSaveSettingsMutation: () => ({
    mutateAsync: saveSettingsMutateAsync,
    isPending: false,
  }),
}));

vi.mock("@/lib/query/proxy", () => ({
  useAppProxyConfig: () => ({
    data: proxyConfigState,
    isLoading: false,
  }),
  useUpdateAppProxyConfig: () => ({
    mutateAsync: updateAppProxyConfigMutateAsync,
    isPending: false,
  }),
}));

function baseProxyConfig() {
  return {
    appType: "codex",
    enabled: false,
    autoFailoverEnabled: false,
    codexChatgptAuthTakeover: false,
    maxRetries: 2,
    streamingFirstByteTimeout: 30,
    streamingIdleTimeout: 120,
    nonStreamingTimeout: 120,
    circuitFailureThreshold: 5,
    circuitSuccessThreshold: 2,
    circuitTimeoutSeconds: 60,
    circuitErrorRateThreshold: 50,
    circuitMinRequests: 10,
  };
}

describe("CodexChatgptAuthToggle", () => {
  beforeEach(() => {
    saveSettingsMutateAsync.mockReset();
    updateAppProxyConfigMutateAsync.mockReset();
    saveSettingsMutateAsync.mockResolvedValue(true);
    updateAppProxyConfigMutateAsync.mockResolvedValue(undefined);
    settingsState = {
      preserveCodexOfficialAuthOnSwitch: false,
    };
    proxyConfigState = baseProxyConfig();
  });

  it("auto-enables official auth preservation when turning on ChatGPT auth mode", async () => {
    render(<CodexChatgptAuthToggle />);

    const toggle = screen.getByRole("switch");
    expect(toggle).not.toBeDisabled();

    fireEvent.click(toggle);

    await waitFor(() => {
      expect(saveSettingsMutateAsync).toHaveBeenCalledWith({
        showInTray: true,
        minimizeToTrayOnClose: true,
        preserveCodexOfficialAuthOnSwitch: true,
      });
    });
    expect(updateAppProxyConfigMutateAsync).toHaveBeenCalledWith({
      ...baseProxyConfig(),
      codexChatgptAuthTakeover: true,
    });
  });

  it("does not require a settings-page detour when preservation is already enabled", async () => {
    settingsState = {
      preserveCodexOfficialAuthOnSwitch: true,
    };

    render(<CodexChatgptAuthToggle />);
    fireEvent.click(screen.getByRole("switch"));

    await waitFor(() => {
      expect(updateAppProxyConfigMutateAsync).toHaveBeenCalledWith({
        ...baseProxyConfig(),
        codexChatgptAuthTakeover: true,
      });
    });
    expect(saveSettingsMutateAsync).not.toHaveBeenCalled();
  });
});
