use crate::claude_append_instructions::runtime_projection_path;
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WrapperKind {
    Current,
    Legacy,
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

fn get_shell_config_path() -> Result<(PathBuf, String), String> {
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

        if windows_powershell.exists() && !powershell.exists() {
            Ok((windows_powershell, "PowerShell".to_string()))
        } else {
            Ok((powershell, "PowerShell".to_string()))
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        let shell = std::env::var("SHELL").unwrap_or_default();
        let shell_name = Path::new(&shell)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();

        match shell_name {
            "zsh" => Ok((home.join(".zshrc"), "Zsh".to_string())),
            "bash" => {
                #[cfg(target_os = "macos")]
                let path = home.join(".bash_profile");
                #[cfg(not(target_os = "macos"))]
                let path = home.join(".bashrc");
                Ok((path, "Bash".to_string()))
            }
            "fish" => Ok((
                home.join(".config").join("fish").join("config.fish"),
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
}

fn get_append_prompt_path() -> Result<PathBuf, String> {
    Ok(runtime_projection_path())
}

fn is_default_append_prompt_path(path: &Path) -> bool {
    path == get_home_dir()
        .join(".claude")
        .join("cc-switch")
        .join("append-prompt.md")
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

fn fish_append_path_expression(path: &Path) -> String {
    if is_default_append_prompt_path(path) {
        "\"$HOME/.claude/cc-switch/append-prompt.md\"".to_string()
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

fn generate_posix_wrapper(append_prompt_path: &Path) -> String {
    let path_expression = posix_append_path_expression(append_prompt_path);
    format!(
        r#"{WRAPPER_START}
claude() {{
    local append_prompt_file={path_expression}
    local has_append_prompt_arg=0
    local arg
    for arg in "$@"; do
        case "$arg" in
            --)
                break
                ;;
            --append-system-prompt|--append-system-prompt=*|--append-system-prompt-file|--append-system-prompt-file=*)
                has_append_prompt_arg=1
                break
                ;;
        esac
    done
    if [ -s "$append_prompt_file" ] && [ "$has_append_prompt_arg" -eq 0 ]; then
        command claude --append-system-prompt-file "$append_prompt_file" "$@"
    else
        command claude "$@"
    fi
}}
{WRAPPER_END}"#
    )
}

fn generate_fish_wrapper(append_prompt_path: &Path) -> String {
    let path_expression = fish_append_path_expression(append_prompt_path);
    format!(
        r#"{WRAPPER_START}
function claude
    set -l append_prompt_file {path_expression}
    set -l has_append_prompt_arg 0
    for arg in $argv
        switch $arg
            case '--'
                break
            case '--append-system-prompt' '--append-system-prompt=*' '--append-system-prompt-file' '--append-system-prompt-file=*'
                set has_append_prompt_arg 1
                break
        end
    end
    if test -s "$append_prompt_file"; and test $has_append_prompt_arg -eq 0
        command claude --append-system-prompt-file "$append_prompt_file" $argv
    else
        command claude $argv
    end
end
{WRAPPER_END}"#
    )
}

fn generate_powershell_wrapper(append_prompt_path: &Path) -> String {
    let path_expression = powershell_append_path_expression(append_prompt_path);
    format!(
        r#"{WRAPPER_START}
$script:CCSwitchClaudeExecutable = (Get-Command claude -CommandType Application -ErrorAction Stop).Path
function global:claude {{
    $appendPromptFile = {path_expression}
    $hasAppendPromptArgument = $false
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
            break
        }}
    }}
    if ((Test-Path -LiteralPath $appendPromptFile -PathType Leaf) -and ((Get-Item -LiteralPath $appendPromptFile).Length -gt 0) -and -not $hasAppendPromptArgument) {{
        & $script:CCSwitchClaudeExecutable --append-system-prompt-file $appendPromptFile @args
    }} else {{
        & $script:CCSwitchClaudeExecutable @args
    }}
}}
{WRAPPER_END}"#
    )
}

fn generate_wrapper_code(shell_type: &str, append_prompt_path: &Path) -> Result<String, String> {
    match shell_type {
        "Zsh" | "Bash" => Ok(generate_posix_wrapper(append_prompt_path)),
        "Fish" => Ok(generate_fish_wrapper(append_prompt_path)),
        "PowerShell" => Ok(generate_powershell_wrapper(append_prompt_path)),
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

fn write_config(path: &Path, content: &str, encoding: TextEncoding) -> Result<(), String> {
    let target = if path.exists()
        && fs::symlink_metadata(path)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
    {
        fs::canonicalize(path).map_err(|e| format!("解析配置文件软链接失败: {e}"))?
    } else {
        path.to_path_buf()
    };

    atomic_write(&target, &encode_config(content, encoding))
        .map_err(|e| format!("写入配置文件失败: {e}"))
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
    let (config_path, shell_type) = get_shell_config_path()?;
    let (content, _) = read_config(&config_path)?;
    let inspection = inspect_wrapper(&content)?;
    let installed = !inspection.blocks.is_empty();
    let append_prompt_path = get_append_prompt_path()?;
    let expected_wrapper = generate_wrapper_code(&shell_type, &append_prompt_path)?
        .replace('\n', detect_newline(&content));
    let current_wrapper_matches = inspection.blocks.len() == 1
        && inspection.blocks[0].kind == WrapperKind::Current
        && &content[inspection.blocks[0].start..inspection.blocks[0].end]
            == expected_wrapper.as_str();
    let needs_upgrade = installed && !current_wrapper_matches;

    Ok(WrapperStatus {
        installed,
        needs_upgrade,
        conflicting_wrapper: inspection.conflicting_wrapper,
        shell_type: Some(shell_type),
        config_file: Some(config_path.to_string_lossy().to_string()),
    })
}

#[tauri::command]
pub fn install_shell_wrapper() -> Result<String, String> {
    let (config_path, shell_type) = get_shell_config_path()?;
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {e}"))?;
    }

    let (existing_content, encoding) = read_config(&config_path)?;
    let inspection = inspect_wrapper(&existing_content)?;
    if inspection.conflicting_wrapper {
        return Err(
            "检测到由 claude-keysmith 管理的 wrapper；为避免覆盖其 system prompt 配置，CC Switch 未执行安装"
                .to_string(),
        );
    }
    let newline = detect_newline(&existing_content);
    let append_prompt_path = get_append_prompt_path()?;
    let wrapper = generate_wrapper_code(&shell_type, &append_prompt_path)?.replace('\n', newline);
    let current_wrapper_matches = inspection.blocks.len() == 1
        && inspection.blocks[0].kind == WrapperKind::Current
        && &existing_content[inspection.blocks[0].start..inspection.blocks[0].end]
            == wrapper.as_str();
    if current_wrapper_matches {
        return Err("Shell wrapper 已安装".to_string());
    }

    backup_config(&config_path)?;
    let new_content = if inspection.blocks.is_empty() {
        let mut content = existing_content;
        if !content.is_empty() {
            if !content.ends_with(newline) {
                content.push_str(newline);
            }
            content.push_str(newline);
        }
        content.push_str(&wrapper);
        content.push_str(newline);
        content
    } else {
        replace_wrapper_blocks(&existing_content, &inspection.blocks, Some(&wrapper))
    };
    write_config(&config_path, &new_content, encoding)?;

    Ok(config_path.to_string_lossy().to_string())
}

#[tauri::command]
pub fn uninstall_shell_wrapper() -> Result<String, String> {
    let (config_path, _) = get_shell_config_path()?;
    if !config_path.exists() {
        return Err("配置文件不存在".to_string());
    }

    let (content, encoding) = read_config(&config_path)?;
    let inspection = inspect_wrapper(&content)?;
    if inspection.blocks.is_empty() {
        return Err("Shell wrapper 未安装".to_string());
    }

    backup_config(&config_path)?;
    let new_content = replace_wrapper_blocks(&content, &inspection.blocks, None);
    write_config(&config_path, &new_content, encoding)?;

    Ok(config_path.to_string_lossy().to_string())
}

#[tauri::command]
pub fn get_shell_wrapper_instructions() -> Result<String, String> {
    let (config_path, shell_type) = get_shell_config_path()?;
    let append_prompt_path = get_append_prompt_path()?;
    let wrapper_code = generate_wrapper_code(&shell_type, &append_prompt_path)?;
    let config_path_text = config_path.to_string_lossy();
    let reload_command = if shell_type == "PowerShell" {
        format!(". {}", quote_powershell(&config_path_text))
    } else {
        format!("source {}", quote_posix(&config_path_text))
    };

    Ok(format!(
        "# Shell Wrapper ({shell_type})\n\n# 配置文件\n{}\n\n# 添加以下代码\n{}\n\n# 重新加载\n{}\n\n# 或者重启终端",
        config_path.display(),
        wrapper_code,
        reload_command
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrappers_skip_the_flag_when_the_append_file_is_empty() {
        let append_path = Path::new("/tmp/append-prompt.md");
        let posix = generate_posix_wrapper(append_path);
        assert!(posix.contains("if [ -s \"$append_prompt_file\" ]"));
        assert!(posix.contains("command claude \"$@\""));

        let fish = generate_fish_wrapper(append_path);
        assert!(fish.contains("if test -s \"$append_prompt_file\""));
        assert!(fish.contains("command claude $argv"));

        let powershell = generate_powershell_wrapper(append_path);
        assert!(powershell.contains("-CommandType Application"));
        assert!(powershell.contains(").Path"));
        assert!(powershell.contains("& $script:CCSwitchClaudeExecutable @args"));
    }

    #[test]
    fn wrappers_defer_to_user_supplied_append_prompt_arguments() {
        let append_path = Path::new("/tmp/append-prompt.md");

        let posix = generate_posix_wrapper(append_path);
        assert!(posix.contains("has_append_prompt_arg=0"));
        assert!(posix.contains("--append-system-prompt-file=*"));
        assert!(posix.contains("[ \"$has_append_prompt_arg\" -eq 0 ]"));

        let fish = generate_fish_wrapper(append_path);
        assert!(fish.contains("set -l has_append_prompt_arg 0"));
        assert!(fish.contains("'--append-system-prompt-file=*'"));
        assert!(fish.contains("test $has_append_prompt_arg -eq 0"));

        let powershell = generate_powershell_wrapper(append_path);
        assert!(powershell.contains("$hasAppendPromptArgument = $false"));
        assert!(powershell.contains("$argument -eq \"--append-system-prompt\""));
        assert!(powershell.contains("$argument -like \"--append-system-prompt-file=*\""));
        assert!(powershell.contains("-not $hasAppendPromptArgument"));
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
            Some(&generate_posix_wrapper(Path::new("/tmp/append-prompt.md"))),
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
        let path = Path::new("/tmp/CC Switch/append'prompt.md");

        let posix = generate_posix_wrapper(path);
        assert!(posix.contains("'/tmp/CC Switch/append'\"'\"'prompt.md'"));

        let fish = generate_fish_wrapper(path);
        assert!(fish.contains("'/tmp/CC Switch/append\\'prompt.md'"));

        let powershell = generate_powershell_wrapper(path);
        assert!(powershell.contains("'/tmp/CC Switch/append''prompt.md'"));
    }

    #[test]
    fn wrapper_status_serializes_as_camel_case() {
        let status = WrapperStatus {
            installed: true,
            needs_upgrade: true,
            conflicting_wrapper: false,
            shell_type: Some("Zsh".to_string()),
            config_file: Some("/tmp/.zshrc".to_string()),
        };
        let value = serde_json::to_value(status).expect("serialize wrapper status");

        assert_eq!(value["needsUpgrade"], true);
        assert_eq!(value["conflictingWrapper"], false);
        assert_eq!(value["shellType"], "Zsh");
        assert_eq!(value["configFile"], "/tmp/.zshrc");
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
}
