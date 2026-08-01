import { describe, expect, it } from "vitest";
import { normalizeClaudeAppendInstructionsForSave } from "@/components/providers/forms/ProviderForm";

describe("ProviderForm Claude append instructions helpers", () => {
  it("normalizes provider-scoped files and keeps the active file", () => {
    expect(
      normalizeClaudeAppendInstructionsForSave({
        files: [" ./a.md ", "./a.md", "", "bad\npath.md"],
        activeFile: " ./b.md ",
      }),
    ).toEqual({
      files: ["./a.md", "./b.md"],
      activeFile: "./b.md",
    });
  });

  it("returns a disabled empty config for a provider without files", () => {
    expect(normalizeClaudeAppendInstructionsForSave()).toEqual({
      files: [],
      activeFile: null,
    });
  });
});
