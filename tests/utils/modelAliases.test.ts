import { describe, expect, it } from "vitest";
import {
  createModelAliasEntry,
  modelAliasesEqual,
  parseModelAliasesFromValue,
  serializeModelAliases,
} from "@/utils/modelAliases";

describe("modelAliases utils", () => {
  it("parses an object map into ordered entries", () => {
    const entries = parseModelAliasesFromValue({
      "codex-auto-review": "gpt-5.6-sol",
      "claude-fable-5": "special-model",
    });
    expect(entries.map((e) => ({ source: e.source, target: e.target }))).toEqual(
      [
        { source: "codex-auto-review", target: "gpt-5.6-sol" },
        { source: "claude-fable-5", target: "special-model" },
      ],
    );
    // 每条都有稳定 id 供列表渲染
    expect(entries.every((e) => typeof e.id === "string" && e.id)).toBe(true);
  });

  it("returns [] for non-object / array / nullish inputs", () => {
    expect(parseModelAliasesFromValue(undefined)).toEqual([]);
    expect(parseModelAliasesFromValue(null)).toEqual([]);
    expect(parseModelAliasesFromValue("x")).toEqual([]);
    expect(parseModelAliasesFromValue(["a"])).toEqual([]);
  });

  it("skips non-string targets when parsing", () => {
    const entries = parseModelAliasesFromValue({
      good: "target",
      bad: 123 as unknown as string,
    });
    expect(entries).toHaveLength(1);
    expect(entries[0].source).toBe("good");
  });

  it("serializes entries back to an object, trimming and dropping blanks", () => {
    expect(
      serializeModelAliases([
        createModelAliasEntry({ source: " codex-auto-review ", target: " gpt-5.6-sol " }),
        createModelAliasEntry({ source: "", target: "no-source" }),
        createModelAliasEntry({ source: "no-target", target: "  " }),
      ]),
    ).toEqual({ "codex-auto-review": "gpt-5.6-sol" });
  });

  it("keeps the first non-empty mapping when a source is duplicated", () => {
    expect(
      serializeModelAliases([
        createModelAliasEntry({ source: "dup", target: "first" }),
        createModelAliasEntry({ source: "dup", target: "second" }),
      ]),
    ).toEqual({ dup: "first" });
  });

  it("returns undefined when nothing valid remains (so the field is not written)", () => {
    expect(serializeModelAliases([])).toBeUndefined();
    expect(
      serializeModelAliases([createModelAliasEntry({ source: "", target: "" })]),
    ).toBeUndefined();
  });

  it("round-trips parse -> serialize without loss", () => {
    const stored = {
      "codex-auto-review": "gpt-5.6-sol",
      "some-model": "other-model",
    };
    expect(serializeModelAliases(parseModelAliasesFromValue(stored))).toEqual(
      stored,
    );
  });

  it("compares alias objects structurally", () => {
    expect(modelAliasesEqual({ a: "1" }, { a: "1" })).toBe(true);
    expect(modelAliasesEqual({ a: "1" }, { a: "2" })).toBe(false);
    expect(modelAliasesEqual({ a: "1" }, { a: "1", b: "2" })).toBe(false);
    expect(modelAliasesEqual(undefined, {})).toBe(true);
  });
});
