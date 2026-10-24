import { describe, expect, it } from "vitest";
import en from "@/i18n/locales/en.json";
import ja from "@/i18n/locales/ja.json";
import tr from "@/i18n/locales/tr.json";
import zh from "@/i18n/locales/zh.json";
import zhTW from "@/i18n/locales/zh-TW.json";

const paths = [
  "appsPage.sectionInstalled",
  "appsPage.sectionNotInstalled",
  "appsPage.dragHandle",
  "codexOauth.loginMethod",
  "codexOauth.browserLogin",
  "codexOauth.deviceCodeLogin",
  "codexOauth.openBrowserLogin",
  "codexOauth.loginExpired",
];

function textAt(locale: unknown, path: string): unknown {
  return path.split(".").reduce<unknown>((node, key) => {
    if (!node || typeof node !== "object") return undefined;
    return (node as Record<string, unknown>)[key];
  }, locale);
}

describe("Application ordering and browser login locale parity", () => {
  it.each(Object.entries({ en, ja, tr, zh, "zh-TW": zhTW }))(
    "%s defines the new UI strings with matching placeholders",
    (_language, locale) => {
      for (const path of paths) {
        const own = textAt(locale, path);
        const reference = textAt(en, path) as string;
        expect(typeof own, path).toBe("string");
        expect((own as string).trim(), path).not.toBe("");
        expect((own as string).match(/\{\{[^}]+\}\}/g) ?? [], path).toEqual(
          reference.match(/\{\{[^}]+\}\}/g) ?? [],
        );
      }
    },
  );
});
