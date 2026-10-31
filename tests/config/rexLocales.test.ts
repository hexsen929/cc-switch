import { describe, expect, it } from "vitest";
import en from "@/i18n/locales/en.json";
import ja from "@/i18n/locales/ja.json";
import tr from "@/i18n/locales/tr.json";
import zh from "@/i18n/locales/zh.json";
import zhTW from "@/i18n/locales/zh-TW.json";

describe("Rex terminal locale parity", () => {
  it.each(Object.entries({ en, ja, tr, zh, "zh-TW": zhTW }))(
    "%s uses one name for Rex and its legacy preference",
    (_language, locale) => {
      const terminals = locale.settings.terminal.options.macos;
      expect(terminals.rex).toBe("Rex Beta");
      expect(terminals["rex-zh"]).toBe(terminals.rex);
      const help = locale.settings.terminal.rexHelp;
      expect(help.trim()).not.toBe("");
      for (const term of [
        "CLI",
        "PATH",
        "Rex Beta.app",
        "Rex Beta 中文.app",
        "Rex Server",
        "Remote Control",
        "CC-Switch",
      ]) {
        expect(help).toContain(term);
      }
    },
  );
});
