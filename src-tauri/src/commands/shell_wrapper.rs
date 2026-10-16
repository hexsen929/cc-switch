use crate::claude_append_instructions::{runtime_projection_path, system_runtime_projection_path};
use crate::config::{atomic_write, get_home_dir};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const WRAPPER_START: &str = "# >>> cc-switch claude append prompt >>>";
const WRAPPER_END: &str = "# <<< cc-switch claude append prompt <<<";
const LEGACY_CC_SWITCH_START: &str = "# >>> claude-keysmith runtime (managed by CC Switch) >>>";
const LEGACY_KEYSMITH_START: &str = "# >>> claude-keysmith runtime >>>";
const LEGACY_KEYSMITH_END: &str = "# <<< claude-keysmith runtime <<<";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WrapperStatus {
    pub installed: bool,
    pub needs_upgrade: bool,
    pub conflicting_wrapper: bool,
    pub shell_type: Option<String>,
    pub config_file: Option<String>,
    /// 曾写入但现已废弃的旧配置文件（如 zsh 的 `~/.zshrc`）上仍残留 wrapper 块；
    /// 需要用户重新「安装」以清理，避免同一函数被定义两次。
    #[serde(default)]
    pub stale_config_file: Option<String>,
    /// 本次检测发现的可疑情况（例如 `.zshenv` 含早退逻辑、wrapper 可能不生效）。
    #[serde(default)]
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WrapperKind {
    Current,
    Legacy,
}

/// 决定把 wrapper 写到哪个/哪些 shell 启动文件。
///
/// zsh 使用 `~/.zshenv`，使正常加载启动文件的交互及非交互 shell 都能定义 wrapper。
/// 直接执行 Claude 二进制的启动方式不会调用 shell 函数。
///
/// - `primary`：真正写入 wrapper 的文件。
/// - `additional`：同样需要安装的文件（如 Bash 的另一种启动入口）。
/// - `legacy`：历史上写过、现在不再是注入点的文件；安装时顺带清理其残留块。
#[derive(Debug, Clone)]
struct ShellTargets {
    primary: PathBuf,
    additional: Vec<PathBuf>,
    legacy: Vec<PathBuf>,
}

impl ShellTargets {
    fn install_paths(&self) -> impl Iterator<Item = &PathBuf> {
        std::iter::once(&self.primary).chain(&self.additional)
    }
}

fn get_shell_targets() -> Result<(ShellTargets, String), String> {
    let home = get_home_dir();

    #[cfg(target_os = "windows")]
    {
        let powershell = home
            .join("Documents")
            .join("PowerShell")
            .join("Microsoft.PowerShell_profile.ps1");
        let windows_powershell = home
            .join("Documents")
            .join("WindowsPowerShell")
            .join("Microsoft.PowerShell_profile.ps1");

        let primary = if windows_powershell.exists() && !powershell.exists() {
            windows_powershell
        } else {
            powershell
        };
        Ok((
            ShellTargets {
                primary,
                additional: Vec::new(),
                legacy: Vec::new(),
            },
            "PowerShell".to_string(),
        ))
    }

    #[cfg(not(target_os = "windows"))]
    {
        let shell = std::env::var("SHELL").unwrap_or_default();
        let shell_name = Path::new(&shell)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();

        shell_targets_for(&home, shell_name)
    }
}

#[cfg(not(target_os = "windows"))]
fn shell_targets_for(home: &Path, shell_name: &str) -> Result<(ShellTargets, String), String> {
    match shell_name {
        "zsh" => Ok((
            ShellTargets {
                primary: home.join(".zshenv"),
                additional: Vec::new(),
                legacy: vec![home.join(".zshrc")],
            },
            "Zsh".to_string(),
        )),
        "bash" => {
            // Bash 只读第一个存在的登录配置；沿用它，避免新建 .bash_profile
            // 遮蔽用户原有的 .bash_login 或 .profile。
            let login = [".bash_profile", ".bash_login", ".profile"]
                .into_iter()
                .map(|name| home.join(name))
                .find(|path| path.exists())
                .unwrap_or_else(|| home.join(".bash_profile"));
            let interactive = home.join(".bashrc");
            #[cfg(target_os = "macos")]
            let (primary, secondary) = (login, interactive);
            #[cfg(not(target_os = "macos"))]
            let (primary, secondary) = (interactive, login);
            Ok((
                ShellTargets {
                    primary,
                    additional: vec![secondary],
                    legacy: Vec::new(),
                },
                "Bash".to_string(),
            ))
        }
        "fish" => Ok((
            ShellTargets {
                primary: home.join(".config").join("fish").join("config.fish"),
                additional: Vec::new(),
                legacy: Vec::new(),
            },
            "Fish".to_string(),
        )),
        _ => Err(format!(
            "暂不支持当前 shell: {}",
            if shell_name.is_empty() {
                "unknown"
            } else {
                shell_name
            }
        )),
    }
}

/// `.zshenv` 若含「早退」逻辑（`return` / `exit`），其后的 wrapper 定义不会执行——
/// 用户会以为装了却没生效。这里只读检测并给出提示，不改写用户文件。
fn detect_early_exit(path: &Path) -> bool {
    if !path.exists() {
        return false;
    }
    let Ok((content, _)) = read_config(path) else {
        return false;
    };
    content.lines().any(|line| {
        let line = line.trim_start();
        !line.starts_with('#')
            && (line == "return"
                || line.starts_with("return ")
                || line == "exit"
                || line.starts_with("exit "))
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WrapperBlock {
    start: usize,
    end: usize,
    kind: WrapperKind,
}

#[derive(Debug, Default)]
struct WrapperInspection {
    blocks: Vec<WrapperBlock>,
    conflicting_wrapper: bool,
}

#[derive(Debug, Clone, Copy)]
enum TextEncoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
}

fn get_append_prompt_path() -> Result<PathBuf, String> {
    Ok(runtime_projection_path())
}

fn get_system_prompt_path() -> Result<PathBuf, String> {
    Ok(system_runtime_projection_path())
}

fn is_default_append_prompt_path(path: &Path) -> bool {
    path == get_home_dir()
        .join(".claude")
        .join("cc-switch")
        .join("append-prompt.md")
}

fn is_default_system_prompt_path(path: &Path) -> bool {
    path == get_home_dir()
        .join(".claude")
        .join("cc-switch")
        .join("system-prompt.md")
}

fn quote_posix(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn quote_fish(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

fn quote_powershell(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn posix_append_path_expression(path: &Path) -> String {
    if is_default_append_prompt_path(path) {
        "\"$HOME/.claude/cc-switch/append-prompt.md\"".to_string()
    } else {
        quote_posix(&path.to_string_lossy())
    }
}

fn posix_system_path_expression(path: &Path) -> String {
    if is_default_system_prompt_path(path) {
        "\"$HOME/.claude/cc-switch/system-prompt.md\"".to_string()
    } else {
        quote_posix(&path.to_string_lossy())
    }
}

fn fish_append_path_expression(path: &Path) -> String {
    if is_default_append_prompt_path(path) {
        "\"$HOME/.claude/cc-switch/append-prompt.md\"".to_string()
    } else {
        quote_fish(&path.to_string_lossy())
    }
}

fn fish_system_path_expression(path: &Path) -> String {
    if is_default_system_prompt_path(path) {
        "\"$HOME/.claude/cc-switch/system-prompt.md\"".to_string()
    } else {
        quote_fish(&path.to_string_lossy())
    }
}

fn powershell_append_path_expression(path: &Path) -> String {
    if is_default_append_prompt_path(path) {
        "Join-Path $HOME \".claude\\cc-switch\\append-prompt.md\"".to_string()
    } else {
        quote_powershell(&path.to_string_lossy())
    }
}

fn powershell_system_path_expression(path: &Path) -> String {
    if is_default_system_prompt_path(path) {
        "Join-Path $HOME \".claude\\cc-switch\\system-prompt.md\"".to_string()
    } else {
        quote_powershell(&path.to_string_lossy())
    }
}

fn generate_posix_wrapper(append_prompt_path: &Path, system_prompt_path: &Path) -> String {
    let append_expr = posix_append_path_expression(append_prompt_path);
    let system_expr = posix_system_path_expression(system_prompt_path);
    format!(
        r#"{WRAPPER_START}
claude() {{
    local append_prompt_file={append_expr}
    local system_prompt_file={system_expr}
    local has_append_prompt_arg=0
    local has_system_prompt_arg=0
    local arg
    for arg in "$@"; do
        case "$arg" in
            --)
                break
                ;;
            --append-system-prompt|--append-system-prompt=*|--append-system-prompt-file|--append-system-prompt-file=*)
                has_append_prompt_arg=1
                ;;
            --system-prompt|--system-prompt=*|--system-prompt-file|--system-prompt-file=*)
                has_system_prompt_arg=1
                ;;
        esac
    done

    local -a extra_args=()
    if [ -s "$system_prompt_file" ] && [ "$has_system_prompt_arg" -eq 0 ]; then
        extra_args+=(--system-prompt-file "$system_prompt_file")
    fi
    if [ -s "$append_prompt_file" ] && [ "$has_append_prompt_arg" -eq 0 ]; then
        extra_args+=(--append-system-prompt-file "$append_prompt_file")
    fi

    command claude "${{extra_args[@]}}" "$@"
}}
{WRAPPER_END}"#
    )
}

fn generate_fish_wrapper(append_prompt_path: &Path, system_prompt_path: &Path) -> String {
    let append_expr = fish_append_path_expression(append_prompt_path);
    let system_expr = fish_system_path_expression(system_prompt_path);
    format!(
        r#"{WRAPPER_START}
function claude
    set -l append_prompt_file {append_expr}
    set -l system_prompt_file {system_expr}
    set -l has_append_prompt_arg 0
    set -l has_system_prompt_arg 0
    for arg in $argv
        switch $arg
            case '--'
                break
            case '--append-system-prompt' '--append-system-prompt=*' '--append-system-prompt-file' '--append-system-prompt-file=*'
                set has_append_prompt_arg 1
            case '--system-prompt' '--system-prompt=*' '--system-prompt-file' '--system-prompt-file=*'
                set has_system_prompt_arg 1
        end
    end

    set -l extra_args
    if test -s "$system_prompt_file"; and test $has_system_prompt_arg -eq 0
        set extra_args --system-prompt-file "$system_prompt_file"
    end
    if test -s "$append_prompt_file"; and test $has_append_prompt_arg -eq 0
        set extra_args $extra_args --append-system-prompt-file "$append_prompt_file"
    end

    if test (count $extra_args) -gt 0
        command claude $extra_args $argv
    else
        command claude $argv
    end
end
{WRAPPER_END}"#
    )
}

fn generate_powershell_wrapper(append_prompt_path: &Path, system_prompt_path: &Path) -> String {
    let append_expr = powershell_append_path_expression(append_prompt_path);
    let system_expr = powershell_system_path_expression(system_prompt_path);
    format!(
        r#"{WRAPPER_START}
$script:CCSwitchClaudeExecutable = (Get-Command claude -CommandType Application -ErrorAction Stop).Path
function global:claude {{
    $appendPromptFile = {append_expr}
    $systemPromptFile = {system_expr}
    $hasAppendPromptArgument = $false
    $hasSystemPromptArgument = $false
    foreach ($argument in $args) {{
        if ($argument -eq "--") {{
            break
        }}
        if (
            $argument -eq "--append-system-prompt" -or
            $argument -like "--append-system-prompt=*" -or
            $argument -eq "--append-system-prompt-file" -or
            $argument -like "--append-system-prompt-file=*"
        ) {{
            $hasAppendPromptArgument = $true
        }}
        if (
            $argument -eq "--system-prompt" -or
            $argument -like "--system-prompt=*" -or
            $argument -eq "--system-prompt-file" -or
            $argument -like "--system-prompt-file=*"
        ) {{
            $hasSystemPromptArgument = $true
        }}
    }}

    $extraArgs = @()
    if ((Test-Path -LiteralPath $systemPromptFile -PathType Leaf) -and ((Get-Item -LiteralPath $systemPromptFile).Length -gt 0) -and -not $hasSystemPromptArgument) {{
        $extraArgs += "--system-prompt-file"
        $extraArgs += $systemPromptFile
    }}
    if ((Test-Path -LiteralPath $appendPromptFile -PathType Leaf) -and ((Get-Item -LiteralPath $appendPromptFile).Length -gt 0) -and -not $hasAppendPromptArgument) {{
        $extraArgs += "--append-system-prompt-file"
        $extraArgs += $appendPromptFile
    }}

    if ($extraArgs.Count -gt 0) {{
        & $script:CCSwitchClaudeExecutable @extraArgs @args
    }} else {{
        & $script:CCSwitchClaudeExecutable @args
    }}
}}
{WRAPPER_END}"#
    )
}

fn generate_wrapper_code(
    shell_type: &str,
    append_prompt_path: &Path,
    system_prompt_path: &Path,
) -> Result<String, String> {
    match shell_type {
        "Zsh" | "Bash" => Ok(generate_posix_wrapper(
            append_prompt_path,
            system_prompt_path,
        )),
        "Fish" => Ok(generate_fish_wrapper(
            append_prompt_path,
            system_prompt_path,
        )),
        "PowerShell" => Ok(generate_powershell_wrapper(
            append_prompt_path,
            system_prompt_path,
        )),
        _ => Err(format!("不支持生成 {shell_type} wrapper")),
    }
}

fn decode_config(bytes: &[u8]) -> Result<(String, TextEncoding), String> {
    if let Some(content) = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]) {
        return String::from_utf8(content.to_vec())
            .map(|text| (text, TextEncoding::Utf8Bom))
            .map_err(|e| format!("配置文件不是有效的 UTF-8: {e}"));
    }

    if let Some(content) = bytes.strip_prefix(&[0xff, 0xfe]) {
        if content.len() % 2 != 0 {
            return Err("UTF-16LE 配置文件长度无效".to_string());
        }
        let units = content
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect::<Vec<_>>();
        return String::from_utf16(&units)
            .map(|text| (text, TextEncoding::Utf16Le))
            .map_err(|e| format!("配置文件不是有效的 UTF-16LE: {e}"));
    }

    if let Some(content) = bytes.strip_prefix(&[0xfe, 0xff]) {
        if content.len() % 2 != 0 {
            return Err("UTF-16BE 配置文件长度无效".to_string());
        }
        let units = content
            .chunks_exact(2)
            .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
            .collect::<Vec<_>>();
        return String::from_utf16(&units)
            .map(|text| (text, TextEncoding::Utf16Be))
            .map_err(|e| format!("配置文件不是有效的 UTF-16BE: {e}"));
    }

    String::from_utf8(bytes.to_vec())
        .map(|text| (text, TextEncoding::Utf8))
        .map_err(|e| format!("配置文件不是有效的 UTF-8: {e}"))
}

fn encode_config(content: &str, encoding: TextEncoding) -> Vec<u8> {
    match encoding {
        TextEncoding::Utf8 => content.as_bytes().to_vec(),
        TextEncoding::Utf8Bom => {
            let mut bytes = vec![0xef, 0xbb, 0xbf];
            bytes.extend_from_slice(content.as_bytes());
            bytes
        }
        TextEncoding::Utf16Le => {
            let mut bytes = vec![0xff, 0xfe];
            bytes.extend(
                content
                    .encode_utf16()
                    .flat_map(|unit| unit.to_le_bytes())
                    .collect::<Vec<_>>(),
            );
            bytes
        }
        TextEncoding::Utf16Be => {
            let mut bytes = vec![0xfe, 0xff];
            bytes.extend(
                content
                    .encode_utf16()
                    .flat_map(|unit| unit.to_be_bytes())
                    .collect::<Vec<_>>(),
            );
            bytes
        }
    }
}

fn read_config(path: &Path) -> Result<(String, TextEncoding), String> {
    if !path.exists() {
        return Ok((String::new(), TextEncoding::Utf8));
    }

    let bytes = fs::read(path).map_err(|e| format!("读取配置文件失败: {e}"))?;
    decode_config(&bytes)
}

fn resolve_config_path(path: &Path) -> Result<PathBuf, String> {
    if fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fs::canonicalize(path).map_err(|e| format!("解析配置文件软链接失败: {e}"))
    } else {
        Ok(path.to_path_buf())
    }
}

