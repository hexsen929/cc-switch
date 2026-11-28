import { useState } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { SmartIntentEditor } from "@/components/proxy/SmartIntentEditor";
import { validateSmartGraph } from "@/components/proxy/smartIntentGraph";
import type { SmartIntentConfig } from "@/types/proxy";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (k: string) => k }),
}));

describe("smart intent settings", () => {
  it("edits a named group, references it from the root and selects a native classifier", () => {
    let last: SmartIntentConfig | undefined;
    function Form() {
      const [value, setValue] = useState<SmartIntentConfig>();
      return (
        <SmartIntentEditor
          appType="codex"
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
    fireEvent.click(screen.getByRole("switch", { name: "smartIntent.title" }));
    fireEvent.change(screen.getByLabelText("smartIntent.groupName"), {
      target: { value: "dev" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: "smartIntent.addGroup" }),
    );
    expect(last?.groups?.dev.orderedOnly).toBe(true);
    fireEvent.change(
      screen.getByRole("combobox", { name: "smartIntent.group" }),
      { target: { value: "" } },
    );
    fireEvent.change(screen.getByLabelText("smartIntent.protocol"), {
      target: { value: "systemOne" },
    });
    fireEvent.change(screen.getAllByLabelText("smartIntent.provider")[1], {
      target: { value: "group:dev" },
    });
    expect(last?.classifiers[0].protocol).toBe("systemOne");
    expect(last?.categories[0].targets[0].groupId).toBe("dev");
    fireEvent.change(
      screen.getByRole("combobox", { name: "smartIntent.group" }),
      { target: { value: "dev" } },
    );
    expect(
      screen.getByRole("button", { name: "smartIntent.removeGroup" }),
    ).toBeDisabled();
    fireEvent.change(screen.getByLabelText("smartIntent.groupName"), {
      target: { value: "judges" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: "smartIntent.addGroup" }),
    );
    fireEvent.click(
      screen.getByRole("switch", { name: "smartIntent.decisionPool" }),
    );
    fireEvent.change(screen.getByLabelText("smartIntent.protocol"), {
      target: { value: "openaiDecisions" },
    });
    fireEvent.change(
      screen.getByRole("combobox", { name: "smartIntent.group" }),
      { target: { value: "" } },
    );
    fireEvent.change(screen.getAllByLabelText("smartIntent.provider")[0], {
      target: { value: "group:judges" },
    });
    expect(last?.classifiers[0].groupId).toBe("judges");
    expect(last?.groups?.judges.members?.[0].protocol).toBe("openaiDecisions");
  });
  it("validates even detached cycles, unknown references and missing members", () => {
    const node = (g: string): SmartIntentConfig => ({
      enabled: true,
      orderedOnly: true,
      classifiers: [],
      categories: [],
      prompt: "",
      timeoutMs: 1500,
      members: [{ groupId: g, model: "", providerId: "" }],
    });
    const cfg = node("dev");
    expect(validateSmartGraph(cfg, [])).toBe("missingGroup");
    cfg.groups = { dev: node("dev") };
    expect(validateSmartGraph(cfg, [])).toBe("groupCycle");
    cfg.groups.dev.members = [];
    expect(validateSmartGraph(cfg, [])).toBe("emptyGroup");
    cfg.groups.dev.members = [
      { providerId: "card", model: "one" },
      { providerId: "card", model: "two" },
    ];
    expect(validateSmartGraph(cfg, ["card"])).toBeUndefined();
    cfg.groups.dev.decisionPool = true;
    expect(validateSmartGraph(cfg, ["card"])).toBe("poolType");
  });
  it("starts off and edits classifications separately from the classifier", () => {
    let last: SmartIntentConfig | undefined;
    function Form() {
      const [value, setValue] = useState<SmartIntentConfig>();
      return (
        <SmartIntentEditor
          appType="codex"
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
