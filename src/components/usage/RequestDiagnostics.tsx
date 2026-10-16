import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import {
  requestDiagnosticsApi,
  type AttemptDiagnostics,
  type RequestDiagnostics as Diagnostics,
} from "@/lib/api/requestDiagnostics";

/** Only measured phases are shown. Missing historical/network data is never inferred. */
export function attemptPhases(attempt: AttemptDiagnostics) {
  const phases: { key: string; ms: number; color: string }[] = [];
  let cursor = 0;
  for (const [key, end, color] of [
    ["prepare", attempt.preparedMs, "bg-fg-3"],
    ["headers", attempt.headersMs, "bg-blue-500"],
    ["content", attempt.firstContentMs, "bg-amber-500"],
    ["receive", attempt.durationMs, "bg-emerald-500"],
  ] as const) {
    if (end == null || !Number.isFinite(end) || end < cursor) continue;
    if (end > cursor) phases.push({ key, ms: end - cursor, color });
    cursor = end;
  }
  return phases;
}

function Preview({
  title,
  body,
  truncated,
}: {
  title: string;
  body: string;
  truncated: boolean;
}) {
  const { t } = useTranslation();
  return (
    <details className="min-w-0 rounded-panel border border-border px-3 py-2">
      <summary className="cursor-pointer text-caption font-medium text-fg-1">
        {title}
      </summary>
      {truncated && (
        <p className="mt-2 text-caption text-fg-3">
          {t("requestDiagnostics.truncated")}
        </p>
      )}
      <pre className="mt-2 max-h-64 overflow-auto whitespace-pre-wrap break-all font-mono text-caption text-fg-2">
        {body || t("requestDiagnostics.noBody")}
      </pre>
    </details>
  );
}

export function RequestDiagnostics({ requestId }: { requestId: string }) {
  const { t } = useTranslation();
  const [record, setRecord] = useState<Diagnostics | null>(null);
  const [capture, setCapture] = useState(false);
  const [configLoaded, setConfigLoaded] = useState(false);
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState(false);
  const revision = useRef(0);

  useEffect(() => {
    let active = true;
    const epoch = ++revision.current;
    setRecord(null);
    setConfigLoaded(false);
    setBusy(false);
    setFailed(false);
    requestDiagnosticsApi
      .get(requestId)
      .then((value) => {
        if (active && revision.current === epoch) setRecord(value ?? null);
      })
      .catch(() => {
        if (active && revision.current === epoch) setFailed(true);
      });
    requestDiagnosticsApi
      .getConfig()
      .then((config) => {
        if (active && revision.current === epoch && config) {
          setCapture(config.captureBodies);
          setConfigLoaded(true);
        }
      })
      .catch(() => {});
    return () => {
      active = false;
      revision.current += 1;
    };
  }, [requestId]);

  const toggleCapture = async () => {
    if (busy) return;
    const epoch = ++revision.current;
    setBusy(true);
    setFailed(false);
    try {
      await requestDiagnosticsApi.setConfig({ captureBodies: !capture });
      if (revision.current !== epoch) return;
      setCapture(!capture);
      if (capture) setRecord(null);
    } catch {
      if (revision.current === epoch) setFailed(true);
    } finally {
      if (revision.current === epoch) setBusy(false);
    }
  };

  return (
    <section
      className="flex min-w-0 flex-col gap-2"
      aria-label={t("requestDiagnostics.title")}
    >
      <h3 className="m-0 text-strong font-semibold text-fg-1">
        {t("requestDiagnostics.title")}
      </h3>
      <p className="m-0 text-caption text-fg-3">
        {t("requestDiagnostics.scope")}
      </p>
      {configLoaded && (
        <div className="flex items-center justify-between gap-3">
          <span className="text-caption text-fg-2">
            {t("requestDiagnostics.capture")}
          </span>
          <Button
            variant={capture ? "solid" : "neutral"}
            size="compact"
            disabled={busy}
            onClick={() => void toggleCapture()}
            aria-pressed={capture}
          >
            {t(
              capture
                ? "requestDiagnostics.captureOn"
                : "requestDiagnostics.captureOff",
            )}
          </Button>
        </div>
      )}
      {failed && (
        <p role="alert" className="m-0 text-caption text-danger">
          {t("requestDiagnostics.loadFailed")}
        </p>
      )}
      {!record ? (
        <p className="m-0 text-caption text-fg-3">
          {t("requestDiagnostics.unavailable")}
        </p>
      ) : (
        <>
          <ol className="m-0 flex list-none flex-col gap-2 p-0">
            {record.attempts.map((attempt, index) => {
              const phases = attemptPhases(attempt);
              const total =
                phases.reduce((sum, phase) => sum + phase.ms, 0) || 1;
              return (
                <li
                  key={`${attempt.providerId}-${index}`}
                  className="min-w-0 rounded-panel border border-border p-3"
                >
                  <div className="flex items-start justify-between gap-2 text-caption">
                    <span className="min-w-0 break-words font-medium text-fg-1">
                      {index + 1}. {attempt.providerName || attempt.providerId}
                    </span>
                    <span className="shrink-0 tabular-nums text-fg-2">
                      {attempt.statusCode ?? "—"} ·{" "}
                      {attempt.durationMs == null
                        ? "…"
                        : `${attempt.durationMs} ms`}
                    </span>
                  </div>
                  <p className="m-0 mt-1 break-all font-mono text-caption text-fg-3">
                    {attempt.model}
                  </p>
                  {phases.length > 0 && (
                    <>
                      <div
                        className="mt-2 flex h-1.5 overflow-hidden rounded-full"
                        aria-hidden="true"
                      >
                        {phases.map((phase) => (
                          <span
                            key={phase.key}
                            className={phase.color}
                            style={{ flexGrow: phase.ms / total }}
                          />
                        ))}
                      </div>
                      <dl className="m-0 mt-2 grid grid-cols-2 gap-x-3 gap-y-1 text-caption text-fg-2">
                        {phases.map((phase) => (
                          <div
                            key={phase.key}
                            className="flex justify-between gap-1"
                          >
                            <dt>
                              {t(`requestDiagnostics.phases.${phase.key}`)}
                            </dt>
                            <dd className="m-0 tabular-nums">{phase.ms} ms</dd>
                          </div>
                        ))}
                      </dl>
                    </>
                  )}
                  {attempt.canceled && (
                    <p className="m-0 mt-2 text-caption text-fg-3">
                      {t("requestDiagnostics.canceled")}
                    </p>
                  )}
                  {attempt.error && (
                    <p className="m-0 mt-2 whitespace-pre-wrap break-all text-caption text-danger">
                      {attempt.error}
                    </p>
                  )}
                </li>
              );
            })}
          </ol>
          {record.attemptsTruncated && (
            <p className="m-0 text-caption text-fg-3">
              {t("requestDiagnostics.attemptsTruncated")}
            </p>
          )}
          {record.captureBodies && (
            <>
              <p className="m-0 text-caption text-fg-3">
                {t("requestDiagnostics.upstream")}
              </p>
              <Preview
                title={t("requestDiagnostics.request")}
                body={record.requestBody}
                truncated={record.requestTruncated}
              />
              <Preview
                title={t("requestDiagnostics.response")}
                body={record.responseBody}
                truncated={record.responseTruncated}
              />
            </>
          )}
        </>
      )}
    </section>
  );
}
