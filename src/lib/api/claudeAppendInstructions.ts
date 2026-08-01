import { invoke } from "@tauri-apps/api/core";

export type ClaudeAppendInstructionsFileState =
  | "valid"
  | "missing"
  | "notFile"
  | "unreadable"
  | "empty"
  | "invalid";

export interface ClaudeAppendInstructionsConfig {
  files: string[];
  activeFile?: string | null;
}

export interface ClaudeAppendInstructionsFileStatus {
  configuredPath: string;
  resolvedPath: string;
  state: ClaudeAppendInstructionsFileState;
  exists: boolean;
  isFile: boolean;
  isSymlink: boolean;
  readable: boolean;
  sizeBytes: number | null;
  modifiedAt: number | null;
  sha256: string | null;
  error: string | null;
}

export const claudeAppendInstructionsApi = {
  async getConfig(): Promise<ClaudeAppendInstructionsConfig> {
    return await invoke("get_claude_append_instructions_config");
  },

  async setConfig(
    config: ClaudeAppendInstructionsConfig,
  ): Promise<ClaudeAppendInstructionsConfig> {
    return await invoke("set_claude_append_instructions_config", { config });
  },

  async inspect(
    configuredPath: string,
  ): Promise<ClaudeAppendInstructionsFileStatus> {
    return await invoke("inspect_claude_append_instructions_file", {
      configuredPath,
    });
  },

  async read(configuredPath: string): Promise<string | null> {
    return await invoke("read_claude_append_instructions_file", {
      configuredPath,
    });
  },

  async write(
    configuredPath: string,
    content: string,
  ): Promise<ClaudeAppendInstructionsFileStatus> {
    return await invoke("write_claude_append_instructions_file", {
      configuredPath,
      content,
    });
  },

  async remove(configuredPath: string): Promise<boolean> {
    return await invoke("delete_claude_append_instructions_file", {
      configuredPath,
    });
  },
};
