import React, {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { open as openFile } from "@tauri-apps/plugin-dialog";
import {
  CircleCheck,
  CircleX,
  FileText,
  FolderOpen,
  Loader2,
  Pencil,
  Terminal,
  Trash2,
  TriangleAlert,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ConfirmDialog";
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
import { Textarea } from "@/components/ui/textarea";
import {
  claudeAppendInstructionsApi,
  type ClaudeAppendInstructionsConfig,
  type ClaudeAppendInstructionsFileState,
  type ClaudeAppendInstructionsFileStatus,
} from "@/lib/api/claudeAppendInstructions";
import { ClaudeShellWrapperCard } from "@/components/claude/ClaudeShellWrapperCard";

interface ClaudeAppendInstructionsPanelProps {
  open: boolean;
}

export interface ClaudeAppendInstructionsPanelHandle {
  openAdd: () => void;
}

interface FileInspection {
  loading: boolean;
  status?: ClaudeAppendInstructionsFileStatus;
  error?: string;
}

const normalizeConfig = (
  config: ClaudeAppendInstructionsConfig,
): ClaudeAppendInstructionsConfig => {
  const files = Array.from(
    new Set(
      (config.files ?? [])
        .map((file) => file.trim())
        .filter((file) => file.length > 0),
    ),
  );
  const activeFile = config.activeFile?.trim() || null;
  if (activeFile && !files.includes(activeFile)) files.push(activeFile);
  return { files, activeFile };
};

const fileName = (path: string): string => {
  const normalized = path.replace(/\\/g, "/").replace(/\/+$/, "");
  return normalized.split("/").pop() || path;
};

const errorMessage = (error: unknown): string =>
  error instanceof Error ? error.message : String(error);

export const ClaudeAppendInstructionsPanel = React.forwardRef<
  ClaudeAppendInstructionsPanelHandle,
  ClaudeAppendInstructionsPanelProps
>(function ClaudeAppendInstructionsPanel(
  { open }: ClaudeAppendInstructionsPanelProps,
  ref: React.ForwardedRef<ClaudeAppendInstructionsPanelHandle>,
) {
  const { t } = useTranslation();
  const [config, setConfig] = useState<ClaudeAppendInstructionsConfig>({
    files: [],
    activeFile: null,
  });
  const [loading, setLoading] = useState(true);
  const [editorOpen, setEditorOpen] = useState(false);
  const [editingPath, setEditingPath] = useState<string | null>(null);
  const [draftPath, setDraftPath] = useState("");
  const [draftContent, setDraftContent] = useState("");
  const [contentLoading, setContentLoading] = useState(false);
  const [contentLoadFailed, setContentLoadFailed] = useState(false);
  const [saving, setSaving] = useState(false);
  const [updatingConfig, setUpdatingConfig] = useState(false);
  const [pendingDeletePath, setPendingDeletePath] = useState<string | null>(
    null,
  );
  const [deletingPath, setDeletingPath] = useState<string | null>(null);
  const [fileInspections, setFileInspections] = useState<
    Record<string, FileInspection>
  >({});
  const editorLoadSequenceRef = useRef(0);

  const normalizedConfig = useMemo(() => normalizeConfig(config), [config]);
  const normalizedFiles = normalizedConfig.files;
  const activeFile = normalizedConfig.activeFile;
  const normalizedDraftPath = draftPath.trim();
  const duplicatePath = normalizedFiles.some(
    (file) => file === normalizedDraftPath && file !== editingPath,
  );

  const reload = useCallback(async () => {
    setLoading(true);
    try {
      const next = normalizeConfig(
        await claudeAppendInstructionsApi.getConfig(),
      );
      setConfig(next);
    } catch (error) {
      toast.error(
        t("claudeAppendInstructions.loadFailed", {
          defaultValue: "加载 Claude 追加指令失败：{{error}}",
          error: errorMessage(error),
        }),
      );
    } finally {
      setLoading(false);
    }
  }, [t]);

  useEffect(() => {
    if (open) void reload();
  }, [open, reload]);

  useEffect(() => {
    let cancelled = false;
    setFileInspections(
      Object.fromEntries(
        normalizedFiles.map((file) => [file, { loading: true }]),
      ),
    );
    for (const file of normalizedFiles) {
      void claudeAppendInstructionsApi
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
            [file]: { loading: false, error: errorMessage(error) },
          }));
        });
    }
    return () => {
      cancelled = true;
    };
  }, [normalizedFiles]);

  const closeEditor = useCallback(() => {
    editorLoadSequenceRef.current += 1;
    setEditorOpen(false);
    setEditingPath(null);
    setDraftPath("");
    setDraftContent("");
    setContentLoading(false);
    setContentLoadFailed(false);
  }, []);

  const openAddEditor = useCallback(() => {
    editorLoadSequenceRef.current += 1;
    setEditingPath(null);
    setDraftPath("");
    setDraftContent("");
    setContentLoading(false);
    setContentLoadFailed(false);
    setEditorOpen(true);
  }, []);

  React.useImperativeHandle(ref, () => ({ openAdd: openAddEditor }), [
    openAddEditor,
  ]);

  const loadEditorContent = useCallback(
    async (path: string) => {
      const sequence = ++editorLoadSequenceRef.current;
      setContentLoading(true);
      setContentLoadFailed(false);
      try {
        const content = await claudeAppendInstructionsApi.read(path);
        if (sequence !== editorLoadSequenceRef.current) return;
        setDraftContent(content ?? "");
      } catch (error) {
        if (sequence !== editorLoadSequenceRef.current) return;
        setDraftContent("");
        setContentLoadFailed(true);
        toast.error(
          t("claudeAppendInstructions.readFailed", {
            defaultValue: "无法读取 Claude 追加指令文件：{{error}}",
            error: errorMessage(error),
          }),
        );
      } finally {
        if (sequence === editorLoadSequenceRef.current) {
          setContentLoading(false);
        }
      }
    },
    [t],
  );

  const openEditEditor = useCallback(
    (path: string) => {
      setEditingPath(path);
      setDraftPath(path);
      setDraftContent("");
      setEditorOpen(true);
      void loadEditorContent(path);
    },
    [loadEditorContent],
  );

  const handleBrowse = useCallback(async () => {
    try {
      const selected = await openFile({
        multiple: false,
        directory: false,
        filters: [{ name: "Markdown / Text", extensions: ["md", "txt"] }],
      });
      const selectedPath = Array.isArray(selected) ? selected[0] : selected;
      if (selectedPath) {
        setDraftPath(selectedPath);
        await loadEditorContent(selectedPath);
      }
    } catch (error) {
      toast.error(
        t("claudeAppendInstructions.browseFailed", {
          defaultValue: "无法选择 Claude 追加指令文件：{{error}}",
          error: errorMessage(error),
        }),
      );
    }
  }, [loadEditorContent, t]);

  const handleSave = useCallback(async () => {
    if (
      !normalizedDraftPath ||
      duplicatePath ||
      contentLoading ||
      contentLoadFailed ||
      saving
    ) {
      return;
    }
    setSaving(true);
    try {
      const status = await claudeAppendInstructionsApi.write(
        normalizedDraftPath,
        draftContent,
      );
      if (!editingPath) {
        const next = await claudeAppendInstructionsApi.setConfig({
          files: [...normalizedFiles, normalizedDraftPath],
          activeFile,
        });
        setConfig(normalizeConfig(next));
      }
      setFileInspections((current) => ({
        ...current,
        [normalizedDraftPath]: { loading: false, status },
      }));
      toast.success(
        t("claudeAppendInstructions.saveSuccess", {
          defaultValue: "Claude 追加指令文件已保存",
        }),
      );
      closeEditor();
    } catch (error) {
      toast.error(
        t("claudeAppendInstructions.saveFailed", {
          defaultValue: "保存 Claude 追加指令文件失败：{{error}}",
          error: errorMessage(error),
        }),
      );
    } finally {
      setSaving(false);
    }
  }, [
    activeFile,
    closeEditor,
    contentLoadFailed,
    contentLoading,
    draftContent,
    duplicatePath,
    editingPath,
    normalizedDraftPath,
    normalizedFiles,
    saving,
    t,
  ]);

  const handleToggle = useCallback(
    async (path: string, enabled: boolean) => {
      if (updatingConfig) return;
      setUpdatingConfig(true);
      try {
        const next = await claudeAppendInstructionsApi.setConfig({
          files: normalizedFiles,
          activeFile: enabled ? path : null,
        });
        setConfig(normalizeConfig(next));
        toast.success(
          t(
            enabled
              ? "claudeAppendInstructions.enableSuccess"
              : "claudeAppendInstructions.disableSuccess",
            {
              defaultValue: enabled
                ? "Claude 追加指令已启用"
                : "Claude 追加指令已停用",
            },
          ),
        );
      } catch (error) {
        toast.error(
          t("claudeAppendInstructions.configFailed", {
            defaultValue: "更新 Claude 追加指令状态失败：{{error}}",
            error: errorMessage(error),
          }),
        );
      } finally {
        setUpdatingConfig(false);
      }
    },
    [normalizedFiles, t, updatingConfig],
  );

  const handleConfirmRemove = useCallback(async () => {
    const path = pendingDeletePath;
    if (!path || deletingPath) return;
    setPendingDeletePath(null);
    setDeletingPath(path);
    try {
      await claudeAppendInstructionsApi.remove(path);
      setConfig((current) =>
        normalizeConfig({
          files: current.files.filter((file) => file !== path),
          activeFile: current.activeFile === path ? null : current.activeFile,
        }),
      );
      setFileInspections((current) => {
        const next = { ...current };
        delete next[path];
        return next;
      });
      if (editingPath === path) closeEditor();
      toast.success(
        t("claudeAppendInstructions.deleteSuccess", {
          defaultValue: "Claude 追加指令文件已删除",
        }),
      );
    } catch (error) {
      toast.error(
        t("claudeAppendInstructions.deleteFailed", {
          defaultValue: "删除 Claude 追加指令文件失败：{{error}}",
          error: errorMessage(error),
        }),
      );
    } finally {
      setDeletingPath(null);
    }
  }, [closeEditor, deletingPath, editingPath, pendingDeletePath, t]);

  const stateLabel = useCallback(
    (state: ClaudeAppendInstructionsFileState) => {
      const labels: Record<ClaudeAppendInstructionsFileState, string> = {
        valid: t("claudeAppendInstructions.stateValid", {
          defaultValue: "文件可用",
        }),
        missing: t("claudeAppendInstructions.stateMissing", {
          defaultValue: "文件不存在",
        }),
        notFile: t("claudeAppendInstructions.stateNotFile", {
          defaultValue: "路径不是文件",
        }),
        unreadable: t("claudeAppendInstructions.stateUnreadable", {
          defaultValue: "文件不可读",
        }),
        empty: t("claudeAppendInstructions.stateEmpty", {
          defaultValue: "文件内容为空",
        }),
        invalid: t("claudeAppendInstructions.stateInvalid", {
          defaultValue: "文件内容无效",
        }),
      };
      return labels[state];
    },
    [t],
  );

  if (!open) return null;

  return (
    <div className="flex min-h-0 flex-1 flex-col px-6">
      <div className="flex-shrink-0 space-y-3 py-4">
        <div className="glass rounded-xl border border-white/10 px-6 py-4">
          <div>
            <div className="min-w-0">
              <div className="flex items-center gap-2 text-sm font-medium text-foreground">
                <Terminal className="h-4 w-4 text-muted-foreground" />
                {t("claudeAppendInstructions.summaryTitle", {
                  defaultValue: "Claude 运行时追加指令",
                })}
              </div>
              <p className="mt-1 text-xs leading-relaxed text-muted-foreground">
                {t("claudeAppendInstructions.summaryHint", {
                  defaultValue:
                    "独立管理 --append-system-prompt-file；不会修改普通 CLAUDE.md 提示词预设。",
                })}
              </p>
            </div>
          </div>
          <div className="mt-3 text-xs text-muted-foreground">
            {t("claudeAppendInstructions.count", {
              defaultValue: "共 {{count}} 个文件",
              count: normalizedFiles.length,
            })}
            {" · "}
            {activeFile
              ? t("claudeAppendInstructions.enabledName", {
                  defaultValue: "已启用：{{name}}",
                  name: fileName(activeFile),
                })
              : t("claudeAppendInstructions.noneEnabled", {
                  defaultValue: "未启用任何文件",
                })}
          </div>
        </div>
      </div>

      <div className="flex-1 overflow-y-auto pb-16">
        {loading ? (
          <div className="py-12 text-center text-muted-foreground">
            {t("claudeAppendInstructions.loading", {
              defaultValue: "加载中...",
            })}
          </div>
        ) : normalizedFiles.length === 0 ? (
          <div className="py-12 text-center">
            <div className="mx-auto mb-4 flex h-16 w-16 items-center justify-center rounded-full bg-muted">
              <FileText size={24} className="text-muted-foreground" />
            </div>
            <h3 className="mb-2 text-lg font-medium text-foreground">
              {t("claudeAppendInstructions.empty", {
                defaultValue: "暂无 Claude 追加指令文件",
              })}
            </h3>
            <p className="text-sm text-muted-foreground">
              {t("claudeAppendInstructions.emptyHint", {
                defaultValue: "点击右上角新建文件，保存后再选择启用。",
              })}
            </p>
          </div>
        ) : (
          <div className="space-y-3">
            {normalizedFiles.map((file) => {
              const inspection = fileInspections[file];
              const status = inspection?.status;
              const statusClass = status
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
                    checked={file === activeFile}
                    disabled={updatingConfig}
                    onCheckedChange={(checked) =>
                      void handleToggle(file, checked)
                    }
                    aria-label={t("claudeAppendInstructions.toggle", {
                      defaultValue: "启用或停用 {{name}}",
                      name: fileName(file),
                    })}
                  />
                  <FileText className="h-4 w-4 shrink-0 text-muted-foreground" />
                  <div className="min-w-0 flex-1">
                    <div
                      className="truncate text-sm font-medium"
                      title={fileName(file)}
                    >
                      {fileName(file)}
                    </div>
                    <div
                      className="truncate text-xs text-muted-foreground"
                      title={file}
                    >
                      {file}
                    </div>
                    <div
                      className={`mt-0.5 flex min-h-4 items-center gap-1 text-xs ${statusClass}`}
                      title={
                        status
                          ? [
                              status.resolvedPath,
                              status.sha256
                                ? `SHA-256: ${status.sha256}`
                                : null,
                              status.error,
                            ]
                              .filter(Boolean)
                              .join("\n")
                          : inspection?.error
                      }
                    >
                      {inspection?.loading ? (
                        <>
                          <Loader2 className="h-3 w-3 animate-spin" />
                          <span>
                            {t("claudeAppendInstructions.stateChecking", {
                              defaultValue: "正在检查文件",
                            })}
                          </span>
                        </>
                      ) : inspection?.error ? (
                        <>
                          <CircleX className="h-3 w-3" />
                          <span>
                            {t("claudeAppendInstructions.stateCheckFailed", {
                              defaultValue: "文件检查失败",
                            })}
                          </span>
                        </>
                      ) : status ? (
                        <>
                          {status.state === "valid" ? (
                            <CircleCheck className="h-3 w-3" />
                          ) : (
                            <TriangleAlert className="h-3 w-3" />
                          )}
                          <span>
                            {status.sizeBytes === null
                              ? stateLabel(status.state)
                              : `${stateLabel(status.state)} · ${status.sizeBytes.toLocaleString()} B`}
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
                    title={t("claudeAppendInstructions.edit", {
                      defaultValue: "编辑文件",
                    })}
                  >
                    <Pencil className="h-4 w-4" />
                  </Button>
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    className="h-8 w-8 shrink-0 text-muted-foreground hover:text-destructive"
                    onClick={() => setPendingDeletePath(file)}
                    disabled={deletingPath !== null}
                    title={t("claudeAppendInstructions.remove", {
                      defaultValue: "删除文件",
                    })}
                  >
                    {deletingPath === file ? (
                      <Loader2 className="h-4 w-4 animate-spin" />
                    ) : (
                      <Trash2 className="h-4 w-4" />
                    )}
                  </Button>
                </div>
              );
            })}
          </div>
        )}

        <div className="mt-6">
          <ClaudeShellWrapperCard />
        </div>
      </div>

      <Dialog
        open={editorOpen}
        onOpenChange={(nextOpen) => {
          if (!nextOpen && !saving) closeEditor();
        }}
      >
        <DialogContent className="max-w-3xl" zIndex="top">
          <DialogHeader>
            <DialogTitle>
              {editingPath
                ? t("claudeAppendInstructions.editTitle", {
                    defaultValue: "编辑 Claude 追加指令文件",
                  })
                : t("claudeAppendInstructions.addTitle", {
                    defaultValue: "新建 Claude 追加指令文件",
                  })}
            </DialogTitle>
            <DialogDescription>
              {t("claudeAppendInstructions.dialogHint", {
                defaultValue:
                  "文件内容会用于 Claude Code 的 --append-system-prompt-file；编辑时直接写回原文件。",
              })}
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-4 overflow-y-auto px-6 py-5">
            <div className="space-y-2">
              <Label htmlFor="claude-append-instructions-path">
                {t("claudeAppendInstructions.path", {
                  defaultValue: "文件路径",
                })}
              </Label>
              <div className="flex gap-1.5">
                <Input
                  id="claude-append-instructions-path"
                  value={draftPath}
                  onChange={(event) => {
                    setDraftPath(event.target.value);
                    setContentLoadFailed(false);
                  }}
                  placeholder={t("claudeAppendInstructions.pathPlaceholder", {
                    defaultValue:
                      "./cc-switch/append-instructions/default.md 或绝对路径",
                  })}
                  autoFocus={!editingPath}
                  disabled={Boolean(editingPath) || saving}
                />
                {!editingPath && (
                  <Button
                    type="button"
                    variant="outline"
                    size="icon"
                    className="shrink-0"
                    onClick={() => void handleBrowse()}
                    disabled={saving}
                    title={t("claudeAppendInstructions.browse", {
                      defaultValue: "选择 Markdown 或文本文件",
                    })}
                  >
                    <FolderOpen className="h-4 w-4" />
                  </Button>
                )}
              </div>
              {duplicatePath && (
                <p className="text-xs text-destructive">
                  {t("claudeAppendInstructions.duplicate", {
                    defaultValue: "该文件已在列表中",
                  })}
                </p>
              )}
            </div>
            <div className="space-y-2">
              <Label htmlFor="claude-append-instructions-content">
                {t("claudeAppendInstructions.content", {
                  defaultValue: "文件内容",
                })}
              </Label>
              <div className="relative">
                <Textarea
                  id="claude-append-instructions-content"
                  value={draftContent}
                  onChange={(event) => setDraftContent(event.target.value)}
                  placeholder={t(
                    "claudeAppendInstructions.contentPlaceholder",
                    {
                      defaultValue: "输入要追加到 Claude Code 的运行时指令...",
                    },
                  )}
                  className="min-h-72 max-h-[45vh] resize-y font-mono text-xs leading-5"
                  autoFocus={Boolean(editingPath)}
                  disabled={contentLoading || saving}
                />
                {contentLoading && (
                  <div className="absolute inset-0 flex items-center justify-center rounded-md bg-background/80 text-sm text-muted-foreground">
                    <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                    {t("claudeAppendInstructions.loadingContent", {
                      defaultValue: "正在读取文件...",
                    })}
                  </div>
                )}
              </div>
              {contentLoadFailed && (
                <p className="text-xs text-destructive">
                  {t("claudeAppendInstructions.readRetry", {
                    defaultValue:
                      "文件读取失败，无法安全保存。请关闭后重试或检查文件权限。",
                  })}
                </p>
              )}
            </div>
          </div>
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              onClick={closeEditor}
              disabled={saving}
            >
              {t("common.cancel", { defaultValue: "取消" })}
            </Button>
            <Button
              type="button"
              onClick={() => void handleSave()}
              disabled={
                !normalizedDraftPath ||
                duplicatePath ||
                contentLoading ||
                contentLoadFailed ||
                saving
              }
            >
              {saving
                ? t("common.saving", { defaultValue: "保存中..." })
                : t("common.save", { defaultValue: "保存" })}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <ConfirmDialog
        isOpen={pendingDeletePath !== null}
        zIndex="top"
        title={t("claudeAppendInstructions.deleteTitle", {
          defaultValue: "删除 Claude 追加指令文件",
        })}
        message={t("claudeAppendInstructions.deleteConfirm", {
          defaultValue:
            "将永久删除磁盘上的文件 {{path}}，并停用它。此操作无法撤销。",
          path: pendingDeletePath ?? "",
        })}
        confirmText={t("claudeAppendInstructions.deleteButton", {
          defaultValue: "删除文件",
        })}
        onConfirm={() => void handleConfirmRemove()}
        onCancel={() => setPendingDeletePath(null)}
      />
    </div>
  );
});

ClaudeAppendInstructionsPanel.displayName = "ClaudeAppendInstructionsPanel";

export default ClaudeAppendInstructionsPanel;