fn backup_config(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("shell-config");
    let backup_path = path.with_file_name(format!("{file_name}.cc-switch.{timestamp}.bak"));
    fs::copy(path, &backup_path).map_err(|e| format!("备份配置文件失败: {e}"))?;
    Ok(())
}

fn marker_ranges(
    content: &str,
    start_marker: &str,
    end_marker: &str,
) -> Result<Vec<(usize, usize)>, String> {
    let mut ranges = Vec::new();
    let mut cursor = 0;

    loop {
        let next_start = content[cursor..]
            .find(start_marker)
            .map(|index| cursor + index);
        let next_end = content[cursor..]
            .find(end_marker)
            .map(|index| cursor + index);

        match (next_start, next_end) {
            (None, None) => break,
            (Some(start), Some(end)) if end >= start + start_marker.len() => {
                if content[start + start_marker.len()..end].contains(start_marker) {
                    return Err(incomplete_wrapper_error());
                }
                let block_end = end + end_marker.len();
                ranges.push((start, block_end));
                cursor = block_end;
            }
            _ => return Err(incomplete_wrapper_error()),
        }
    }

    Ok(ranges)
}

fn next_legacy_start(content: &str, cursor: usize) -> Option<(usize, &'static str)> {
    [LEGACY_CC_SWITCH_START, LEGACY_KEYSMITH_START]
        .into_iter()
        .filter_map(|marker| {
            content[cursor..]
                .find(marker)
                .map(|index| (cursor + index, marker))
        })
        .min_by_key(|(index, _)| *index)
}

