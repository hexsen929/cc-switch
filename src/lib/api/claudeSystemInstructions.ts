import { invoke } from "@tauri-apps/api/core";
import type { ClaudeSystemInstructionsConfig } from "@/types";
import type { ClaudeAppendInstructionsFileStatus } from "./claudeAppendInstructions";

export type { ClaudeSystemInstructionsConfig } from "@/types";
export type {
  ClaudeAppendInstructionsFileState as ClaudeSystemInstructionsFileState,
  ClaudeAppendInstructionsFileStatus as ClaudeSystemInstructionsFileStatus,
} from "./claudeAppendInstructions";

export const claudeSystemInstructionsApi = {
  async getConfig(): Promise<ClaudeSystemInstructionsConfig> {
    return await invoke("get_claude_system_instructions_config");
  },

  async setConfig(
    config: ClaudeSystemInstructionsConfig,
  ): Promise<ClaudeSystemInstructionsConfig> {
    return await invoke("set_claude_system_instructions_config", { config });
  },

  async inspect(
    configuredPath: string,
  ): Promise<ClaudeAppendInstructionsFileStatus> {
    return await invoke("inspect_claude_system_instructions_file", {
      configuredPath,
    });
  },

  async read(configuredPath: string): Promise<string | null> {
    return await invoke("read_claude_system_instructions_file", {
      configuredPath,
    });
  },

  async write(
    configuredPath: string,
    content: string,
  ): Promise<ClaudeAppendInstructionsFileStatus> {
    return await invoke("write_claude_system_instructions_file", {
      configuredPath,
      content,
    });
  },

  async remove(configuredPath: string): Promise<boolean> {
    return await invoke("delete_claude_system_instructions_file", {
      configuredPath,
    });
  },
};
