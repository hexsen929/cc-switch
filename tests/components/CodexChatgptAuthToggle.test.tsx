import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { CodexChatgptAuthToggle } from "@/components/proxy/CodexChatgptAuthToggle";

const save = vi.fn();
const legacyUpdate = vi.fn();
let settings: Record<string, unknown> | undefined;
let pending = false;

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, options?: { defaultValue?: string }) =>
      options?.defaultValue ?? key,
  }),
}));
vi.mock("@/lib/query", () => ({
  useSettingsQuery: () => ({ data: settings, isLoading: false }),
  useSaveSettingsMutation: () => ({ mutate: save, isPending: pending }),
}));
// A stale legacy proxy preference must never determine the toggle or be written.
vi.mock("@/lib/query/proxy", () => ({
  useAppProxyConfig: () => ({
    data: { codexChatgptAuthTakeover: false },
    isLoading: false,
  }),
  useUpdateAppProxyConfig: () => ({
    mutateAsync: legacyUpdate,
    isPending: false,
  }),
}));

describe("CodexChatgptAuthToggle", () => {
  beforeEach(() => {
    save.mockReset();
    legacyUpdate.mockReset();
    settings = { showInTray: false, preserveCodexOfficialAuthOnSwitch: false };
    pending = false;
  });

  it("writes the canonical setting once without updating proxy configuration", () => {
    render(<CodexChatgptAuthToggle />);
    fireEvent.click(screen.getByRole("switch"));
    expect(save).toHaveBeenCalledTimes(1);
    expect(save).toHaveBeenCalledWith(
      { showInTray: false, preserveCodexOfficialAuthOnSwitch: true },
      expect.any(Object),
    );
    expect(legacyUpdate).not.toHaveBeenCalled();
  });

  it("reflects settings-page changes even when the legacy proxy flag disagrees", () => {
    const { rerender } = render(<CodexChatgptAuthToggle />);
    expect(screen.getByRole("switch")).not.toBeChecked();
    settings = { ...settings, preserveCodexOfficialAuthOnSwitch: true };
    rerender(<CodexChatgptAuthToggle />);
    expect(screen.getByRole("switch")).toBeChecked();
    fireEvent.click(screen.getByRole("switch"));
    expect(save).toHaveBeenCalledWith(
      { showInTray: false, preserveCodexOfficialAuthOnSwitch: false },
      expect.any(Object),
    );
    expect(legacyUpdate).not.toHaveBeenCalled();
  });

  it("does not construct a partial settings snapshot before settings load", () => {
    settings = undefined;
    render(<CodexChatgptAuthToggle />);
    expect(screen.getByRole("switch")).toBeDisabled();
    fireEvent.click(screen.getByRole("switch"));
    expect(save).not.toHaveBeenCalled();
  });

  it("disables repeated clicks while saving", () => {
    pending = true;
    render(<CodexChatgptAuthToggle />);
    expect(screen.getByRole("switch")).toBeDisabled();
  });

  it("explains the direct-switch scope instead of promising proxy logout", () => {
    render(<CodexChatgptAuthToggle />);
    expect(screen.getByTitle(/下次直连切换/)).toHaveAttribute(
      "title",
      expect.stringContaining("代理模式始终保留"),
    );
  });
});