fn incomplete_wrapper_error() -> String {
    "检测到不完整的 Claude shell wrapper，请先手动修复配置文件".to_string()
}

fn inspect_wrapper(content: &str) -> Result<WrapperInspection, String> {
    let mut inspection = WrapperInspection::default();

    for (start, end) in marker_ranges(content, WRAPPER_START, WRAPPER_END)? {
        inspection.blocks.push(WrapperBlock {
            start,
            end,
            kind: WrapperKind::Current,
        });
    }

    let mut cursor = 0;
    loop {
        let next_start = next_legacy_start(content, cursor);
        let next_end = content[cursor..]
            .find(LEGACY_KEYSMITH_END)
            .map(|index| cursor + index);

        let (start, start_marker, end) = match (next_start, next_end) {
            (None, None) => break,
            (Some((start, marker)), Some(end)) if end >= start + marker.len() => {
                (start, marker, end)
            }
            _ => return Err(incomplete_wrapper_error()),
        };

        if next_legacy_start(content, start + start_marker.len())
            .is_some_and(|(nested_start, _)| nested_start < end)
        {
            return Err(incomplete_wrapper_error());
        }

        let block_end = end + LEGACY_KEYSMITH_END.len();
        let block_content = &content[start..block_end];
        let is_external_keysmith = start_marker == LEGACY_KEYSMITH_START
            && (block_content.contains("--system-prompt-file")
                || block_content.contains("Managed by claude-keysmith"));

        if is_external_keysmith {
            inspection.conflicting_wrapper = true;
        } else {
            inspection.blocks.push(WrapperBlock {
                start,
                end: block_end,
                kind: WrapperKind::Legacy,
            });
        }
        cursor = block_end;
    }

    inspection.blocks.sort_by_key(|block| block.start);
    if inspection
        .blocks
        .windows(2)
        .any(|blocks| blocks[0].end > blocks[1].start)
    {
        return Err(incomplete_wrapper_error());
    }

    Ok(inspection)
}

