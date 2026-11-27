import { useState } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { SmartIntentEditor } from "@/components/proxy/SmartIntentEditor";
import type { SmartIntentConfig } from "@/types/proxy";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (k: string) => k }),
}));

describe("smart intent settings", () => {
  it("starts off and edits classifications separately from the classifier", () => {
    let last: SmartIntentConfig | undefined;
    function Form() {
      const [value, setValue] = useState<SmartIntentConfig>();
      return (
        <SmartIntentEditor
          disabled={false}
          providers={[]}
          value={value}
          onChange={(next) => {
            last = next;
            setValue(next);
          }}
        />
      );
    }
    render(<Form />);
    const toggle = screen.getByRole("switch", { name: "smartIntent.title" });
    expect(toggle).not.toBeChecked();
    expect(screen.queryByRole("textbox")).toBeNull();
    fireEvent.click(toggle);
    expect(last?.categories).toHaveLength(2);
    fireEvent.change(
      screen.getAllByRole("textbox", { name: "smartIntent.model" })[0],
      { target: { value: "cheap-judge" } },
    );
    expect(last?.classifiers[0].model).toBe("cheap-judge");
    expect(last?.categories[0].targets[0].model).toBe("");
    fireEvent.click(
      screen.getByRole("button", { name: "smartIntent.addCategory" }),
    );
    expect(last?.categories).toHaveLength(3);
    fireEvent.click(toggle);
    expect(last?.enabled).toBe(false);
    expect(last?.classifiers[0].model).toBe("cheap-judge");
  });
});
