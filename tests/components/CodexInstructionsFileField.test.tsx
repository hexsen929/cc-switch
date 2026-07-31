import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { CodexInstructionsFileField } from "@/components/providers/forms/CodexInstructionsFileField";

const dialogMocks = vi.hoisted(() => ({
  open: vi.fn(),
}));

const instructionsApiMocks = vi.hoisted(() => ({
  inspect: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: dialogMocks.open,
}));

vi.mock("@/lib/api/codexInstructions", () => ({
  codexInstructionsApi: {
    inspect: instructionsApiMocks.inspect,
  },
}));

const addButtonName = /添加文件|新增檔案|Add file|ファイルを追加/;
const saveButtonName = /保存|儲存|Save/;
const pathLabel = /文件路径|檔案路徑|File path|ファイルパス/;

describe("CodexInstructionsFileField", () => {
  beforeEach(() => {
    dialogMocks.open.mockReset();
    instructionsApiMocks.inspect.mockReset();
    instructionsApiMocks.inspect.mockImplementation(
      () => new Promise(() => undefined),
    );
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

  it("adds a manually entered path without enabling it", () => {
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
    fireEvent.click(screen.getByRole("button", { name: saveButtonName }));

    expect(onSavedFilesChange).toHaveBeenCalledWith([
      "./default.md",
      "./instruction_5.6.md",
    ]);
    expect(onActiveFileChange).not.toHaveBeenCalled();
  });

  it("adds a file selected from the native dialog", async () => {
    dialogMocks.open.mockResolvedValue("/tmp/instruction_5.6.md");
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
    });
    fireEvent.click(screen.getByRole("button", { name: saveButtonName }));
    expect(onSavedFilesChange).toHaveBeenCalledWith([
      "/tmp/instruction_5.6.md",
    ]);
  });

  it("updates config when editing the active file", () => {
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

    fireEvent.click(
      screen.getAllByTitle(
        /编辑文件路径|編輯檔案路徑|Edit file path|ファイルパスを編集/,
      )[0],
    );
    fireEvent.change(screen.getByLabelText(pathLabel), {
      target: { value: "./new.md" },
    });
    fireEvent.click(screen.getByRole("button", { name: saveButtonName }));

    expect(onSavedFilesChange).toHaveBeenCalledWith(["./new.md", "./other.md"]);
    expect(onActiveFileChange).toHaveBeenCalledWith("./new.md");
  });

  it("disables and removes the active file", () => {
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

    fireEvent.click(
      screen.getAllByTitle(
        /从列表移除|從清單移除|Remove from list|一覧から削除/,
      )[0],
    );
    expect(onActiveFileChange).toHaveBeenCalledWith(null);
    expect(onSavedFilesChange).toHaveBeenCalledWith(["./other.md"]);
  });
});
