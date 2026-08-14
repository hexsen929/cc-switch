import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { open as openFile } from "@tauri-apps/plugin-dialog";
import type { TOptions } from "i18next";
import {
  CircleCheck,
  CircleX,
  FileText,
  FolderOpen,
  Loader2,
  Pencil,
  Plus,
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
import { claudeSystemInstructionsApi } from "@/lib/api/claudeSystemInstructions";
import type { ClaudeSystemInstructionsConfig } from "@/types";

export interface ClaudeAppendInstructionsFileFieldProps {
  config: ClaudeAppendInstructionsConfig;
  onChange: (config: ClaudeAppendInstructionsConfig) => void;
}

export interface ClaudeSystemInstructionsFileFieldProps {
  config: ClaudeSystemInstructionsConfig;
  onChange: (config: ClaudeSystemInstructionsConfig) => void;
}

type ClaudeInstructionKind = "append" | "system";

interface ClaudeInstructionFilesConfig {
  files: string[];
  activeFile?: string | null;
}

interface ClaudeInstructionFileApi {
  inspect: (
    configuredPath: string,
  ) => Promise<ClaudeAppendInstructionsFileStatus>;
  read: (configuredPath: string) => Promise<string | null>;
  write: (
    configuredPath: string,
    content: string,
  ) => Promise<ClaudeAppendInstructionsFileStatus>;
  remove: (configuredPath: string) => Promise<boolean>;
}

interface ClaudeInstructionsFileFieldProps {
  kind: ClaudeInstructionKind;
  config: ClaudeInstructionFilesConfig;
  onChange: (config: ClaudeInstructionFilesConfig) => void;
}

interface FileInspection {
  loading: boolean;
  status?: ClaudeAppendInstructionsFileStatus;
  error?: string;
}

const normalizeConfig = (
  config: ClaudeInstructionFilesConfig,
): ClaudeInstructionFilesConfig => {
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

export function ClaudeAppendInstructionsFileField({
  config,
  onChange,
}: ClaudeAppendInstructionsFileFieldProps) {
  return (
    <ClaudeInstructionsFileField
      kind="append"
      config={config}
      onChange={onChange}
    />
  );
}

export function ClaudeSystemInstructionsFileField({
  config,
  onChange,
}: ClaudeSystemInstructionsFileFieldProps) {
  return (
    <ClaudeInstructionsFileField
      kind="system"
      config={config}
      onChange={onChange}
    />
  );
}

function ClaudeInstructionsFileField({
  kind,
  config,
  onChange,
}: ClaudeInstructionsFileFieldProps) {
  const { t } = useTranslation();
  const isSystem = kind === "system";
  const translationNamespace = isSystem
    ? "claudeSystemInstructions"
    : "claudeAppendInstructions";
  const instructionLabel = isSystem ? "Claude 系统指令" : "Claude 追加指令";
  const runtimeFlag = isSystem
    ? "--system-prompt-file"
    : "--append-system-prompt-file";
  const defaultRelativePath = isSystem
    ? "./cc-switch/system-instructions/default.md"
    : "./cc-switch/append-instructions/default.md";
  const instructionsApi: ClaudeInstructionFileApi = isSystem
    ? claudeSystemInstructionsApi
    : claudeAppendInstructionsApi;
  const instructionT = useCallback(
    (key: string, options: TOptions & { defaultValue: string }) =>
      t(`${translationNamespace}.${key}`, options),
    [t, translationNamespace],
  );
  const [editorOpen, setEditorOpen] = useState(false);
  const [editingPath, setEditingPath] = useState<string | null>(null);
  const [draftPath, setDraftPath] = useState("");
  const [draftContent, setDraftContent] = useState("");
  const [contentLoading, setContentLoading] = useState(false);
  const [contentLoadFailed, setContentLoadFailed] = useState(false);
  const [saving, setSaving] = useState(false);
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

  useEffect(() => {
    let cancelled = false;
    setFileInspections(
      Object.fromEntries(
        normalizedFiles.map((file) => [file, { loading: true }]),
      ),
    );
    for (const file of normalizedFiles) {
      void instructionsApi
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
  }, [instructionsApi, normalizedFiles]);

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

  const loadEditorContent = useCallback(
    async (path: string) => {
      const sequence = ++editorLoadSequenceRef.current;
      setContentLoading(true);
      setContentLoadFailed(false);
      try {
        const content = await instructionsApi.read(path);
        if (sequence !== editorLoadSequenceRef.current) return;
        setDraftContent(content ?? "");
      } catch (error) {
        if (sequence !== editorLoadSequenceRef.current) return;
        setDraftContent("");
        setContentLoadFailed(true);
        toast.error(
          instructionT("readFailed", {
            defaultValue: `无法读取 ${instructionLabel}文件：{{error}}`,
            error: errorMessage(error),
          }),
        );
      } finally {
        if (sequence === editorLoadSequenceRef.current) {
          setContentLoading(false);
        }
      }
    },
    [instructionLabel, instructionT, instructionsApi],
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
        instructionT("browseFailed", {
          defaultValue: `无法选择 ${instructionLabel}文件：{{error}}`,
          error: errorMessage(error),
        }),
      );
    }
  }, [instructionLabel, instructionT, loadEditorContent]);

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
      const status = await instructionsApi.write(
        normalizedDraftPath,
        draftContent,
      );
      if (!editingPath) {
        onChange(
          normalizeConfig({
            files: [...normalizedFiles, normalizedDraftPath],
            activeFile,
          }),
        );
      }
      setFileInspections((current) => ({
        ...current,
        [normalizedDraftPath]: { loading: false, status },
      }));
      toast.success(
        instructionT("saveSuccess", {
          defaultValue: `${instructionLabel}文件已保存`,
        }),
      );
      closeEditor();
    } catch (error) {
      toast.error(
        instructionT("saveFailed", {
          defaultValue: `保存${instructionLabel}文件失败：{{error}}`,
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
    onChange,
    saving,
    instructionLabel,
    instructionT,
    instructionsApi,
  ]);

  const handleToggle = useCallback(
    (path: string, enabled: boolean) => {
      onChange(
        normalizeConfig({
          files: normalizedFiles,
          activeFile: enabled ? path : null,
        }),
      );
    },
    [normalizedFiles, onChange],
  );

  const handleConfirmRemove = useCallback(async () => {
    const path = pendingDeletePath;
    if (!path || deletingPath) return;
    setPendingDeletePath(null);
    setDeletingPath(path);
    try {
      await instructionsApi.remove(path);
      onChange(
        normalizeConfig({
          files: normalizedFiles.filter((file) => file !== path),
          activeFile: activeFile === path ? null : activeFile,
        }),
      );
      setFileInspections((current) => {
        const next = { ...current };
        delete next[path];
        return next;
      });
      if (editingPath === path) closeEditor();
      toast.success(
        instructionT("deleteSuccess", {
          defaultValue: `${instructionLabel}文件已删除`,
        }),
      );
    } catch (error) {
      toast.error(
        instructionT("deleteFailed", {
          defaultValue: `删除${instructionLabel}文件失败：{{error}}`,
          error: errorMessage(error),
        }),
      );
    } finally {
      setDeletingPath(null);
    }
  }, [
    activeFile,
    closeEditor,
    deletingPath,
    editingPath,
    normalizedFiles,
    onChange,
    pendingDeletePath,
    instructionLabel,
    instructionT,
    instructionsApi,
  ]);

  const stateLabel = useCallback(
    (state: ClaudeAppendInstructionsFileState) => {
      const labels: Record<ClaudeAppendInstructionsFileState, string> = {
        valid: instructionT("stateValid", {
          defaultValue: "文件可用",
        }),
        missing: instructionT("stateMissing", {
          defaultValue: "文件不存在",
        }),
        notFile: instructionT("stateNotFile", {
          defaultValue: "路径不是文件",
        }),
        unreadable: instructionT("stateUnreadable", {
          defaultValue: "文件不可读",
        }),
        empty: instructionT("stateEmpty", {
          defaultValue: "文件内容为空",
        }),
        invalid: instructionT("stateInvalid", {
          defaultValue: "文件内容无效",
        }),
      };
      return labels[state];
    },
    [instructionT],
  );

  return (
    <div className="space-y-3 border-t border-border-default pt-3">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0 space-y-1">
          <div className="flex items-center gap-2 text-sm font-medium text-foreground">
            <Terminal className="h-4 w-4 shrink-0 text-muted-foreground" />
            {instructionT("summaryTitle", {
              defaultValue: isSystem
                ? "Claude 运行时系统指令"
                : "Claude 运行时追加指令",
            })}
          </div>
          <p className="text-xs leading-relaxed text-muted-foreground">
            {instructionT("summaryHint", {
              defaultValue: `每个 Claude 供应商独立管理 ${runtimeFlag}；保存供应商后应用，不会修改普通 CLAUDE.md 提示词预设。`,
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
          {instructionT("add", {
            defaultValue: "新建文件",
          })}
        </Button>
      </div>

      <div className="rounded-md border border-border-default bg-muted/30 px-3 py-2 text-xs text-muted-foreground">
        {instructionT("count", {
          defaultValue: "共 {{count}} 个文件",
          count: normalizedFiles.length,
        })}
        {" · "}
        {activeFile
          ? instructionT("enabledName", {
              defaultValue: "已启用：{{name}}",
              name: fileName(activeFile),
            })
          : instructionT("noneEnabled", {
              defaultValue: "未启用任何文件",
            })}
      </div>

      {normalizedFiles.length === 0 ? (
        <div className="flex min-h-20 items-center justify-center rounded-md border border-dashed border-border-default px-4 text-center text-sm text-muted-foreground">
          {instructionT("empty", {
            defaultValue: `暂无 ${instructionLabel}文件`,
          })}
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
                  onCheckedChange={(checked) => handleToggle(file, checked)}
                  aria-label={instructionT("toggle", {
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
                            status.sha256 ? `SHA-256: ${status.sha256}` : null,
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
                          {instructionT("stateChecking", {
                            defaultValue: "正在检查文件",
                          })}
                        </span>
                      </>
                    ) : inspection?.error ? (
                      <>
                        <CircleX className="h-3 w-3" />
                        <span>
                          {instructionT("stateCheckFailed", {
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
                  title={instructionT("edit", {
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
                  title={instructionT("remove", {
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
                ? instructionT("editTitle", {
                    defaultValue: `编辑${instructionLabel}文件`,
                  })
                : instructionT("addTitle", {
                    defaultValue: `新建${instructionLabel}文件`,
                  })}
            </DialogTitle>
            <DialogDescription>
              {instructionT("dialogHint", {
                defaultValue: `文件内容会用于 Claude Code 的 ${runtimeFlag}；编辑时直接写回原文件。`,
              })}
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-4 overflow-y-auto px-6 py-5">
            <div className="space-y-2">
              <Label htmlFor={`claude-${kind}-instructions-path`}>
                {instructionT("path", {
                  defaultValue: "文件路径",
                })}
              </Label>
              <div className="flex gap-1.5">
                <Input
                  id={`claude-${kind}-instructions-path`}
                  value={draftPath}
                  onChange={(event) => {
                    setDraftPath(event.target.value);
                    setContentLoadFailed(false);
                  }}
                  placeholder={instructionT("pathPlaceholder", {
                    defaultValue: `${defaultRelativePath} 或绝对路径`,
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
                    title={instructionT("browse", {
                      defaultValue: "选择 Markdown 或文本文件",
                    })}
                  >
                    <FolderOpen className="h-4 w-4" />
                  </Button>
                )}
              </div>
              {duplicatePath && (
                <p className="text-xs text-destructive">
                  {instructionT("duplicate", {
                    defaultValue: "该文件已在列表中",
                  })}
                </p>
              )}
            </div>
            <div className="space-y-2">
              <Label htmlFor={`claude-${kind}-instructions-content`}>
                {instructionT("content", {
                  defaultValue: "文件内容",
                })}
              </Label>
              <div className="relative">
                <Textarea
                  id={`claude-${kind}-instructions-content`}
                  value={draftContent}
                  onChange={(event) => setDraftContent(event.target.value)}
                  placeholder={instructionT("contentPlaceholder", {
                    defaultValue: `输入要通过 ${runtimeFlag} 加载的运行时指令...`,
                  })}
                  className="min-h-72 max-h-[45vh] resize-y font-mono text-xs leading-5"
                  autoFocus={Boolean(editingPath)}
                  disabled={contentLoading || saving}
                />
                {contentLoading && (
                  <div className="absolute inset-0 flex items-center justify-center rounded-md bg-background/80 text-sm text-muted-foreground">
                    <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                    {instructionT("loadingContent", {
                      defaultValue: "正在读取文件...",
                    })}
                  </div>
                )}
              </div>
              {contentLoadFailed && (
                <p className="text-xs text-destructive">
                  {instructionT("readRetry", {
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
        title={instructionT("deleteTitle", {
          defaultValue: `删除${instructionLabel}文件`,
        })}
        message={instructionT("deleteConfirm", {
          defaultValue: `将永久删除磁盘上的文件 {{path}}，并从此供应商的${instructionLabel}列表中移除。此操作无法撤销。`,
          path: pendingDeletePath ?? "",
        })}
        confirmText={instructionT("deleteButton", {
          defaultValue: "删除文件",
        })}
        onConfirm={() => void handleConfirmRemove()}
        onCancel={() => setPendingDeletePath(null)}
      />
    </div>
  );
}

export default ClaudeAppendInstructionsFileField;
