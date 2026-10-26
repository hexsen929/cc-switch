import type { ComponentProps, ReactNode } from "react";
import { QueryClientProvider } from "@tanstack/react-query";
import { act, render as rtlRender, screen } from "@testing-library/react";
import {
  createTestQueryClient,
  renderWithQueryClient as render,
} from "../utils/testQueryClient";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { UsageTrendChart } from "@/components/usage/UsageTrendChart";

const trendState = vi.hoisted(() => ({
  isLoading: false,
  chartWidths: [] as number[],
}));

vi.mock("recharts", async (importOriginal) => {
  const recharts = await importOriginal<typeof import("recharts")>();
  return {
    ...recharts,
    // jsdom has no layout; keep the actual charts with fixed dimensions.
    AreaChart: (props: ComponentProps<typeof recharts.AreaChart>) => {
      trendState.chartWidths.push(Number(props.width));
      return (
        <recharts.AreaChart
          {...props}
          responsive={false}
          width={640}
          height={132}
        />
      );
    },
    BarChart: (props: ComponentProps<typeof recharts.BarChart>) => {
      trendState.chartWidths.push(Number(props.width));
      return (
        <recharts.BarChart
          {...props}
          responsive={false}
          width={640}
          height={132}
        />
      );
    },
  };
});

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) => key,
    i18n: { resolvedLanguage: "en", language: "en" },
  }),
}));

vi.mock("@/lib/query/usage", () => ({
  useUsageTrends: () => ({
    data: [
      {
        date: "2026-09-30T00:00:00Z",
        requestCount: 12,
        totalInputTokens: 100,
        totalOutputTokens: 50,
        totalCacheCreationTokens: 0,
        totalCacheReadTokens: 0,
        totalCost: "0.01",
      },
    ],
    isLoading: trendState.isLoading,
  }),
}));

interface ObservedResize {
  callback: ResizeObserverCallback;
  target?: Element;
  disconnect: ReturnType<typeof vi.fn>;
}

let initialWidth = 640;
const observations: ObservedResize[] = [];

beforeEach(() => {
  initialWidth = 640;
  trendState.isLoading = false;
  trendState.chartWidths.length = 0;
  observations.length = 0;
  const originalBounds = HTMLElement.prototype.getBoundingClientRect;
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(
    function (this: HTMLElement) {
      if (!this.classList.contains("h-[132px]")) {
        return originalBounds.call(this);
      }
      return {
        width: initialWidth,
        height: 132,
        x: 0,
        y: 0,
        top: 0,
        left: 0,
        right: initialWidth,
        bottom: 132,
        toJSON: () => ({}),
      } as DOMRect;
    },
  );
  vi.stubGlobal(
    "ResizeObserver",
    class {
      private observation: ObservedResize;
      constructor(callback: ResizeObserverCallback) {
        this.observation = { callback, disconnect: vi.fn() };
        observations.push(this.observation);
      }
      observe(target: Element) {
        this.observation.target = target;
      }
      unobserve() {}
      disconnect() {
        this.observation.disconnect();
      }
    },
  );
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

function chartObservation(container: HTMLElement): ObservedResize {
  const box = container.querySelector("section > div");
  const observer = observations.find((entry) => entry.target === box);
  if (!observer) throw new Error("The chart container must be observed");
  return observer;
}

function deliverWidth(observer: ObservedResize, width: number) {
  act(() => {
    observer.callback(
      [
        {
          target: observer.target,
          contentRect: { width, height: 132 },
        } as ResizeObserverEntry,
      ],
      {} as ResizeObserver,
    );
  });
}

describe("UsageTrendChart (single metric)", () => {
  it("titles the chart with the metric picked on the metric strip", () => {
    const { rerender } = render(
      <UsageTrendChart
        range={{ preset: "7d" }}
        metric="tokens"
        refreshIntervalMs={0}
      />,
    );
    // 图表自己不再有指标切换和单系列图例：指标由上方指标条选
    expect(
      screen.getByRole("heading", { name: "usage.trend.titleMetric" }),
    ).toBeInTheDocument();
    expect(screen.queryByTestId("usage-trend-legend")).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "usage.trend.cost" }),
    ).not.toBeInTheDocument();

    rerender(
      <UsageTrendChart
        range={{ preset: "7d" }}
        metric="hitRate"
        refreshIntervalMs={0}
      />,
    );
    expect(
      screen.getByRole("heading", { name: "usage.trend.titleMetric" }),
    ).toBeInTheDocument();
  });
});

