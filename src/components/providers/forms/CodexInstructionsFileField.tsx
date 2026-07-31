import { useCallback, useEffect, useMemo, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  CircleCheck,
  CircleX,
  FileText,
  FolderOpen,
  Loader2,
  Pencil,
  Plus,
  Trash2,
  TriangleAlert,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import {
  codexInstructionsApi,
  type CodexInstructionsFileState,
  type CodexInstructionsFileStatus,
} from "@/lib/api/codexInstructions";
import { normalizeCodexModelInstructionsFiles } from "@/utils/providerConfigUtils";

interface CodexInstructionsFileFieldProps {
  enabled: boolean;
  path: string;
  savedFiles: string[];
  onActiveFileChange: (path: string | null) => void;
  onSavedFilesChange: (files: string[]) => void;
}

interface FileInspection {
  loading: boolean;
  status?: CodexInstructionsFileStatus;
  error?: string;
}

const instructionFileName = (path: string): string => {
  const normalized = path.replace(/\\/g, "/").replace(/\/+$/, "");
  return normalized.split("/").pop() || path;
};

export function CodexInstructionsFileField({
  enabled,
  path,
  savedFiles,
  onActiveFileChange,
  onSavedFilesChange,
}: CodexInstructionsFileFieldProps) {
  const { t } = useTranslation();
  const [editorOpen, setEditorOpen] = useState(false);
  const [editingPath, setEditingPath] = useState<string | null>(null);
  const [draftPath, setDraftPath] = useState("");
  const [fileInspections, setFileInspections] = useState<
    Record<string, FileInspection>
  >({});

  const normalizedFiles = useMemo(
    () => normalizeCodexModelInstructionsFiles(savedFiles),
    [savedFiles],
  );
  const activePath = enabled ? path.trim() : "";
  const normalizedDraftPath =
    normalizeCodexModelInstructionsFiles([], draftPath)[0] || "";
  const duplicatePath = normalizedFiles.some(
    (file) => file === normalizedDraftPath && file !== editingPath,
  );

  useEffect(() => {
    let cancelled = false;
    setFileInspections(
      Object.fromEntries(
        normalizedFiles.map((file) => [file, { loading: true }]),
      ),
    );

    for (const file of normalizedFiles) {
      void codexInstructionsApi
        .inspect(file)
        .then((status) => {
          if (cancelled) return;
          setFileInspections((current) => ({
            ...current,
            [file]: { loading: false, status },
          }));
        })
        .catch((error) => {
          if (cancelled) return;
          setFileInspections((current) => ({
            ...current,
            [file]: { loading: false, error: String(error) },
          }));
        });
    }

    return () => {
      cancelled = true;
    };
  }, [normalizedFiles]);

  const inspectionStateLabel = useCallback(
    (state: CodexInstructionsFileState): string => {
      switch (state) {
        case "valid":
          return t("codexConfig.instructionsFileStateValid", {
            defaultValue: "文件可用",
          });
        case "missing":
          return t("codexConfig.instructionsFileStateMissing", {
            defaultValue: "文件不存在",
          });
        case "notFile":
          return t("codexConfig.instructionsFileStateNotFile", {
            defaultValue: "路径不是文件",
          });
        case "unreadable":
          return t("codexConfig.instructionsFileStateUnreadable", {
            defaultValue: "文件不可读",
          });
        case "empty":
          return t("codexConfig.instructionsFileStateEmpty", {
            defaultValue: "文件内容为空",
          });
        case "invalid":
          return t("codexConfig.instructionsFileStateInvalid", {
            defaultValue: "文件内容无效",
          });
      }
    },
    [t],
  );

  const closeEditor = useCallback(() => {
    setEditorOpen(false);
    setEditingPath(null);
    setDraftPath("");
  }, []);

  const openAddEditor = useCallback(() => {
    setEditingPath(null);
    setDraftPath("");
    setEditorOpen(true);
  }, []);

  const openEditEditor = useCallback((file: string) => {
    setEditingPath(file);
    setDraftPath(file);
    setEditorOpen(true);
  }, []);

  const handleBrowse = useCallback(async () => {
    try {
      const selected = await open({
        multiple: false,
        directory: false,
        filters: [
          {
            name: "Markdown / Text",
            extensions: ["md", "txt"],
          },
        ],
      });
      const selectedPath = Array.isArray(selected) ? selected[0] : selected;
      if (selectedPath) setDraftPath(selectedPath);
    } catch (error) {
      toast.error(
        t("codexConfig.instructionsFileBrowseFailed", {
          defaultValue: "无法选择模型指令文件：{{error}}",
          error: String(error),
        }),
      );
    }
  }, [t]);

  const handleSave = useCallback(() => {
    if (!normalizedDraftPath || duplicatePath) return;

    const nextFiles = editingPath
      ? normalizeCodexModelInstructionsFiles(
          normalizedFiles.map((file) =>
            file === editingPath ? normalizedDraftPath : file,
          ),
        )
      : normalizeCodexModelInstructionsFiles(
          normalizedFiles,
          normalizedDraftPath,
        );
    onSavedFilesChange(nextFiles);

    if (editingPath && editingPath === activePath) {
      onActiveFileChange(normalizedDraftPath);
    }
    closeEditor();
  }, [
    activePath,
    closeEditor,
    duplicatePath,
    editingPath,
    normalizedDraftPath,
    normalizedFiles,
    onActiveFileChange,
    onSavedFilesChange,
  ]);

  const handleRemove = useCallback(
    (file: string) => {
      if (file === activePath) onActiveFileChange(null);
      onSavedFilesChange(normalizedFiles.filter((item) => item !== file));
    },
    [activePath, normalizedFiles, onActiveFileChange, onSavedFilesChange],
  );

  const activeFileName = activePath ? instructionFileName(activePath) : "";

  return (
    <div className="space-y-3 border-t border-border-default pt-3">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0 space-y-1">
          <Label>
            {t("codexConfig.instructionsFileLabel", {
              defaultValue: "模型指令文件",
            })}
          </Label>
          <p className="text-xs leading-relaxed text-muted-foreground">
            {t("codexConfig.instructionsFileHint", {
              defaultValue:
                "启用后替换 Codex 为所选模型提供的内置基础指令。仅在自定义模型需要专用指令时使用。",
            })}
          </p>
        </div>
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="shrink-0 gap-1.5"
          onClick={openAddEditor}
        >
          <Plus className="h-4 w-4" />
          {t("codexConfig.instructionsFileAdd", {
            defaultValue: "添加文件",
          })}
        </Button>
      </div>

      <div className="rounded-md border border-border-default bg-muted/30 px-3 py-2 text-xs text-muted-foreground">
        {t("codexConfig.instructionsFileCount", {
          defaultValue: "共 {{count}} 个文件",
          count: normalizedFiles.length,
        })}
        {" · "}
        {activeFileName
          ? t("codexConfig.instructionsFileEnabledName", {
              defaultValue: "已启用：{{name}}",
              name: activeFileName,
            })
          : t("codexConfig.instructionsFileNoneEnabled", {
              defaultValue: "未启用任何文件",
            })}
      </div>

      {normalizedFiles.length === 0 ? (
        <div className="flex min-h-20 items-center justify-center rounded-md border border-dashed border-border-default px-4 text-center text-sm text-muted-foreground">
          {t("codexConfig.instructionsFileEmpty", {
            defaultValue: "暂无模型指令文件",
          })}
        </div>
      ) : (
        <div className="space-y-2">
          {normalizedFiles.map((file) => {
            const isActive = file === activePath;
            const name = instructionFileName(file);
            const inspection = fileInspections[file];
            const status = inspection?.status;
            const stateLabel = status ? inspectionStateLabel(status.state) : "";
            const statusTitle = status
              ? [
                  status.resolvedPath,
                  status.sha256 ? `SHA-256: ${status.sha256}` : null,
                  status.error,
                ]
                  .filter(Boolean)
                  .join("\n")
              : inspection?.error;
            const statusClassName = status
              ? status.state === "valid"
                ? "text-green-600 dark:text-green-400"
                : status.state === "missing" ||
                    status.state === "notFile" ||
                    status.state === "empty"
                  ? "text-amber-600 dark:text-amber-400"
                  : "text-destructive"
              : inspection?.error
                ? "text-destructive"
                : "text-muted-foreground";
            return (
              <div
                key={file}
                className="flex min-h-16 items-center gap-3 rounded-md border border-border-default bg-muted/20 px-3 py-2.5"
              >
                <Switch
                  checked={isActive}
                  onCheckedChange={(checked) =>
                    onActiveFileChange(checked ? file : null)
                  }
                  aria-label={t("codexConfig.instructionsFileToggle", {
                    defaultValue: "启用或停用 {{name}}",
                    name,
                  })}
                />
                <FileText className="h-4 w-4 shrink-0 text-muted-foreground" />
                <div className="min-w-0 flex-1">
                  <div className="truncate text-sm font-medium" title={name}>
                    {name}
                  </div>
                  <div
                    className="truncate text-xs text-muted-foreground"
                    title={file}
                  >
                    {file}
                  </div>
                  <div
                    className={`mt-0.5 flex min-h-4 items-center gap-1 text-xs ${statusClassName}`}
                    title={statusTitle}
                  >
                    {inspection?.loading ? (
                      <>
                        <Loader2 className="h-3 w-3 shrink-0 animate-spin" />
                        <span>
                          {t("codexConfig.instructionsFileStateChecking", {
                            defaultValue: "正在检查文件",
                          })}
                        </span>
                      </>
                    ) : inspection?.error ? (
                      <>
                        <CircleX className="h-3 w-3 shrink-0" />
                        <span>
                          {t("codexConfig.instructionsFileStateCheckFailed", {
                            defaultValue: "文件检查失败",
                          })}
                        </span>
                      </>
                    ) : status ? (
                      <>
                        {status.state === "valid" ? (
                          <CircleCheck className="h-3 w-3 shrink-0" />
                        ) : (
                          <TriangleAlert className="h-3 w-3 shrink-0" />
                        )}
                        <span>
                          {status.sizeBytes === null
                            ? stateLabel
                            : t("codexConfig.instructionsFileStateWithSize", {
                                defaultValue: "{{state}} · {{size}} B",
                                state: stateLabel,
                                size: status.sizeBytes.toLocaleString(),
                              })}
                        </span>
                      </>
                    ) : null}
                  </div>
                </div>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  className="h-8 w-8 shrink-0"
                  onClick={() => openEditEditor(file)}
                  title={t("codexConfig.instructionsFileEdit", {
                    defaultValue: "编辑文件路径",
                  })}
                >
                  <Pencil className="h-4 w-4" />
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  className="h-8 w-8 shrink-0 text-muted-foreground hover:text-destructive"
                  onClick={() => handleRemove(file)}
                  title={t("codexConfig.instructionsFileRemove", {
                    defaultValue: "从列表移除",
                  })}
                >
                  <Trash2 className="h-4 w-4" />
                </Button>
              </div>
            );
          })}
        </div>
      )}

      <Dialog
        open={editorOpen}
        onOpenChange={(open) => {
          if (!open) closeEditor();
        }}
      >
        <DialogContent className="max-w-lg" zIndex="top">
          <DialogHeader>
            <DialogTitle>
              {editingPath
                ? t("codexConfig.instructionsFileEditTitle", {
                    defaultValue: "编辑模型指令文件",
                  })
                : t("codexConfig.instructionsFileAddTitle", {
                    defaultValue: "添加模型指令文件",
                  })}
            </DialogTitle>
            <DialogDescription>
              {t("codexConfig.instructionsFileDialogHint", {
                defaultValue:
                  "支持 .md 或 .txt 文件；相对路径以包含 config.toml 的目录为基准。",
              })}
            </DialogDescription>
          </DialogHeader>

          <div className="space-y-2 px-6 py-5">
            <Label htmlFor="codex-model-instructions-file-path">
              {t("codexConfig.instructionsFilePath", {
                defaultValue: "文件路径",
              })}
            </Label>
            <div className="flex gap-1.5">
              <Input
                id="codex-model-instructions-file-path"
                value={draftPath}
                onChange={(event) => setDraftPath(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    event.preventDefault();
                    handleSave();
                  }
                }}
                placeholder={t("codexConfig.instructionsFilePlaceholder", {
                  defaultValue: "./instruction_5.6.md 或绝对路径",
                })}
                autoFocus
              />
              <Button
                type="button"
                variant="outline"
                size="icon"
                className="shrink-0"
                onClick={handleBrowse}
                title={t("codexConfig.instructionsFileBrowse", {
                  defaultValue: "选择 Markdown 或文本文件",
                })}
              >
                <FolderOpen className="h-4 w-4" />
              </Button>
            </div>
            {duplicatePath && (
              <p className="text-xs text-destructive">
                {t("codexConfig.instructionsFileDuplicate", {
                  defaultValue: "该文件已在列表中",
                })}
              </p>
            )}
          </div>

          <DialogFooter>
            <Button type="button" variant="outline" onClick={closeEditor}>
              {t("common.cancel", { defaultValue: "取消" })}
            </Button>
            <Button
              type="button"
              onClick={handleSave}
              disabled={!normalizedDraftPath || duplicatePath}
            >
              {t("common.save", { defaultValue: "保存" })}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
