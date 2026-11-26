import { invoke } from "@tauri-apps/api/core";

export interface DiagnosticsConfig {
  captureBodies: boolean;
}

export type BodyState =
  | "notCaptured"
  | "pending"
  | "ready"
  | "evicted"
  | "tooLarge"
  | "unreadable"
  | "interrupted"
  | "memoryLimit";

export interface FullBodyMetadata {
  state: BodyState;
  bytes: number;
}

export interface FullBodyResult {
  state: BodyState;
  body: string | null;
}

export interface AttemptDiagnostics {
  providerId: string;
  providerName: string;
  model: string;
  startedMs: number;
  preparedMs: number | null;
  headersMs: number | null;
  firstContentMs: number | null;
  durationMs: number | null;
  statusCode: number | null;
  error: string | null;
  canceled: boolean;
  responseOutcome?:
    "completed" | "failed" | "incomplete" | "disconnected" | null;
  requestFull?: FullBodyMetadata;
  responseFull?: FullBodyMetadata;
  context?: {
    system: number;
    tools: number;
    chat: number;
    results: number;
    media: number;
    partial: boolean;
  } | null;
}

export interface RequestDiagnostics {
  traceId: string;
  route?: [string, string][];
  captureBodies: boolean;
  requestBody: string;
  responseBody: string;
  requestTruncated: boolean;
  responseTruncated: boolean;
  attempts: AttemptDiagnostics[];
  attemptsTruncated: boolean;
}

export const requestDiagnosticsApi = {
  get: (requestId: string): Promise<RequestDiagnostics | null> =>
    invoke("get_request_diagnostics", { requestId }),
  getBody: (
    requestId: string,
    side: "request" | "response",
    attemptIndex: number,
  ): Promise<FullBodyResult> =>
    invoke("get_request_diagnostics_body", { requestId, side, attemptIndex }),
  getConfig: (): Promise<DiagnosticsConfig> =>
    invoke("get_request_diagnostics_config"),
  exportBody: (
    requestId: string,
    side: "request" | "response",
    attemptIndex: number,
  ): Promise<boolean> =>
    invoke("export_request_diagnostics_body", {
      requestId,
      side,
      attemptIndex,
    }),
  setConfig: (config: DiagnosticsConfig): Promise<void> =>
    invoke("set_request_diagnostics_config", { config }),
};