fn replace_wrapper_blocks(
    content: &str,
    blocks: &[WrapperBlock],
    replacement: Option<&str>,
) -> String {
    let removed_len = blocks
        .iter()
        .map(|block| block.end - block.start)
        .sum::<usize>();
    let replacement_len = replacement.map(str::len).unwrap_or_default();
    let mut updated = String::with_capacity(content.len() - removed_len + replacement_len);
    let mut cursor = 0;

    for (index, block) in blocks.iter().enumerate() {
        updated.push_str(&content[cursor..block.start]);
        if index == 0 {
            if let Some(replacement) = replacement {
                updated.push_str(replacement);
            }
        }
        cursor = block.end;
    }
    updated.push_str(&content[cursor..]);
    updated
}

fn detect_newline(content: &str) -> &'static str {
    if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

#[tauri::command]
pub fn check_shell_wrapper_status() -> Result<WrapperStatus, String> {
    let (targets, shell_type) = get_shell_targets()?;
    let append_prompt_path = get_append_prompt_path()?;
    let system_prompt_path = get_system_prompt_path()?;
    let wrapper = generate_wrapper_code(&shell_type, &append_prompt_path, &system_prompt_path)?;
    check_wrapper_status_at(&targets, &shell_type, &wrapper)
}

fn current_wrapper_matches(content: &str, inspection: &WrapperInspection, wrapper: &str) -> bool {
    inspection.blocks.len() == 1
        && inspection.blocks[0].kind == WrapperKind::Current
        && content[inspection.blocks[0].start..inspection.blocks[0].end]
            == wrapper.replace('\n', detect_newline(content))
}

fn check_wrapper_status_at(
    targets: &ShellTargets,
    shell_type: &str,
    wrapper: &str,
) -> Result<WrapperStatus, String> {
    let mut installed = false;
    let mut all_current = true;
    let mut conflicting_wrapper = false;
    for path in targets.install_paths() {
        let (content, _) = read_config(path)?;
        let inspection = inspect_wrapper(&content)?;
        installed |= !inspection.blocks.is_empty();
        all_current &= current_wrapper_matches(&content, &inspection, wrapper);
        conflicting_wrapper |= inspection.conflicting_wrapper;
    }

    let mut stale_config_file = None;
    for legacy in &targets.legacy {
        let (legacy_content, _) = read_config(legacy)?;
        let inspection = inspect_wrapper(&legacy_content)?;
        conflicting_wrapper |= inspection.conflicting_wrapper;
        if stale_config_file.is_none() && !inspection.blocks.is_empty() {
            stale_config_file = Some(legacy.to_string_lossy().to_string());
        }
    }

    let warning = if shell_type == "Zsh" && detect_early_exit(&targets.primary) {
        Some(
            "~/.zshenv 含 return/exit 早退语句；其后的 wrapper 可能不会执行，运行时指令或不生效。"
                .to_string(),
        )
    } else {
        None
    };

    Ok(WrapperStatus {
        installed: installed || stale_config_file.is_some(),
        needs_upgrade: (installed && !all_current) || stale_config_file.is_some(),
        conflicting_wrapper,
        shell_type: Some(shell_type.to_string()),
        config_file: Some(targets.primary.to_string_lossy().to_string()),
        stale_config_file,
        warning,
    })
}

