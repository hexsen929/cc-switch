import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Columns3 } from "lucide-react";
import type { UsageDisplaySettings } from "@/types";
import { ChecklistMenu } from "./ChecklistMenu";
import {
  REQUEST_LOG_COLUMN_LABEL_KEYS,
  resolveRequestLogColumns,
  setRequestLogColumn,
  TOGGLEABLE_REQUEST_LOG_COLUMNS,
  type RequestLogColumnId,
} from "./requestLogColumns";

interface RequestLogColumnsMenuProps {
  value: UsageDisplaySettings | undefined;
  onChange: (next: UsageDisplaySettings) => Promise<boolean> | boolean | void;
}

/** 请求日志工具栏的「显示的列」：只存改过的列；保存要等设置读回来，先按刚勾的显示 */
export function RequestLogColumnsMenu({
  value,
  onChange,
}: RequestLogColumnsMenuProps) {
  const { t } = useTranslation();
  const savedColumns = value?.requestLogColumns;
  const [columnOverrides, setColumnOverrides] = useState(savedColumns ?? {});
  const optimisticColumns = useRef(savedColumns ?? {});
  const confirmedColumns = useRef(savedColumns ?? {});
  const queuedColumns = useRef<Record<string, boolean> | null>(null);
  const saving = useRef(false);
  const mounted = useRef(true);
  const latestProps = useRef({ value, onChange });
  useEffect(() => {
    latestProps.current = { value, onChange };
  }, [value, onChange]);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      queuedColumns.current = null;
    };
  }, []);
  useEffect(() => {
    // 保存中读回的只是中间状态，不能冲掉后续点击的乐观选态。
    if (!saving.current) {
      confirmedColumns.current = savedColumns ?? {};
      optimisticColumns.current = savedColumns ?? {};
      setColumnOverrides(savedColumns ?? {});
    }
  }, [savedColumns]);
  const logColumns = resolveRequestLogColumns(columnOverrides);

  // 一次只发起一个保存，避免旧写晚到覆盖新写。连续点击只保留最新的完整快照，
  // 它已包含前面的选择；当前保存失败也继续处理后续选择。
  const flushSaves = async () => {
    if (saving.current) return;
    saving.current = true;
    try {
      while (mounted.current && queuedColumns.current != null) {
        const next = queuedColumns.current;
        queuedColumns.current = null;
        let saved = false;
        try {
          const current = latestProps.current;
          saved =
            (await current.onChange({
              ...current.value,
              requestLogColumns: next,
            })) !== false;
        } catch {
          // 调用方负责提示保存失败；这里和返回 false 一样回滚，不中断队列。
        }
        if (!mounted.current) return;
        if (saved) confirmedColumns.current = next;
        if (queuedColumns.current == null) {
          const settled = saved ? next : confirmedColumns.current;
          optimisticColumns.current = settled;
          setColumnOverrides(settled);
        }
      }
    } finally {
      saving.current = false;
    }
  };

  const saveColumnOverrides = (next: Record<string, boolean>) => {
    if (!mounted.current) return;
    optimisticColumns.current = next;
    setColumnOverrides(next);
    queuedColumns.current = next;
    void flushSaves();
  };

  return (
    <ChecklistMenu
      quiet
      icon={<Columns3 aria-hidden="true" className="h-4 w-4" />}
      label={t("usage.display.logColumns")}
      summary={t("usage.display.logColumns")}
      items={TOGGLEABLE_REQUEST_LOG_COLUMNS.map(({ id }) => ({
        id,
        label: t(REQUEST_LOG_COLUMN_LABEL_KEYS[id]),
        checked: logColumns.includes(id),
      }))}
      onToggle={(id) => {
        const column = id as RequestLogColumnId;
        const current = optimisticColumns.current;
        saveColumnOverrides(
          setRequestLogColumn(
            current,
            column,
            !resolveRequestLogColumns(current).includes(column),
          ),
        );
      }}
      onReset={
        Object.keys(columnOverrides).length > 0
          ? () => saveColumnOverrides({})
          : undefined
      }
    />
  );
}
