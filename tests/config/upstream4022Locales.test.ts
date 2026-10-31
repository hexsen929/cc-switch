import { describe, expect, it } from "vitest";
import en from "@/i18n/locales/en.json";
import ja from "@/i18n/locales/ja.json";
import tr from "@/i18n/locales/tr.json";
import zh from "@/i18n/locales/zh.json";
import zhTW from "@/i18n/locales/zh-TW.json";

const paths = [
  "authCenter.quota.refreshAll",
  "dsh.account.balance",
  "dsh.account.balanceFailed",
  "dsh.account.bonus",
  "dsh.account.cardSubtitle",
  "dsh.account.editDisabled",
  "dsh.account.empty",
  "dsh.account.expired",
  "dsh.account.expiredChip",
  "dsh.account.help",
  "dsh.account.manageAccounts",
  "dsh.account.managedHint",
  "dsh.account.presetDescription",
  "dsh.account.presetName",
  "dsh.account.shared",
  "dsh.account.signInFailed",
  "dsh.account.signOut",
  "dsh.account.signOutConfirm",
  "dsh.account.signOutFailed",
  "dsh.account.signedIn",
  "dsh.account.statusFailed",
  "dsh.account.unnamed",
  "dsh.sharedProviders",
  "dsh.sharedProvidersNote",
  "mcpPage.import.dshNote",
  "provider.liveConfigReadFailed",
  "provider.liveConfigReadFailedHint",
  "provider.openTerminalDialog.extraArgs",
  "provider.openTerminalDialog.extraArgsHint",
  "provider.openTerminalDialog.extraArgsPlaceholder",
  "providerForm.partnerPromotion.ospreyai",
  "sessionManager.dshMergedNote",
  "settings.webdavSync.clear",
  "settings.webdavSync.clearConfirm.confirm",
  "settings.webdavSync.clearConfirm.message",
  "settings.webdavSync.clearConfirm.title",
  "settings.webdavSync.clearFailed",
  "settings.webdavSync.clearSuccess",
  "settings.webdavSync.clearing",
  "subscription.expiredHintAccounts",
  "usage.sessionAccountProvider.hint",
  "usage.sessionAccountProvider.label",
  "usage.sessionAccountProvider.short",
];

function textAt(locale: unknown, path: string): unknown {
  return path.split(".").reduce<unknown>((node, key) => {
    if (!node || typeof node !== "object") return undefined;
    return (node as Record<string, unknown>)[key];
  }, locale);
}

describe("Upstream 4.0.22 locale parity", () => {
  it.each(Object.entries({ en, ja, tr, zh, "zh-TW": zhTW }))(
    "%s defines the added UI strings and interpolation values",
    (_language, locale) => {
      for (const path of paths) {
        const own = textAt(locale, path);
        const reference = textAt(en, path) as string;
        expect(typeof own, path).toBe("string");
        expect((own as string).trim(), path).not.toBe("");
        expect(
          ((own as string).match(/\{\{[^}]+\}\}/g) ?? []).sort(),
          path,
        ).toEqual((reference.match(/\{\{[^}]+\}\}/g) ?? []).sort());
      }
    },
  );
});