describe("UsageTrendChart styles", () => {
  // 样式在用量「设置」里选，存在 settings.usageDisplay.trendChartType
  const renderWithChartType = (
    trendChartType?: string,
    metric: "cost" | "requests" | "tokens" | "hitRate" = "tokens",
  ) => {
    const props = {
      range: { preset: "7d" as const },
      metric,
      refreshIntervalMs: 0,
    };
    const client = createTestQueryClient();
    // 不让挂载时的重新拉取用 mock 后端的默认设置覆盖掉这里给的值
    client.setDefaultOptions({
      queries: { retry: false, staleTime: Infinity },
    });
    client.setQueryData(["settings"], { usageDisplay: { trendChartType } });
    return rtlRender(<UsageTrendChart {...props} />, {
      wrapper: ({ children }: { children: ReactNode }) => (
        <QueryClientProvider client={client}>{children}</QueryClientProvider>
      ),
    });
  };

  it("draws a bar chart by default without a style switch on the chart", () => {
    const { container } = renderWithChartType(undefined);
    expect(container.querySelector(".recharts-bar-rectangle")).not.toBeNull();
    expect(container.querySelector(".recharts-area")).toBeNull();
    expect(
      screen.queryByRole("button", {
        name: "usage.display.trendChartTypes.area",
      }),
    ).not.toBeInTheDocument();
  });

  it.each(["tokens", "requests", "cost", "hitRate"] as const)(
    "draws the saved area style for the %s metric",
    (metric) => {
      const { container } = renderWithChartType("area", metric);
      expect(container.querySelector(".recharts-area")).not.toBeNull();
      // A one-bucket range must still show a point in area mode.
      expect(container.querySelector(".recharts-area-dot")).not.toBeNull();
      expect(container.querySelector(".recharts-bar-rectangle")).toBeNull();
    },
  );

  it("falls back to a bar chart for an unknown saved style", () => {
    const { container } = renderWithChartType("unknown");
    expect(container.querySelector(".recharts-bar-rectangle")).not.toBeNull();
    expect(container.querySelector(".recharts-area")).toBeNull();
  });
});

describe("UsageTrendChart container width", () => {
  const chart = () => (
    <UsageTrendChart
      range={{ preset: "7d" }}
      metric="tokens"
      refreshIntervalMs={0}
    />
  );

  it("waits for a positive width and recovers after the container is shown", () => {
    initialWidth = 0;
    const { container } = render(chart());
    const observer = chartObservation(container);
    expect(trendState.chartWidths).toEqual([]);

    deliverWidth(observer, 720.4);
    expect(trendState.chartWidths[trendState.chartWidths.length - 1]).toBe(720);
    deliverWidth(observer, 0);
    expect(container.querySelector(".recharts-wrapper")).toBeNull();
    deliverWidth(observer, Number.NaN);
    expect(container.querySelector(".recharts-wrapper")).toBeNull();
    deliverWidth(observer, 480);
    expect(trendState.chartWidths[trendState.chartWidths.length - 1]).toBe(480);
    expect(trendState.chartWidths.every((width) => width > 0)).toBe(true);
  });

  it("skips duplicate widths and disconnects before late notifications", () => {
    const { container, unmount } = render(chart());
    const observer = chartObservation(container);
    const count = trendState.chartWidths.length;
    deliverWidth(observer, 640);
    expect(trendState.chartWidths).toHaveLength(count);
    deliverWidth(observer, 520);
    expect(trendState.chartWidths[trendState.chartWidths.length - 1]).toBe(520);

    unmount();
    expect(observer.disconnect).toHaveBeenCalledOnce();
    const finalCount = trendState.chartWidths.length;
    deliverWidth(observer, 700);
    expect(trendState.chartWidths).toHaveLength(finalCount);
  });

  it("measures while loading so finishing the query can render immediately", () => {
    trendState.isLoading = true;
    const { rerender } = render(chart());
    expect(trendState.chartWidths).toEqual([]);
    trendState.isLoading = false;
    rerender(chart());
    expect(trendState.chartWidths[trendState.chartWidths.length - 1]).toBe(640);
  });
});
