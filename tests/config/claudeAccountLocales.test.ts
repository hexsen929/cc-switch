import { describe, expect, it } from "vitest";
import en from "@/i18n/locales/en.json";
import ja from "@/i18n/locales/ja.json";
import tr from "@/i18n/locales/tr.json";
import zh from "@/i18n/locales/zh.json";
import zhTW from "@/i18n/locales/zh-TW.json";

function leaves(node: unknown, path = ""): Record<string, string> {
  if (typeof node === "string") return { [path]: node };
  return Object.fromEntries(
    Object.entries(node as Record<string, unknown>).flatMap(([key, value]) =>
      Object.entries(leaves(value, path ? `${path}.${key}` : key)),
    ),
  );
}

interface AccountLocale {
  provider: { form: { claudeAccount: unknown } };
  authCenter: {
    claude: unknown;
    group: { claudeHelp: string; claudeHelpTerminal: string };
  };
  providerCard: { action: { openTerminal: string } };
}

function accountStrings(locale: AccountLocale): Record<string, string> {
  return {
    ...leaves(
      locale.provider.form.claudeAccount,
      "provider.form.claudeAccount",
    ),
    ...leaves(locale.authCenter.claude, "authCenter.claude"),
    "authCenter.group.claudeHelp": locale.authCenter.group.claudeHelp,
    "authCenter.group.claudeHelpTerminal":
      locale.authCenter.group.claudeHelpTerminal,
    "providerCard.action.openTerminal": locale.providerCard.action.openTerminal,
  };
}

describe("Claude official account locale parity", () => {
  const reference = accountStrings(en);

  it.each(Object.entries({ en, ja, tr, zh, "zh-TW": zhTW }))(
    "%s defines account switching, quota, plan and terminal labels",
    (_language, locale) => {
      const own = accountStrings(locale);
      expect(Object.keys(own).sort()).toEqual(Object.keys(reference).sort());
      for (const [path, text] of Object.entries(reference)) {
        expect(own[path].trim(), path).not.toBe("");
        expect(own[path].match(/\{\{[^}]+\}\}/g) ?? [], path).toEqual(
          text.match(/\{\{[^}]+\}\}/g) ?? [],
        );
      }
    },
  );
});
