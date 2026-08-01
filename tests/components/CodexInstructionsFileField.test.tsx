import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { CodexInstructionsFileField } from "@/components/providers/forms/CodexInstructionsFileField";

const dialogMocks = vi.hoisted(() => ({
  open: vi.fn(),
}));

const instructionsApiMocks = vi.hoisted(() => ({
  inspect: vi.fn(),
  read: vi.fn(),
  write: vi.fn(),
  remove: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: dialogMocks.open,
}));

vi.mock("@/lib/api/codexInstructions", () => ({
  codexInstructionsApi: {
    inspect: instructionsApiMocks.inspect,
    read: instructionsApiMocks.read,
    write: instructionsApiMocks.write,
    remove: instructionsApiMocks.remove,
  },
}));

const addButtonName = /新建文件|新建檔案|New file|新規ファイル/;
const saveButtonName = /保存|儲存|Save/;
const pathLabel = /文件路径|檔案路徑|File path|ファイルパス/;
const contentLabel = /文件内容|檔案內容|File content|ファイル内容/;
const editButtonName = /编辑文件|編輯檔案|Edit file|ファイルを編集/;
const deleteButtonName = /删除文件|刪除檔案|Delete file|ファイルを削除/;

const fileStatus = (configuredPath: string, content = "content") => ({
  configuredPath,
  resolvedPath: `/Users/test/.codex/${configuredPath.replace(/^\.\//, "")}`,
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

describe("CodexInstructionsFileField", () => {
  beforeEach(() => {
    dialogMocks.open.mockReset();
    instructionsApiMocks.inspect.mockReset();
    instructionsApiMocks.inspect.mockImplementation(
      () => new Promise(() => undefined),
    );
    instructionsApiMocks.read.mockReset();
    instructionsApiMocks.read.mockResolvedValue("");
    instructionsApiMocks.write.mockReset();
    instructionsApiMocks.write.mockImplementation(
      async (configuredPath: string, content: string) =>
        fileStatus(configuredPath, content),
    );
    instructionsApiMocks.remove.mockReset();
    instructionsApiMocks.remove.mockResolvedValue(true);
  });

  it("checks saved paths and shows the resolved file state", async () => {
    instructionsApiMocks.inspect.mockResolvedValue({
      configuredPath: "./missing.md",
      resolvedPath: "/Users/test/.codex/missing.md",
      state: "missing",
      exists: false,
      isFile: false,
      isSymlink: false,
      readable: false,
      sizeBytes: null,
      modifiedAt: null,
      sha256: null,
      error: "No such file or directory",
    });

    render(
      <CodexInstructionsFileField
        enabled={false}
        path=""
        savedFiles={["./missing.md"]}
        onActiveFileChange={vi.fn()}
        onSavedFilesChange={vi.fn()}
      />,
    );

    await waitFor(() => {
      expect(instructionsApiMocks.inspect).toHaveBeenCalledWith("./missing.md");
      expect(
        screen.getByText(
          /文件不存在|檔案不存在|File not found|ファイルが見つかりません/,
        ),
      ).toBeInTheDocument();
    });
  });

  it("shows saved files and switches the single active file", () => {
    const onActiveFileChange = vi.fn();
    render(
      <CodexInstructionsFileField
        enabled={true}
        path="./instruction_5.6.md"
        savedFiles={["./instruction_5.6.md", "./instruction_default.md"]}
        onActiveFileChange={onActiveFileChange}
        onSavedFilesChange={vi.fn()}
      />,
    );

    const switches = screen.getAllByRole("switch");
    expect(switches).toHaveLength(2);
    expect(switches[0]).toBeChecked();
    expect(switches[1]).not.toBeChecked();

    fireEvent.click(switches[1]);
    expect(onActiveFileChange).toHaveBeenCalledWith("./instruction_default.md");
  });

  it("disables the current file without deleting it", () => {
    const onActiveFileChange = vi.fn();
    const onSavedFilesChange = vi.fn();
    render(
      <CodexInstructionsFileField
        enabled={true}
        path="./instruction_5.6.md"
        savedFiles={["./instruction_5.6.md"]}
        onActiveFileChange={onActiveFileChange}
        onSavedFilesChange={onSavedFilesChange}
      />,
    );

    fireEvent.click(screen.getByRole("switch"));
    expect(onActiveFileChange).toHaveBeenCalledWith(null);
    expect(onSavedFilesChange).not.toHaveBeenCalled();
  });

  it("creates a manually entered file without enabling it", async () => {
    const onActiveFileChange = vi.fn();
    const onSavedFilesChange = vi.fn();
    render(
      <CodexInstructionsFileField
        enabled={false}
        path=""
        savedFiles={["./default.md"]}
        onActiveFileChange={onActiveFileChange}
        onSavedFilesChange={onSavedFilesChange}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: addButtonName }));
    fireEvent.change(screen.getByLabelText(pathLabel), {
      target: { value: "./instruction_5.6.md" },
    });
    fireEvent.change(screen.getByLabelText(contentLabel), {
      target: { value: "Use concise answers.\n" },
    });
    fireEvent.click(screen.getByRole("button", { name: saveButtonName }));

    await waitFor(() => {
      expect(instructionsApiMocks.write).toHaveBeenCalledWith(
        "./instruction_5.6.md",
        "Use concise answers.\n",
      );
      expect(onSavedFilesChange).toHaveBeenCalledWith([
        "./default.md",
        "./instruction_5.6.md",
      ]);
    });
    expect(onActiveFileChange).not.toHaveBeenCalled();
  });

  it("adds a file selected from the native dialog", async () => {
    dialogMocks.open.mockResolvedValue("/tmp/instruction_5.6.md");
    instructionsApiMocks.read.mockResolvedValue("Selected instructions.\n");
    const onSavedFilesChange = vi.fn();
    render(
      <CodexInstructionsFileField
        enabled={false}
        path=""
        savedFiles={[]}
        onActiveFileChange={vi.fn()}
        onSavedFilesChange={onSavedFilesChange}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: addButtonName }));
    fireEvent.click(
      screen.getByTitle(
        /选择 Markdown 或文本文件|選擇 Markdown 或文字檔案|Choose a Markdown or text file|Markdown またはテキストファイルを選択/,
      ),
    );

    await waitFor(() => {
      expect(screen.getByLabelText(pathLabel)).toHaveValue(
        "/tmp/instruction_5.6.md",
      );
      expect(screen.getByLabelText(contentLabel)).toHaveValue(
        "Selected instructions.\n",
      );
    });
    fireEvent.click(screen.getByRole("button", { name: saveButtonName }));
    await waitFor(() => {
      expect(instructionsApiMocks.write).toHaveBeenCalledWith(
        "/tmp/instruction_5.6.md",
        "Selected instructions.\n",
      );
      expect(onSavedFilesChange).toHaveBeenCalledWith([
        "/tmp/instruction_5.6.md",
      ]);
    });
  });

  it("loads and updates the original content in place", async () => {
    instructionsApiMocks.read.mockResolvedValue("Old instructions.\n");
    const onActiveFileChange = vi.fn();
    const onSavedFilesChange = vi.fn();
    render(
      <CodexInstructionsFileField
        enabled={true}
        path="./old.md"
        savedFiles={["./old.md", "./other.md"]}
        onActiveFileChange={onActiveFileChange}
        onSavedFilesChange={onSavedFilesChange}
      />,
    );

    fireEvent.click(screen.getAllByTitle(editButtonName)[0]);
    await waitFor(() => {
      expect(instructionsApiMocks.read).toHaveBeenCalledWith("./old.md");
      expect(screen.getByLabelText(contentLabel)).toHaveValue(
        "Old instructions.\n",
      );
    });
    expect(screen.getByLabelText(pathLabel)).toBeDisabled();
    fireEvent.change(screen.getByLabelText(contentLabel), {
      target: { value: "New instructions.\n" },
    });
    fireEvent.click(screen.getByRole("button", { name: saveButtonName }));

    await waitFor(() => {
      expect(instructionsApiMocks.write).toHaveBeenCalledWith(
        "./old.md",
        "New instructions.\n",
      );
    });
    expect(onSavedFilesChange).not.toHaveBeenCalled();
    expect(onActiveFileChange).not.toHaveBeenCalled();
  });

  it("does not overwrite an existing file when reading it fails", async () => {
    instructionsApiMocks.read.mockRejectedValue(new Error("permission denied"));
    render(
      <CodexInstructionsFileField
        enabled={false}
        path=""
        savedFiles={["./locked.md"]}
        onActiveFileChange={vi.fn()}
        onSavedFilesChange={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByTitle(editButtonName));
    await waitFor(() => {
      expect(instructionsApiMocks.read).toHaveBeenCalledWith("./locked.md");
      expect(
        screen.getByText(
          /无法安全保存|無法安全儲存|cannot be saved safely|安全に保存できません/,
        ),
      ).toBeInTheDocument();
    });

    expect(screen.getByRole("button", { name: saveButtonName })).toBeDisabled();
    expect(instructionsApiMocks.write).not.toHaveBeenCalled();
  });

  it("deletes the original file before disabling and removing it", async () => {
    const onActiveFileChange = vi.fn();
    const onSavedFilesChange = vi.fn();
    render(
      <CodexInstructionsFileField
        enabled={true}
        path="./active.md"
        savedFiles={["./active.md", "./other.md"]}
        onActiveFileChange={onActiveFileChange}
        onSavedFilesChange={onSavedFilesChange}
      />,
    );

    fireEvent.click(screen.getAllByTitle(deleteButtonName)[0]);
    expect(instructionsApiMocks.remove).not.toHaveBeenCalled();

    const confirmDialog = screen.getByRole("dialog");
    fireEvent.click(
      within(confirmDialog).getByRole("button", { name: deleteButtonName }),
    );

    await waitFor(() => {
      expect(instructionsApiMocks.remove).toHaveBeenCalledWith("./active.md");
      expect(onActiveFileChange).toHaveBeenCalledWith(null);
      expect(onSavedFilesChange).toHaveBeenCalledWith(["./other.md"]);
    });
  });

  it("keeps the provider entry when deleting the original file fails", async () => {
    instructionsApiMocks.remove.mockRejectedValue(
      new Error("permission denied"),
    );
    const onActiveFileChange = vi.fn();
    const onSavedFilesChange = vi.fn();
    render(
      <CodexInstructionsFileField
        enabled={true}
        path="./active.md"
        savedFiles={["./active.md"]}
        onActiveFileChange={onActiveFileChange}
        onSavedFilesChange={onSavedFilesChange}
      />,
    );

    fireEvent.click(screen.getByTitle(deleteButtonName));
    fireEvent.click(
      within(screen.getByRole("dialog")).getByRole("button", {
        name: deleteButtonName,
      }),
    );

    await waitFor(() => {
      expect(instructionsApiMocks.remove).toHaveBeenCalledWith("./active.md");
    });
    expect(onActiveFileChange).not.toHaveBeenCalled();
    expect(onSavedFilesChange).not.toHaveBeenCalled();
  });
});