struct ConfigChange {
    path: PathBuf,
    target: PathBuf,
    original: Option<Vec<u8>>,
    updated: Vec<u8>,
}

/// 只读检查并准备变更；所有文件都检查成功之后才能开始写入。
fn plan_wrapper_change(
    path: &Path,
    replacement: Option<&str>,
    protect_external: bool,
) -> Result<Option<ConfigChange>, String> {
    let target = resolve_config_path(path)?;
    let original = match fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("读取配置文件 {} 失败: {error}", path.display())),
    };
    let (content, encoding) = decode_config(original.as_deref().unwrap_or_default())?;
    let inspection = inspect_wrapper(&content)?;
    if protect_external && inspection.conflicting_wrapper {
        return Err(format!(
            "检测到由 claude-keysmith 管理的 wrapper（{}）；为避免覆盖其 system prompt 配置，CC Switch 未执行安装",
            path.display()
        ));
    }

    let updated = if let Some(wrapper) = replacement {
        if current_wrapper_matches(&content, &inspection, wrapper) {
            return Ok(None);
        }
        let newline = detect_newline(&content);
        let wrapper = wrapper.replace('\n', newline);
        if inspection.blocks.is_empty() {
            let mut updated = content;
            if !updated.is_empty() {
                if !updated.ends_with(newline) {
                    updated.push_str(newline);
                }
                updated.push_str(newline);
            }
            updated.push_str(&wrapper);
            updated.push_str(newline);
            updated
        } else {
            replace_wrapper_blocks(&content, &inspection.blocks, Some(&wrapper))
        }
    } else {
        if inspection.blocks.is_empty() {
            return Ok(None);
        }
        replace_wrapper_blocks(&content, &inspection.blocks, None)
    };

    Ok(Some(ConfigChange {
        path: path.to_path_buf(),
        target,
        original,
        updated: encode_config(&updated, encoding),
    }))
}

fn apply_config_changes(changes: &[ConfigChange]) -> Result<(), String> {
    apply_config_changes_with(changes, |path, bytes| {
        atomic_write(path, bytes).map_err(|error| error.to_string())
    })
}

/// 单文件写入是原子的；后续文件失败时，恢复此前成功写入的文件。
fn apply_config_changes_with(
    changes: &[ConfigChange],
    mut write: impl FnMut(&Path, &[u8]) -> Result<(), String>,
) -> Result<(), String> {
    for change in changes {
        backup_config(&change.path)?;
    }
    for (index, change) in changes.iter().enumerate() {
        if let Err(error) = write(&change.target, &change.updated) {
            let mut rollback_errors = Vec::new();
            for previous in changes[..index].iter().rev() {
                let restored = if let Some(original) = &previous.original {
                    write(&previous.target, original)
                } else {
                    fs::remove_file(&previous.target).map_err(|error| error.to_string())
                };
                if let Err(rollback_error) = restored {
                    rollback_errors.push(format!("{}: {rollback_error}", previous.path.display()));
                }
            }
            let detail = if rollback_errors.is_empty() {
                "已恢复此前的配置".to_string()
            } else {
                format!(
                    "部分配置恢复失败，请使用备份恢复: {}",
                    rollback_errors.join("、")
                )
            };
            return Err(format!(
                "写入配置文件 {} 失败: {error}；{detail}",
                change.path.display()
            ));
        }
    }
    Ok(())
}

// Kept small and private for the focused regression tests. Production installs use the
// multi-file transaction above so every target is checked before any file is changed.
#[cfg(test)]
fn write_wrapper_into(path: &Path, shell_type: &str, wrapper: &str) -> Result<bool, String> {
    let change = plan_wrapper_change(path, Some(wrapper), true)?;
    let Some(change) = change else {
        return Ok(false);
    };
    let _ = shell_type;
    apply_config_changes(std::slice::from_ref(&change))?;
    Ok(true)
}

#[cfg(test)]
fn remove_wrapper_from(path: &Path) -> Result<bool, String> {
    let change = plan_wrapper_change(path, None, false)?;
    let Some(change) = change else {
        return Ok(false);
    };
    apply_config_changes(std::slice::from_ref(&change))?;
    Ok(true)
}

#[tauri::command]
pub fn install_shell_wrapper() -> Result<String, String> {
    let (targets, shell_type) = get_shell_targets()?;
    let append_prompt_path = get_append_prompt_path()?;
    let system_prompt_path = get_system_prompt_path()?;
    let wrapper = generate_wrapper_code(&shell_type, &append_prompt_path, &system_prompt_path)?;

    install_wrapper_at(&targets, &wrapper)
}

