import { invoke } from "@tauri-apps/api/core";

export interface WrapperStatus {
  installed: boolean;
  needsUpgrade: boolean;
  conflictingWrapper: boolean;
  shellType?: string;
  configFile?: string;
  /** 旧注入点（如 zsh 的 ~/.zshrc）仍残留 wrapper，需重新「安装」以清理。 */
  staleConfigFile?: string | null;
  /** 检测到可疑情况（如 ~/.zshenv 含 return 早退，wrapper 可能不生效）。 */
  warning?: string | null;
}

/**
 * 检查 shell wrapper 状态
 */
export async function checkShellWrapperStatus(): Promise<WrapperStatus> {
  return await invoke("check_shell_wrapper_status");
}

/**
 * 安装 shell wrapper
 * @returns 配置文件路径
 */
export async function installShellWrapper(): Promise<string> {
  return await invoke("install_shell_wrapper");
}

/**
 * 卸载 shell wrapper
 * @returns 配置文件路径
 */
export async function uninstallShellWrapper(): Promise<string> {
  return await invoke("uninstall_shell_wrapper");
}

/**
 * 获取手动安装指令
 * @returns 安装指令文本
 */
export async function getShellWrapperInstructions(): Promise<string> {
  return await invoke("get_shell_wrapper_instructions");
}
