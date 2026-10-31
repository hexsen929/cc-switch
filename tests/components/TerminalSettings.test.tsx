import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TerminalSelect } from "@/components/settings/TerminalSettings";

const platform = vi.hoisted(() => ({ name: "macos" }));

vi.mock("@/lib/platform", () => ({
  isMac: () => platform.name === "macos",
  isWindows: () => platform.name === "windows",
  isLinux: () => platform.name === "linux",
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));

const macLabel = (terminal: string) =>
  `settings.terminal.options.macos.${terminal}`;

describe("TerminalSelect", () => {
  let scrollIntoView: PropertyDescriptor | undefined;

  beforeEach(() => {
    platform.name = "macos";
    scrollIntoView = Object.getOwnPropertyDescriptor(
      Element.prototype,
      "scrollIntoView",
    );
    Object.defineProperty(Element.prototype, "scrollIntoView", {
      configurable: true,
      value: vi.fn(),
    });
  });

  afterEach(() => {
    if (scrollIntoView) {
      Object.defineProperty(
        Element.prototype,
        "scrollIntoView",
        scrollIntoView,
      );
    } else {
      delete (Element.prototype as Partial<Element>).scrollIntoView;
    }
  });

  it.each(["rex", "rex-zh"])(
    "selects the %s app independently without replacing existing terminals",
    async (terminal) => {
      const onChange = vi.fn();
      const user = userEvent.setup();
      render(<TerminalSelect onChange={onChange} />);
      const select = screen.getByRole("combobox", {
        name: "settings.terminal.title",
      });
      expect(select).toHaveTextContent(macLabel("terminal"));
      await user.click(select);

      for (const existing of ["terminal", "iterm2", "ghostty", "warp"]) {
        expect(
          screen.getByRole("option", { name: macLabel(existing) }),
        ).toBeInTheDocument();
      }
      await user.click(
        await screen.findByRole("option", { name: macLabel(terminal) }),
      );
      expect(onChange).toHaveBeenCalledWith(terminal);
    },
  );

  it.each(["rex", "rex-zh"])("displays the saved %s preference", (terminal) => {
    render(<TerminalSelect value={terminal} onChange={vi.fn()} />);
    expect(screen.getByRole("combobox")).toHaveTextContent(macLabel(terminal));
  });

  it.each(["windows", "linux"])(
    "does not offer macOS Rex apps on %s",
    async (name) => {
      platform.name = name;
      const user = userEvent.setup();
      render(<TerminalSelect onChange={vi.fn()} />);
      await user.click(screen.getByRole("combobox"));
      for (const terminal of ["rex", "rex-zh"]) {
        expect(
          screen.queryByRole("option", { name: macLabel(terminal) }),
        ).not.toBeInTheDocument();
      }
    },
  );
});