fn install_wrapper_at(targets: &ShellTargets, wrapper: &str) -> Result<String, String> {
    let mut changes = Vec::new();
    for path in targets.install_paths() {
        if let Some(change) = plan_wrapper_change(path, Some(wrapper), true)? {
            changes.push(change);
        }
    }
    let mut cleaned: Vec<String> = Vec::new();
    for legacy in &targets.legacy {
        if let Some(change) = plan_wrapper_change(legacy, None, true)? {
            changes.push(change);
            cleaned.push(legacy.to_string_lossy().to_string());
        }
    }

    if changes.is_empty() {
        return Err("Shell wrapper 已安装".to_string());
    }
    apply_config_changes(&changes)?;

    let mut result = targets
        .install_paths()
        .map(|path| path.to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("、");
    if !cleaned.is_empty() {
        result.push_str(&format!("（已清理旧配置 {}）", cleaned.join("、")));
    }
    Ok(result)
}

#[tauri::command]
pub fn uninstall_shell_wrapper() -> Result<String, String> {
    let (targets, _) = get_shell_targets()?;
    uninstall_wrapper_at(&targets)
}

fn uninstall_wrapper_at(targets: &ShellTargets) -> Result<String, String> {
    let mut changes = Vec::new();
    for path in targets.install_paths().chain(&targets.legacy) {
        if let Some(change) = plan_wrapper_change(path, None, false)? {
            changes.push(change);
        }
    }
    if changes.is_empty() {
        return Err("Shell wrapper 未安装".to_string());
    }
    apply_config_changes(&changes)?;
    Ok(changes
        .iter()
        .map(|change| change.path.to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("、"))
}

#[tauri::command]
pub fn get_shell_wrapper_instructions() -> Result<String, String> {
    let (targets, shell_type) = get_shell_targets()?;
    let append_prompt_path = get_append_prompt_path()?;
    let system_prompt_path = get_system_prompt_path()?;
    let wrapper_code =
        generate_wrapper_code(&shell_type, &append_prompt_path, &system_prompt_path)?;
    let config_path_text = targets.primary.to_string_lossy();
    let reload_command = if shell_type == "PowerShell" {
        format!(". {}", quote_powershell(&config_path_text))
    } else {
        format!("source {}", quote_posix(&config_path_text))
    };
    let placement_note = match shell_type.as_str() {
        "Zsh" => "\n# 说明\n写入 ~/.zshenv 可覆盖正常加载启动文件的交互和非交互 zsh；\n写入 ~/.zshrc 只覆盖交互式 zsh。直接执行 Claude 二进制会跳过 shell wrapper。",
        "Bash" => "\n# 说明\n将上述代码写入列出的每个配置文件，分别覆盖登录和交互非登录 Bash。",
        _ => "",
    };
    let config_paths = targets
        .install_paths()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join("\n");

    Ok(format!(
        "# Shell Wrapper ({shell_type})\n\n# 配置文件\n{}\n\n# 添加以下代码\n{}\n\n# 重新加载\n{}\n\n# 或者重启终端{}",
        config_paths,
        wrapper_code,
        reload_command,
        placement_note
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrappers_skip_the_flag_when_the_append_file_is_empty() {
        let append_path = Path::new("/tmp/append-prompt.md");
        let system_path = Path::new("/tmp/system-prompt.md");
        let posix = generate_posix_wrapper(append_path, system_path);
        assert!(posix.contains("if [ -s \"$append_prompt_file\" ]"));
        assert!(posix.contains("if [ -s \"$system_prompt_file\" ]"));
        assert!(posix.contains("local -a extra_args=()"));
        assert!(posix.contains("command claude \"${extra_args[@]}\" \"$@\""));
        assert!(!posix.contains("eval "));

        let fish = generate_fish_wrapper(append_path, system_path);
        assert!(fish.contains("if test -s \"$append_prompt_file\""));
        assert!(fish.contains("if test -s \"$system_prompt_file\""));
        assert!(fish.contains("command claude $argv"));

        let powershell = generate_powershell_wrapper(append_path, system_path);
        assert!(powershell.contains("-CommandType Application"));
        assert!(powershell.contains(").Path"));
        assert!(powershell.contains("& $script:CCSwitchClaudeExecutable @args"));
    }

    #[test]
    fn wrappers_defer_to_user_supplied_append_prompt_arguments() {
        let append_path = Path::new("/tmp/append-prompt.md");
        let system_path = Path::new("/tmp/system-prompt.md");

        let posix = generate_posix_wrapper(append_path, system_path);
        assert!(posix.contains("has_append_prompt_arg=0"));
        assert!(posix.contains("has_system_prompt_arg=0"));
        assert!(posix.contains("--append-system-prompt-file=*"));
        assert!(posix.contains("--system-prompt-file=*"));
        assert!(posix.contains("[ \"$has_append_prompt_arg\" -eq 0 ]"));
        assert!(posix.contains("[ \"$has_system_prompt_arg\" -eq 0 ]"));

        let fish = generate_fish_wrapper(append_path, system_path);
        assert!(fish.contains("set -l has_append_prompt_arg 0"));
        assert!(fish.contains("set -l has_system_prompt_arg 0"));
        assert!(fish.contains("'--append-system-prompt-file=*'"));
        assert!(fish.contains("'--system-prompt-file=*'"));
        assert!(fish.contains("test $has_append_prompt_arg -eq 0"));
        assert!(fish.contains("test $has_system_prompt_arg -eq 0"));

        let powershell = generate_powershell_wrapper(append_path, system_path);
        assert!(powershell.contains("$hasAppendPromptArgument = $false"));
        assert!(powershell.contains("$hasSystemPromptArgument = $false"));
        assert!(powershell.contains("$argument -eq \"--append-system-prompt\""));
        assert!(powershell.contains("$argument -eq \"--system-prompt\""));
        assert!(powershell.contains("$argument -like \"--append-system-prompt-file=*\""));
        assert!(powershell.contains("$argument -like \"--system-prompt-file=*\""));
        assert!(powershell.contains("-not $hasAppendPromptArgument"));
        assert!(powershell.contains("-not $hasSystemPromptArgument"));
    }

    #[test]
    fn wrapper_inspection_rejects_partial_markers() {
        assert!(inspect_wrapper(WRAPPER_START).is_err());
        assert!(inspect_wrapper(WRAPPER_END).is_err());
        assert!(inspect_wrapper(LEGACY_CC_SWITCH_START).is_err());

        let inspection = inspect_wrapper("plain config").unwrap();
        assert!(inspection.blocks.is_empty());
        assert!(!inspection.conflicting_wrapper);
    }

    #[test]
    fn legacy_cc_switch_wrapper_can_be_upgraded() {
        let legacy = format!(
            "before\n{LEGACY_CC_SWITCH_START}\nclaude() {{\n  command claude --append-system-prompt-file file \"$@\"\n}}\n{LEGACY_KEYSMITH_END}\nafter\n"
        );
        let inspection = inspect_wrapper(&legacy).expect("inspect legacy wrapper");
        assert_eq!(inspection.blocks.len(), 1);
        assert_eq!(inspection.blocks[0].kind, WrapperKind::Legacy);
        assert!(!inspection.conflicting_wrapper);

        let upgraded = replace_wrapper_blocks(
            &legacy,
            &inspection.blocks,
            Some(&generate_posix_wrapper(
                Path::new("/tmp/append-prompt.md"),
                Path::new("/tmp/system-prompt.md"),
            )),
        );
        assert_eq!(upgraded.matches(WRAPPER_START).count(), 1);
        assert!(!upgraded.contains(LEGACY_CC_SWITCH_START));
        assert!(upgraded.starts_with("before\n"));
        assert!(upgraded.ends_with("\nafter\n"));
    }

    #[test]
    fn append_only_legacy_keysmith_marker_is_managed_but_full_keysmith_is_not() {
        let append_only = format!(
            "{LEGACY_KEYSMITH_START}\nclaude() {{ command claude --append-system-prompt-file file \"$@\"; }}\n{LEGACY_KEYSMITH_END}"
        );
        let inspection = inspect_wrapper(&append_only).expect("inspect append-only wrapper");
        assert_eq!(inspection.blocks.len(), 1);
        assert_eq!(inspection.blocks[0].kind, WrapperKind::Legacy);
        assert!(!inspection.conflicting_wrapper);

        let keysmith = format!(
            "{LEGACY_KEYSMITH_START}\n# Managed by claude-keysmith. Do not edit by hand.\nclaude() {{ command claude --system-prompt-file system --append-system-prompt-file append \"$@\"; }}\n{LEGACY_KEYSMITH_END}"
        );
        let inspection = inspect_wrapper(&keysmith).expect("inspect keysmith wrapper");
        assert!(inspection.blocks.is_empty());
        assert!(inspection.conflicting_wrapper);
    }

    #[test]
    fn custom_append_path_is_quoted_for_each_shell() {
        let append_path = Path::new("/tmp/CC Switch/append'prompt.md");
        let system_path = Path::new("/tmp/CC Switch/system'prompt.md");

        let posix = generate_posix_wrapper(append_path, system_path);
        assert!(posix.contains("'/tmp/CC Switch/append'\"'\"'prompt.md'"));
        assert!(posix.contains("'/tmp/CC Switch/system'\"'\"'prompt.md'"));

        let fish = generate_fish_wrapper(append_path, system_path);
        assert!(fish.contains("'/tmp/CC Switch/append\\'prompt.md'"));
        assert!(fish.contains("'/tmp/CC Switch/system\\'prompt.md'"));

        let powershell = generate_powershell_wrapper(append_path, system_path);
        assert!(powershell.contains("'/tmp/CC Switch/append''prompt.md'"));
        assert!(powershell.contains("'/tmp/CC Switch/system''prompt.md'"));
    }

    #[test]
    fn wrapper_status_serializes_as_camel_case() {
        let status = WrapperStatus {
            installed: true,
            needs_upgrade: true,
            conflicting_wrapper: false,
            shell_type: Some("Zsh".to_string()),
            config_file: Some("/tmp/.zshenv".to_string()),
            stale_config_file: Some("/tmp/.zshrc".to_string()),
            warning: None,
        };
        let value = serde_json::to_value(status).expect("serialize wrapper status");

        assert_eq!(value["needsUpgrade"], true);
        assert_eq!(value["conflictingWrapper"], false);
        assert_eq!(value["shellType"], "Zsh");
        assert_eq!(value["configFile"], "/tmp/.zshenv");
        assert_eq!(value["staleConfigFile"], "/tmp/.zshrc");
        assert!(value.get("needs_upgrade").is_none());
    }

    #[test]
    fn config_encoding_round_trips() {
        let text = "# profile\r\nWrite-Output 'ok'\r\n";
        for encoding in [
            TextEncoding::Utf8,
            TextEncoding::Utf8Bom,
            TextEncoding::Utf16Le,
            TextEncoding::Utf16Be,
        ] {
            let encoded = encode_config(text, encoding);
            let (decoded, _) = decode_config(&encoded).expect("decode config");
            assert_eq!(decoded, text);
        }
    }

    // Regression: zsh must inject via ~/.zshenv (read by every zsh, including the
    // login non-interactive shell that GUIs use), and .zshrc becomes a legacy path
    // that install cleans up. Before the fix, only .zshrc was written, so a claude
    // launched from the Dock never received the runtime instructions.
    #[cfg(not(target_os = "windows"))]
    fn zsh_targets_with_test_home() -> (std::path::PathBuf, Vec<std::path::PathBuf>) {
        let home = tempfile::tempdir().expect("temp home");
        let home_path = home.path().to_path_buf();
        // get_home_dir() consults CC_SWITCH_TEST_HOME; keep the guard alive.
        std::env::set_var("CC_SWITCH_TEST_HOME", &home_path);
        std::env::set_var("SHELL", "/bin/zsh");
        std::mem::forget(home); // keep the dir alive for the caller
        let (targets, shell_type) = get_shell_targets().expect("zsh targets");
        assert_eq!(shell_type, "Zsh");
        (targets.primary, targets.legacy)
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn zsh_writes_to_zshenv_and_cleans_stale_zshrc() {
        let (primary, legacy) = zsh_targets_with_test_home();
        assert!(
            primary.ends_with(".zshenv"),
            "zsh primary must be ~/.zshenv, got {primary:?}"
        );
        assert!(
            legacy.iter().any(|p| p.ends_with(".zshrc")),
            "~/.zshrc must be listed as a legacy target to clean up"
        );

        // Simulate the pre-fix state: old wrapper only in .zshrc.
        let zshrc = legacy
            .iter()
            .find(|p| p.ends_with(".zshrc"))
            .expect(".zshrc legacy path")
            .clone();
        let wrapper = generate_posix_wrapper(
            Path::new("/tmp/append-prompt.md"),
            Path::new("/tmp/system-prompt.md"),
        );
        fs::create_dir_all(zshrc.parent().unwrap()).unwrap();
        fs::write(&zshrc, format!("{wrapper}\n")).unwrap();

        // Install writes the primary (.zshenv)...
        assert!(write_wrapper_into(&primary, "Zsh", &wrapper).expect("install primary"));
        // ...and the stale .zshrc block is removed.
        assert!(remove_wrapper_from(&zshrc).expect("clean legacy"));

        let (zshenv_text, _) = read_config(&primary).unwrap();
        assert_eq!(zshenv_text.matches(WRAPPER_START).count(), 1);
        let (zshrc_text, _) = read_config(&zshrc).unwrap();
        assert!(
            !zshrc_text.contains(WRAPPER_START),
            "stale .zshrc wrapper must be gone, got: {zshrc_text:?}"
        );

        // Idempotent: writing the same wrapper again is a no-op.
        assert!(!write_wrapper_into(&primary, "Zsh", &wrapper).expect("idempotent"));
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn zshenv_with_early_return_is_reported_as_warning() {
        let (primary, _legacy) = zsh_targets_with_test_home();
        fs::create_dir_all(primary.parent().unwrap()).unwrap();
        fs::write(&primary, "return\n# wrapper would never run\n").unwrap();
        assert!(
            detect_early_exit(&primary),
            "bare `return` in .zshenv must be flagged"
        );
        fs::write(&primary, "# a comment mentioning return\nexport FOO=1\n").unwrap();
        assert!(
            !detect_early_exit(&primary),
            "`return` inside a comment must not be flagged"
        );
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn bash_installs_into_both_login_and_interactive_files() {
        let home = tempfile::tempdir().expect("temp home");
        let (targets, shell_type) = shell_targets_for(home.path(), "bash").expect("bash targets");
        assert_eq!(shell_type, "Bash");
        assert_eq!(targets.install_paths().count(), 2);
        let wrapper = generate_posix_wrapper(
            Path::new("/tmp/append-prompt.md"),
            Path::new("/tmp/system-prompt.md"),
        );
        let result = install_wrapper_at(&targets, &wrapper).expect("install bash wrappers");
        assert!(result.contains(".bash_profile"));
        assert!(result.contains(".bashrc"));
        for path in targets.install_paths() {
            let content = fs::read_to_string(path).expect("read bash wrapper");
            assert_eq!(content.matches(WRAPPER_START).count(), 1);
        }
        let status = check_wrapper_status_at(&targets, "Bash", &wrapper).expect("status");
        assert!(status.installed);
        assert!(!status.needs_upgrade);
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn external_wrapper_in_legacy_file_is_reported_before_install() {
        let home = tempfile::tempdir().expect("temp home");
        let (targets, shell_type) = shell_targets_for(home.path(), "zsh").expect("zsh targets");
        let legacy = &targets.legacy[0];
        let external = format!(
            "{LEGACY_KEYSMITH_START}\n# Managed by claude-keysmith. Do not edit by hand.\nclaude() {{ command claude --system-prompt-file external \"$@\"; }}\n{LEGACY_KEYSMITH_END}\n"
        );
        fs::write(legacy, external).expect("write external wrapper");
        let wrapper = generate_wrapper_code(
            "Zsh",
            Path::new("/tmp/append-prompt.md"),
            Path::new("/tmp/system-prompt.md"),
        )
        .expect("generate wrapper");
        let status = check_wrapper_status_at(&targets, &shell_type, &wrapper).expect("status");
        assert!(status.conflicting_wrapper);
        assert!(install_wrapper_at(&targets, &wrapper).is_err());
        assert!(
            !targets.primary.exists(),
            "primary must stay untouched on conflict"
        );
    }

    #[test]
    fn multi_file_install_rolls_back_when_a_later_write_fails() {
        let temp = tempfile::tempdir().expect("temp");
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        let wrapper = generate_posix_wrapper(
            Path::new("/tmp/append-prompt.md"),
            Path::new("/tmp/system-prompt.md"),
        );
        let first_change = plan_wrapper_change(&first, Some(&wrapper), true)
            .expect("plan first")
            .expect("first change");
        let second_change = plan_wrapper_change(&second, Some(&wrapper), true)
            .expect("plan second")
            .expect("second change");
        let changes = vec![first_change, second_change];
        let mut writes = 0;
        let result = apply_config_changes_with(&changes, |path, bytes| {
            writes += 1;
            if writes == 2 {
                return Err("injected write failure".to_string());
            }
            fs::write(path, bytes).map_err(|error| error.to_string())
        });
        assert!(result.is_err());
        assert!(!first.exists(), "first file must be rolled back");
        assert!(!second.exists(), "failed file must remain absent");
    }
}
