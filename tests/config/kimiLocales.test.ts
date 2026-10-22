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

describe("Kimi Code locale parity", () => {
  const reference = leaves(en.kimi);

  it.each(Object.entries({ en, ja, tr, zh, "zh-TW": zhTW }))(
    "%s defines every Kimi field without losing existing diagnostics",
    (_language, locale) => {
      const own = leaves(locale.kimi);
      expect(Object.keys(own).sort()).toEqual(Object.keys(reference).sort());
      expect(Object.values(own).every((text) => text.trim())).toBe(true);
      expect(locale.requestDiagnostics.title.trim()).not.toBe("");
    },
  );
});
