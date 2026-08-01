import { useState } from "react";
import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ClaudeAppendInstructionsFileField } from "@/components/providers/forms/ClaudeAppendInstructionsFileField";
import type { ClaudeAppendInstructionsConfig } from "@/lib/api/claudeAppendInstructions";

const apiMocks = vi.hoisted(() => ({
  inspect: vi.fn(),
  read: vi.fn(),
  write: vi.fn(),
  remove: vi.fn(),
}));

vi.mock("@/lib/api/claudeAppendInstructions", () => ({
  claudeAppendInstructionsApi: apiMocks,
}));

vi.mock("@/components/claude/ClaudeShellWrapperCard", () => ({
  ClaudeShellWrapperCard: () => <div data-testid="claude-shell-wrapper" />,
}));

vi.mock("@/components/ConfirmDialog", () => ({
  ConfirmDialog: ({ isOpen, title, confirmText, onConfirm }: any) =>
    isOpen ? (
      <div role="dialog" aria-label={title}>
        <button type="button" onClick={() => onConfirm(false)}>
          {confirmText}
        </button>
      </div>
    ) : null,
}));

const status = (configuredPath: string, content = "content") => ({
  configuredPath,
  resolvedPath: `/Users/test/.claude/${configuredPath.replace(/^\.\//, "")}`,
  state: content.trim() ? "valid" : "empty",
  exists: true,
  isFile: true,
  isSymlink: false,
  readable: true,
  sizeBytes: content.length,
  modifiedAt: 123,
  sha256: "a".repeat(64),
  error: null,
});

const saveButton = /保存|儲存|Save/;
const editButton = /编辑文件|編輯檔案|Edit file|ファイルを編集/;
const deleteButton = /删除文件|刪除檔案|Delete file|ファイルを削除/;

const ControlledField = ({
  initialConfig,
  onChange,
}: {
  initialConfig: ClaudeAppendInstructionsConfig;
  onChange?: (config: ClaudeAppendInstructionsConfig) => void;
}) => {
  const [config, setConfig] = useState(initialConfig);
  return (
    <ClaudeAppendInstructionsFileField
      config={config}
      onChange={(next) => {
        setConfig(next);
        onChange?.(next);
      }}
    />
  );
};

describe("ClaudeAppendInstructionsFileField", () => {
  beforeEach(() => {
    apiMocks.inspect.mockReset();
    apiMocks.inspect.mockImplementation(() => new Promise(() => undefined));
    apiMocks.read.mockReset();
    apiMocks.read.mockResolvedValue("");
    apiMocks.write.mockReset();
    apiMocks.write.mockImplementation(async (path, content) =>
      status(path, content),
    );
    apiMocks.remove.mockReset();
    apiMocks.remove.mockResolvedValue(true);
  });

  it("adds a file to the current provider form", async () => {
    const onChange = vi.fn();
    render(
      <ControlledField
        initialConfig={{ files: [], activeFile: null }}
        onChange={onChange}
      />,
    );
    screen.getByText(
      /暂无 Claude 追加指令文件|No Claude append instruction files/,
    );

    fireEvent.click(
      screen.getByRole("button", {
        name: /新建文件|新增檔案|New file|新規ファイル/,
      }),
    );
    fireEvent.change(
      screen.getByLabelText(/文件路径|檔案路徑|File path|ファイルパス/),
      { target: { value: "./separate.md" } },
    );
    fireEvent.change(
      screen.getByLabelText(/文件内容|檔案內容|File content|ファイル内容/),
      { target: { value: "Separate Claude instructions.\n" } },
    );
    fireEvent.click(screen.getByRole("button", { name: saveButton }));

    await waitFor(() => {
      expect(apiMocks.write).toHaveBeenCalledWith(
        "./separate.md",
        "Separate Claude instructions.\n",
      );
      expect(onChange).toHaveBeenCalledWith({
        files: ["./separate.md"],
        activeFile: null,
      });
    });
  });

  it("switches the active file only in the current provider form", async () => {
    const onChange = vi.fn();
    render(
      <ControlledField
        initialConfig={{
          files: ["./first.md", "./second.md"],
          activeFile: "./first.md",
        }}
        onChange={onChange}
      />,
    );

    const switches = await screen.findAllByRole("switch");
    fireEvent.click(switches[1]);

    expect(onChange).toHaveBeenCalledWith({
      files: ["./first.md", "./second.md"],
      activeFile: "./second.md",
    });
  });

  it("renders each provider's own file list", () => {
    const onChange = vi.fn();
    const { rerender } = render(
      <ClaudeAppendInstructionsFileField
        config={{ files: ["./provider-a.md"], activeFile: "./provider-a.md" }}
        onChange={onChange}
      />,
    );
    expect(screen.getByText("provider-a.md")).toBeInTheDocument();

    rerender(
      <ClaudeAppendInstructionsFileField
        config={{ files: ["./provider-b.md"], activeFile: null }}
        onChange={onChange}
      />,
    );
    expect(screen.getByText("provider-b.md")).toBeInTheDocument();
    expect(screen.queryByText("provider-a.md")).not.toBeInTheDocument();
  });

  it("does not overwrite a file when reading the original fails", async () => {
    apiMocks.read.mockRejectedValue(new Error("permission denied"));
    render(
      <ControlledField
        initialConfig={{ files: ["./locked.md"], activeFile: null }}
      />,
    );

    fireEvent.click(await screen.findByTitle(editButton));
    await screen.findByText(/无法安全保存|cannot be safely overwritten/);
    expect(screen.getByRole("button", { name: saveButton })).toBeDisabled();
    expect(apiMocks.write).not.toHaveBeenCalled();
  });

  it("keeps the provider entry when deleting the file fails", async () => {
    const onChange = vi.fn();
    apiMocks.remove.mockRejectedValue(new Error("permission denied"));
    render(
      <ControlledField
        initialConfig={{
          files: ["./active.md"],
          activeFile: "./active.md",
        }}
        onChange={onChange}
      />,
    );

    fireEvent.click(await screen.findByTitle(deleteButton));
    fireEvent.click(
      within(screen.getByRole("dialog")).getByRole("button", {
        name: deleteButton,
      }),
    );

    await waitFor(() => {
      expect(apiMocks.remove).toHaveBeenCalledWith("./active.md");
    });
    expect(screen.getByText("active.md")).toBeInTheDocument();
    expect(onChange).not.toHaveBeenCalled();
  });
});
