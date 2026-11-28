import { useState } from "react";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { RoutingModelInput } from "@/components/proxy/RoutingModelInput";
import { queryClient } from "@/lib/query/queryClient";
import type { FetchedModel } from "@/lib/api/model-fetch";
import type { Provider } from "@/types";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

const card = (name: string, key = "fixture"): Provider => ({
  id: name,
  name,
  settingsConfig: {
    auth: { OPENAI_API_KEY: key },
    config: `model_provider="custom"\n[model_providers.custom]\nbase_url="https://${name}.example.test/v1"`,
  },
});
const fetchButton = (row: HTMLElement) =>
  within(row).getByRole("button", { name: "providerForm.fetchModels" });
function Field({ provider, label }: { provider?: Provider; label: string }) {
  const [value, setValue] = useState("original");
  return (
    <section aria-label={label}>
      <RoutingModelInput
        app="codex"
        provider={provider}
        value={value}
        label={label}
        disabled={false}
        onChange={setValue}
      />
    </section>
  );
}

describe("routing model catalog", () => {
  beforeEach(() => {
    queryClient.clear();
    invoke.mockReset();
    Element.prototype.scrollIntoView = vi.fn();
  });

  it("opens the actual searchable menu after one fetch, applies only the selected row and shares the catalog", async () => {
    const p = card("a");
    render(
      <>
        <Field label="classifier" provider={p} />
        <Field label="answer" provider={p} />
      </>,
    );
    expect(invoke).not.toHaveBeenCalled();
    invoke.mockResolvedValue([
      { id: "small", ownedBy: "vendor" },
      { id: "large", ownedBy: "__proto__" },
      { id: "small", ownedBy: "vendor" },
    ]);
    fireEvent.click(
      fetchButton(screen.getByRole("region", { name: "classifier" })),
    );
    const search = await screen.findByRole("combobox", {
      name: "Search models...",
    });
    fireEvent.change(search, { target: { value: "large" } });
    fireEvent.click(await screen.findByRole("option", { name: "large" }));
    expect(screen.getByRole("textbox", { name: "classifier" })).toHaveValue(
      "large",
    );
    expect(screen.getByRole("textbox", { name: "answer" })).toHaveValue(
      "original",
    );
    fireEvent.click(
      within(screen.getByRole("region", { name: "answer" })).getByRole(
        "button",
        { name: "Select model" },
      ),
    );
    fireEvent.click(await screen.findByRole("option", { name: "small" }));
    expect(screen.getByRole("textbox", { name: "answer" })).toHaveValue(
      "small",
    );
    expect(invoke).toHaveBeenCalledTimes(1);
  });

  it("keeps an old response out of a different card and out of a replaced binding with the same id", async () => {
    let finish!: (rows: FetchedModel[]) => void;
    invoke.mockReturnValueOnce(
      new Promise<FetchedModel[]>((resolve) => {
        finish = resolve;
      }),
    );
    const a = card("a");
    const view = render(<Field provider={a} label="model" />);
    fireEvent.click(fetchButton(screen.getByRole("region", { name: "model" })));
    await waitFor(() => expect(invoke).toHaveBeenCalledTimes(1));
    view.rerender(<Field provider={card("b")} label="model" />);
    await act(async () => finish([{ id: "from-a", ownedBy: null }]));
    expect(screen.queryByRole("option")).toBeNull();
    expect(screen.queryByRole("button", { name: "Select model" })).toBeNull();
    view.rerender(
      <Field provider={card("a", "new-account-key")} label="model" />,
    );
    expect(screen.queryByRole("button", { name: "Select model" })).toBeNull();
    expect(screen.getByRole("textbox", { name: "model" })).toHaveValue(
      "original",
    );
  });

  it("keeps a manually entered id after an empty response or failure", async () => {
    const p = card("empty");
    const view = render(<Field label="model" />);
    expect(
      fetchButton(screen.getByRole("region", { name: "model" })),
    ).toBeDisabled();
    view.rerender(<Field provider={p} label="model" />);
    fireEvent.change(screen.getByRole("textbox", { name: "model" }), {
      target: { value: "private-model" },
    });
    const button = fetchButton(screen.getByRole("region", { name: "model" }));
    invoke
      .mockResolvedValueOnce([])
      .mockRejectedValueOnce(new Error("HTTP 404"));
    for (const n of [1, 2]) {
      fireEvent.click(button);
      await waitFor(() => expect(invoke).toHaveBeenCalledTimes(n));
      await act(async () => {});
      await waitFor(() => expect(button).toBeEnabled());
      expect(screen.queryByRole("option")).toBeNull();
      expect(screen.getByRole("textbox", { name: "model" })).toHaveValue(
        "private-model",
      );
    }
  });
});
