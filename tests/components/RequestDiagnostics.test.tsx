import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  RequestDiagnostics,
  attemptPhases,
} from "@/components/usage/RequestDiagnostics";
import type { AttemptDiagnostics } from "@/lib/api/requestDiagnostics";

const api = vi.hoisted(() => ({
  get: vi.fn(),
  getConfig: vi.fn(),
  setConfig: vi.fn(),
}));
vi.mock("@/lib/api/requestDiagnostics", () => ({ requestDiagnosticsApi: api }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));

const attempt: AttemptDiagnostics = {
  providerId: "p",
  providerName: "Provider",
  model: "m",
  startedMs: 0,
  preparedMs: 10,
  headersMs: 110,
  firstContentMs: 130,
  durationMs: 330,
  statusCode: 200,
  error: null,
  canceled: false,
};

describe("RequestDiagnostics", () => {
  beforeEach(() => {
    api.get.mockResolvedValue(null);
    api.getConfig.mockResolvedValue({ captureBodies: false });
    api.setConfig.mockResolvedValue(undefined);
  });

  it("shows measured phases and leaves missing timings unavailable", () => {
    expect(attemptPhases(attempt).map(({ key, ms }) => [key, ms])).toEqual([
      ["prepare", 10],
      ["headers", 100],
      ["content", 20],
      ["receive", 200],
    ]);
    expect(
      attemptPhases({
        ...attempt,
        preparedMs: null,
        headersMs: null,
        firstContentMs: null,
        durationMs: null,
      }),
    ).toEqual([]);
  });

  it("enables body capture explicitly for future requests", async () => {
    render(<RequestDiagnostics requestId="req" />);
    fireEvent.click(
      await screen.findByRole("button", {
        name: "requestDiagnostics.captureOff",
      }),
    );
    await waitFor(() =>
      expect(api.setConfig).toHaveBeenCalledWith({ captureBodies: true }),
    );
    expect(
      await screen.findByRole("button", {
        name: "requestDiagnostics.captureOn",
      }),
    ).toHaveAttribute("aria-pressed", "true");
  });

  it("renders response text without executing markup and lists retries", async () => {
    api.get.mockResolvedValue({
      captureBodies: true,
      requestBody: "{masked}",
      responseBody: "<script>bad()</script>",
      requestTruncated: false,
      responseTruncated: true,
      attemptsTruncated: false,
      attempts: [
        { ...attempt, statusCode: 429, error: "Rate limited" },
        attempt,
      ],
    });
    const { container } = render(<RequestDiagnostics requestId="req" />);
    expect(await screen.findByText("1. Provider")).toBeInTheDocument();
    expect(screen.getByText("2. Provider")).toBeInTheDocument();
    expect(screen.getByText("<script>bad()</script>")).toBeInTheDocument();
    expect(container.querySelector("script")).toBeNull();
    expect(
      screen.getByText("requestDiagnostics.truncated"),
    ).toBeInTheDocument();
  });

  it("clears the displayed preview after capture is revoked", async () => {
    api.getConfig.mockResolvedValue({ captureBodies: true });
    api.get.mockResolvedValue({
      captureBodies: true,
      requestBody: "private preview",
      responseBody: "",
      attempts: [],
      requestTruncated: false,
      responseTruncated: false,
      attemptsTruncated: false,
    });
    render(<RequestDiagnostics requestId="req" />);
    expect(await screen.findByText("private preview")).toBeInTheDocument();
    fireEvent.click(
      await screen.findByRole("button", {
        name: "requestDiagnostics.captureOn",
      }),
    );
    await waitFor(() =>
      expect(screen.queryByText("private preview")).toBeNull(),
    );
    expect(api.setConfig).toHaveBeenCalledWith({ captureBodies: false });
  });

  it("does not resurrect an in-flight preview after capture is revoked", async () => {
    let resolveRead!: (value: unknown) => void;
    api.get.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveRead = resolve;
        }),
    );
    api.getConfig.mockResolvedValue({ captureBodies: true });
    render(<RequestDiagnostics requestId="req" />);
    fireEvent.click(
      await screen.findByRole("button", {
        name: "requestDiagnostics.captureOn",
      }),
    );
    await screen.findByRole("button", {
      name: "requestDiagnostics.captureOff",
    });
    resolveRead({
      captureBodies: true,
      requestBody: "revoked preview",
      responseBody: "",
      attempts: [],
      requestTruncated: false,
      responseTruncated: false,
      attemptsTruncated: false,
    });
    await waitFor(() =>
      expect(screen.queryByText("revoked preview")).toBeNull(),
    );
  });
});
