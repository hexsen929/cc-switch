import { invoke } from "@tauri-apps/api/core";

export type CodexInstructionsFileState =
  | "valid"
  | "missing"
  | "notFile"
  | "unreadable"
  | "empty"
  | "invalid";

export interface CodexInstructionsFileStatus {
  configuredPath: string;
  resolvedPath: string;
  state: CodexInstructionsFileState;
  exists: boolean;
  isFile: boolean;
  isSymlink: boolean;
  readable: boolean;
  sizeBytes: number | null;
  modifiedAt: number | null;
  sha256: string | null;
  error: string | null;
}

export const codexInstructionsApi = {
  async inspect(configuredPath: string): Promise<CodexInstructionsFileStatus> {
    return await invoke("inspect_codex_instructions_file", { configuredPath });
  },

  async read(configuredPath: string): Promise<string | null> {
    return await invoke("read_codex_instructions_file", { configuredPath });
  },

  async write(
    configuredPath: string,
    content: string,
  ): Promise<CodexInstructionsFileStatus> {
    return await invoke("write_codex_instructions_file", {
      configuredPath,
      content,
    });
  },

  async remove(configuredPath: string): Promise<boolean> {
    return await invoke("delete_codex_instructions_file", { configuredPath });
  },
};
