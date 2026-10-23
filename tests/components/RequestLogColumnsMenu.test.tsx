import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { RequestLogColumnsMenu } from "@/components/usage/RequestLogColumnsMenu";
import type { UsageDisplaySettings } from "@/types";

function deferredSaves() {
  const saves: {
    resolve: (saved: boolean) => void;
    reject: (error: Error) => void;
  }[] = [];
  const onChange = vi.fn(
    (_next: UsageDisplaySettings) =>
      new Promise<boolean>((resolve, reject) =>
        saves.push({ resolve, reject }),
      ),
  );
  return { saves, onChange };
}

// 「显示的列」在请求日志的表格工具栏里
describe("RequestLogColumnsMenu", () => {
  it("shows and hides request log columns", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn().mockResolvedValue(true);
    render(
      <RequestLogColumnsMenu
        value={{ currency: "CNY", requestLogColumns: { app: false } }}
        onChange={onChange}
      />,
    );

    await user.click(
      screen.getByRole("button", { name: "usage.display.logColumns" }),
    );
    expect(
      screen.queryByRole("menuitemcheckbox", { name: "usage.time" }),
    ).not.toBeInTheDocument();
    const status = await screen.findByRole("menuitemcheckbox", {
      name: "usage.statusCode",
    });
    expect(status).toHaveAttribute("aria-checked", "false");
    await user.click(status);

    // 菜单不关，可以接着勾；保存时其它显示设置原样带上，只存改过的列
    expect(onChange).toHaveBeenLastCalledWith({
      currency: "CNY",
      requestLogColumns: { app: false, statusCode: true },
    });
    expect(
      screen.getByRole("menuitemcheckbox", { name: "usage.statusCode" }),
    ).toHaveAttribute("aria-checked", "true");

    // 勾回默认值的列不再写进设置
    await user.click(
      screen.getByRole("menuitemcheckbox", { name: "usage.app" }),
    );
    expect(onChange).toHaveBeenLastCalledWith({
      currency: "CNY",
      requestLogColumns: { statusCode: true },
    });

    await user.click(
      screen.getByRole("menuitem", { name: "usage.logColumns.reset" }),
    );
    expect(onChange).toHaveBeenLastCalledWith({
      currency: "CNY",
      requestLogColumns: {},
    });
  });

  it("keeps later toggles while an earlier save is still in flight", async () => {
    const user = userEvent.setup();
    const saves: ((ok: boolean) => void)[] = [];
    const onChange = vi.fn(
      () => new Promise<boolean>((resolve) => saves.push(resolve)),
    );
    const { rerender } = render(
      <RequestLogColumnsMenu value={{ currency: "CNY" }} onChange={onChange} />,
    );
    await user.click(
      screen.getByRole("button", { name: "usage.display.logColumns" }),
    );
    const item = (name: string) =>
      screen.getByRole("menuitemcheckbox", { name });
    await user.click(
      await screen.findByRole("menuitemcheckbox", {
        name: "usage.logColumns.duration",
      }),
    );
    await user.click(item("usage.statusCode"));
    // 第二次保存必须等第一次完全结束，后端旧写就没有机会晚到覆盖新写。
    expect(onChange).toHaveBeenCalledTimes(1);

    // 第一次保存先回来、设置读回只含第一列：后勾的列不能被冲掉
    rerender(
      <RequestLogColumnsMenu
        value={{ currency: "CNY", requestLogColumns: { duration: true } }}
        onChange={onChange}
      />,
    );
    await act(async () => saves[0](true));
    expect(onChange).toHaveBeenCalledTimes(2);
    expect(item("usage.statusCode")).toHaveAttribute("aria-checked", "true");

    // 最后一次保存失败：退回到设置里实际存的
    await act(async () => saves[1](false));
    expect(item("usage.statusCode")).toHaveAttribute("aria-checked", "false");
    expect(item("usage.logColumns.duration")).toHaveAttribute(
      "aria-checked",
      "true",
    );
  });

  it("serializes writes and coalesces pending changes into the latest complete snapshot", async () => {
    const user = userEvent.setup();
    const { saves, onChange } = deferredSaves();
    const { rerender } = render(
      <RequestLogColumnsMenu value={{ currency: "CNY" }} onChange={onChange} />,
    );
    await user.click(
      screen.getByRole("button", { name: "usage.display.logColumns" }),
    );
    const item = (name: string) =>
      screen.getByRole("menuitemcheckbox", { name });
    await user.click(item("usage.logColumns.duration"));
    await user.click(item("usage.statusCode"));
    await user.click(item("usage.provider"));
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(item("usage.statusCode")).toHaveAttribute("aria-checked", "true");
    expect(item("usage.provider")).toHaveAttribute("aria-checked", "false");

    // 保存间隙读回了新币种和第一份列设置：下一次写保留最新的其它设置，
    // 但列仍然包含排队期间的全部点击，而不是读回的中间快照。
    rerender(
      <RequestLogColumnsMenu
        value={{ currency: "JPY", requestLogColumns: { duration: true } }}
        onChange={onChange}
      />,
    );
    await act(async () => saves[0].resolve(true));
    expect(onChange).toHaveBeenCalledTimes(2);
    expect(onChange).toHaveBeenLastCalledWith({
      currency: "JPY",
      requestLogColumns: { duration: true, statusCode: true, provider: false },
    });
    await act(async () => saves[1].resolve(true));
    expect(onChange).toHaveBeenCalledTimes(2);
    expect(item("usage.logColumns.duration")).toHaveAttribute(
      "aria-checked",
      "true",
    );
    expect(item("usage.statusCode")).toHaveAttribute("aria-checked", "true");
    expect(item("usage.provider")).toHaveAttribute("aria-checked", "false");
  });

  it("continues with later choices when an earlier save rejects", async () => {
    const user = userEvent.setup();
    const { saves, onChange } = deferredSaves();
    render(<RequestLogColumnsMenu value={undefined} onChange={onChange} />);
    await user.click(
      screen.getByRole("button", { name: "usage.display.logColumns" }),
    );
    await user.click(
      screen.getByRole("menuitemcheckbox", {
        name: "usage.logColumns.duration",
      }),
    );
    await user.click(
      screen.getByRole("menuitemcheckbox", { name: "usage.statusCode" }),
    );
    await act(async () => saves[0].reject(new Error("save failed")));
    expect(onChange).toHaveBeenCalledTimes(2);
    expect(onChange).toHaveBeenLastCalledWith({
      requestLogColumns: { duration: true, statusCode: true },
    });
    await act(async () => saves[1].resolve(true));
    expect(
      screen.getByRole("menuitemcheckbox", { name: "usage.statusCode" }),
    ).toHaveAttribute("aria-checked", "true");
  });

  it.each(["false", "reject"] as const)(
    "rolls the latest failed save back to the last success without waiting for props (%s)",
    async (failure) => {
      const user = userEvent.setup();
      const { saves, onChange } = deferredSaves();
      render(<RequestLogColumnsMenu value={undefined} onChange={onChange} />);
      await user.click(
        screen.getByRole("button", { name: "usage.display.logColumns" }),
      );
      const item = (name: string) =>
        screen.getByRole("menuitemcheckbox", { name });
      await user.click(item("usage.logColumns.duration"));
      await user.click(item("usage.statusCode"));
      await act(async () => saves[0].resolve(true));
      await act(async () => {
        if (failure === "reject") {
          saves[1].reject(new Error("save failed"));
        } else {
          saves[1].resolve(false);
        }
      });
      expect(item("usage.logColumns.duration")).toHaveAttribute(
        "aria-checked",
        "true",
      );
      expect(item("usage.statusCode")).toHaveAttribute("aria-checked", "false");

      // 失败不会卡住工作循环，下一次点击也不能复活刚回滚掉的选择。
      await user.click(item("usage.provider"));
      expect(onChange).toHaveBeenCalledTimes(3);
      expect(onChange).toHaveBeenLastCalledWith({
        requestLogColumns: { duration: true, provider: false },
      });
      await act(async () => saves[2].resolve(true));
    },
  );

  it("does not replace the last success with an older props snapshot during a later save", async () => {
    const user = userEvent.setup();
    const { saves, onChange } = deferredSaves();
    const { rerender } = render(
      <RequestLogColumnsMenu value={undefined} onChange={onChange} />,
    );
    await user.click(
      screen.getByRole("button", { name: "usage.display.logColumns" }),
    );
    const item = (name: string) =>
      screen.getByRole("menuitemcheckbox", { name });
    await user.click(item("usage.logColumns.duration"));
    await user.click(item("usage.statusCode"));
    await act(async () => saves[0].resolve(true));
    await user.click(item("usage.provider"));
    await act(async () => saves[1].resolve(true));
    expect(onChange).toHaveBeenCalledTimes(3);

    // 前两次已成功，第三次还在保存时第一份 props 才交付。
    rerender(
      <RequestLogColumnsMenu
        value={{ requestLogColumns: { duration: true } }}
        onChange={onChange}
      />,
    );
    await act(async () => saves[2].resolve(false));
    expect(item("usage.logColumns.duration")).toHaveAttribute(
      "aria-checked",
      "true",
    );
    expect(item("usage.statusCode")).toHaveAttribute("aria-checked", "true");
    expect(item("usage.provider")).toHaveAttribute("aria-checked", "true");
  });

  it("does not start queued writes after unmount", async () => {
    const user = userEvent.setup();
    const { saves, onChange } = deferredSaves();
    const { unmount } = render(
      <RequestLogColumnsMenu value={undefined} onChange={onChange} />,
    );
    await user.click(
      screen.getByRole("button", { name: "usage.display.logColumns" }),
    );
    await user.click(
      screen.getByRole("menuitemcheckbox", {
        name: "usage.logColumns.duration",
      }),
    );
    await user.click(
      screen.getByRole("menuitemcheckbox", { name: "usage.statusCode" }),
    );
    expect(onChange).toHaveBeenCalledTimes(1);
    unmount();
    await act(async () => saves[0].resolve(true));
    expect(onChange).toHaveBeenCalledTimes(1);
  });
});
