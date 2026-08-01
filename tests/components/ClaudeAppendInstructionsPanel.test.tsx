import { createRef } from "react";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import ClaudeAppendInstructionsPanel, {
  type ClaudeAppendInstructionsPanelHandle,
} from "@/components/claude/ClaudeAppendInstructionsPanel";

const apiMocks = vi.hoisted(() => ({
  getConfig: vi.fn(),
  setConfig: vi.fn(),
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

describe("ClaudeAppendInstructionsPanel", () => {
  beforeEach(() => {
    apiMocks.getConfig.mockReset();
    apiMocks.getConfig.mockResolvedValue({ files: [], activeFile: null });
    apiMocks.setConfig.mockReset();
    apiMocks.setConfig.mockImplementation(async (config) => config);
    apiMocks.inspect.mockReset();
    apiMocks.inspect.mockImplementation(async (path) => status(path));
    apiMocks.read.mockReset();
    apiMocks.read.mockResolvedValue("");
    apiMocks.write.mockReset();
    apiMocks.write.mockImplementation(async (path, content) =>
      status(path, content),
    );
    apiMocks.remove.mockReset();
    apiMocks.remove.mockResolvedValue(true);
  });

  it("creates a file in the independent Claude instruction list", async () => {
    const ref = createRef<ClaudeAppendInstructionsPanelHandle>();
    render(<ClaudeAppendInstructionsPanel ref={ref} open />);
    await screen.findByText(
      /暂无 Claude 追加指令文件|No Claude append instruction files/,
    );

    act(() => ref.current?.openAdd());
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
      expect(apiMocks.setConfig).toHaveBeenCalledWith({
        files: ["./separate.md"],
        activeFile: null,
      });
    });
  });

  it("switches the active file through the independent config API", async () => {
    apiMocks.getConfig.mockResolvedValue({
      files: ["./first.md", "./second.md"],
      activeFile: "./first.md",
    });
    render(<ClaudeAppendInstructionsPanel open />);

    const switches = await screen.findAllByRole("switch");
    fireEvent.click(switches[1]);

    await waitFor(() => {
      expect(apiMocks.setConfig).toHaveBeenCalledWith({
        files: ["./first.md", "./second.md"],
        activeFile: "./second.md",
      });
    });
  });

  it("does not overwrite a file when reading the original fails", async () => {
    apiMocks.getConfig.mockResolvedValue({
      files: ["./locked.md"],
      activeFile: null,
    });
    apiMocks.read.mockRejectedValue(new Error("permission denied"));
    render(<ClaudeAppendInstructionsPanel open />);

    fireEvent.click(await screen.findByTitle(editButton));
    await screen.findByText(/无法安全保存|cannot be safely overwritten/);
    expect(screen.getByRole("button", { name: saveButton })).toBeDisabled();
    expect(apiMocks.write).not.toHaveBeenCalled();
  });

  it("keeps the independent entry when deleting the file fails", async () => {
    apiMocks.getConfig.mockResolvedValue({
      files: ["./active.md"],
      activeFile: "./active.md",
    });
    apiMocks.remove.mockRejectedValue(new Error("permission denied"));
    render(<ClaudeAppendInstructionsPanel open />);

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
    expect(apiMocks.setConfig).not.toHaveBeenCalled();
  });
});
