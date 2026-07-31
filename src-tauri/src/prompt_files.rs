use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

use crate::app_config::AppType;
use crate::codex_config::get_codex_auth_path;
use crate::config::get_claude_settings_path;
use crate::error::AppError;
use crate::gemini_config::get_gemini_dir;
use crate::openclaw_config::get_openclaw_dir;
use crate::opencode_config::get_opencode_dir;

const CLAUDE_MANAGED_IMPORT_START: &str = "<!-- cc-switch:prompt:start -->";
const CLAUDE_MANAGED_IMPORT_END: &str = "<!-- cc-switch:prompt:end -->";
const CLAUDE_MANAGED_IMPORT_PREFIX: &str = "@cc-switch/prompts/";

#[derive(Debug, Clone, PartialEq, Eq)]
struct ClaudeManagedImportBlock {
    start: usize,
    end: usize,
    target: String,
}

/// 返回指定应用所使用的提示词文件路径。
pub fn prompt_file_path(app: &AppType) -> Result<PathBuf, AppError> {
    if matches!(app, AppType::ClaudeDesktop) {
        return Err(AppError::localized(
            "app.prompts_unsupported",
            "当前应用暂不支持 Prompts",
            "This app does not support Prompts",
        ));
    }

    let base_dir: PathBuf = match app {
        AppType::Claude => get_base_dir_with_fallback(get_claude_settings_path(), ".claude")?,
        AppType::Codex => get_base_dir_with_fallback(get_codex_auth_path(), ".codex")?,
        AppType::Gemini => get_gemini_dir(),
        AppType::GrokBuild => crate::grok_config::get_grok_config_dir(),
        AppType::OpenCode => get_opencode_dir(),
        AppType::OpenClaw => get_openclaw_dir(),
        AppType::Hermes => crate::hermes_config::get_hermes_dir(),
        AppType::ClaudeDesktop => unreachable!("handled above"),
    };

    let filename = match app {
        AppType::Claude => "CLAUDE.md",
        AppType::Codex => "AGENTS.md",
        AppType::Gemini => "GEMINI.md",
        AppType::GrokBuild | AppType::OpenCode | AppType::OpenClaw | AppType::Hermes => "AGENTS.md",
        AppType::ClaudeDesktop => unreachable!("handled above"),
    };

    Ok(base_dir.join(filename))
}

/// 返回 CC Switch 管理的 Claude Code append-prompt 文件路径
/// (~/.claude/cc-switch/append-prompt.md)
/// 仅对 Claude 应用有效，其他应用返回 None
pub fn append_prompt_file_path(app: &AppType) -> Result<Option<PathBuf>, AppError> {
    if !matches!(app, AppType::Claude) {
        return Ok(None);
    }

    let base_dir = get_base_dir_with_fallback(get_claude_settings_path(), ".claude")?;
    // Keep the generated prompt separate from third-party tools such as
    // claude-keysmith. Their files may use the same CLI flag but must remain
    // under their own ownership.
    let cc_switch_dir = base_dir.join("cc-switch");
    Ok(Some(cc_switch_dir.join("append-prompt.md")))
}

fn claude_config_dir() -> Result<PathBuf, AppError> {
    prompt_file_path(&AppType::Claude)?
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| AppError::Config("Claude 配置目录无效".to_string()))
}

fn managed_prompt_filename(prompt_id: &str) -> String {
    let slug = prompt_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    let slug = slug
        .trim_matches(|character| matches!(character, '-' | '.'))
        .chars()
        .take(48)
        .collect::<String>();
    let slug = if slug.is_empty() {
        "prompt"
    } else {
        slug.as_str()
    };
    let digest = format!("{:x}", Sha256::digest(prompt_id.as_bytes()));
    format!("{slug}-{}.md", &digest[..12])
}

fn managed_prompt_target(prompt_id: &str) -> String {
    format!(
        "{CLAUDE_MANAGED_IMPORT_PREFIX}{}",
        managed_prompt_filename(prompt_id)
    )
}

pub fn claude_managed_prompt_file_path(prompt_id: &str) -> Result<PathBuf, AppError> {
    Ok(claude_config_dir()?
        .join("cc-switch")
        .join("prompts")
        .join(managed_prompt_filename(prompt_id)))
}

fn is_managed_prompt_filename(filename: &str) -> bool {
    let Some(stem) = filename.strip_suffix(".md") else {
        return false;
    };
    let Some((slug, digest)) = stem.rsplit_once('-') else {
        return false;
    };

    !slug.is_empty()
        && slug.len() <= 48
        && !matches!(slug.as_bytes().first(), Some(b'-' | b'.'))
        && slug
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        && digest.len() == 12
        && digest
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

pub fn claude_managed_prompt_path_from_target(target: &str) -> Result<PathBuf, AppError> {
    let filename = target
        .strip_prefix(CLAUDE_MANAGED_IMPORT_PREFIX)
        .ok_or_else(|| AppError::InvalidInput("CC Switch 提示词导入目标已被修改".to_string()))?;
    if !is_managed_prompt_filename(filename) {
        return Err(AppError::InvalidInput(
            "CC Switch 提示词导入目标无效".to_string(),
        ));
    }

    Ok(claude_config_dir()?
        .join("cc-switch")
        .join("prompts")
        .join(filename))
}

fn marker_index(content: &str, marker: &str) -> Result<Option<usize>, AppError> {
    let mut matches = content.match_indices(marker);
    let first = matches.next().map(|(index, _)| index);
    if matches.next().is_some() {
        return Err(AppError::InvalidInput(
            "检测到重复的 CC Switch 提示词导入标记".to_string(),
        ));
    }
    Ok(first)
}

fn marker_is_own_line(content: &str, start: usize, marker: &str) -> bool {
    let line_start = start == 0 || content.as_bytes().get(start.wrapping_sub(1)) == Some(&b'\n');
    let after = &content[start + marker.len()..];
    line_start && (after.is_empty() || after.starts_with('\n') || after.starts_with("\r\n"))
}

fn inspect_claude_managed_import(
    content: &str,
) -> Result<Option<ClaudeManagedImportBlock>, AppError> {
    let start = marker_index(content, CLAUDE_MANAGED_IMPORT_START)?;
    let end = marker_index(content, CLAUDE_MANAGED_IMPORT_END)?;

    let (start, end) = match (start, end) {
        (None, None) => return Ok(None),
        (Some(start), Some(end)) if end > start + CLAUDE_MANAGED_IMPORT_START.len() => (start, end),
        _ => {
            return Err(AppError::InvalidInput(
                "检测到残缺的 CC Switch 提示词导入块".to_string(),
            ))
        }
    };

    if !marker_is_own_line(content, start, CLAUDE_MANAGED_IMPORT_START)
        || !marker_is_own_line(content, end, CLAUDE_MANAGED_IMPORT_END)
    {
        return Err(AppError::InvalidInput(
            "CC Switch 提示词导入标记必须独占一行".to_string(),
        ));
    }

    let body = &content[start + CLAUDE_MANAGED_IMPORT_START.len()..end];
    let body_lines = body
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if body_lines.len() != 1 {
        return Err(AppError::InvalidInput(
            "CC Switch 提示词导入块内容无效".to_string(),
        ));
    }
    claude_managed_prompt_path_from_target(body_lines[0])?;

    let mut block_end = end + CLAUDE_MANAGED_IMPORT_END.len();
    if content[block_end..].starts_with("\r\n") {
        block_end += 2;
    } else if content[block_end..].starts_with('\n') {
        block_end += 1;
    }

    Ok(Some(ClaudeManagedImportBlock {
        start,
        end: block_end,
        target: body_lines[0].to_string(),
    }))
}

fn newline_for(content: &str) -> &'static str {
    if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

fn render_claude_managed_import(target: &str, newline: &str) -> String {
    [
        CLAUDE_MANAGED_IMPORT_START,
        target,
        CLAUDE_MANAGED_IMPORT_END,
        "",
    ]
    .join(newline)
}

pub fn ensure_claude_managed_import(
    content: &str,
    prompt_id: &str,
) -> Result<(String, Option<String>), AppError> {
    let existing = inspect_claude_managed_import(content)?;
    let previous_target = existing.as_ref().map(|block| block.target.clone());
    let newline = newline_for(content);
    let target = managed_prompt_target(prompt_id);
    let desired = render_claude_managed_import(&target, newline);

    if let Some(block) = existing {
        let mut updated =
            String::with_capacity(content.len() - (block.end - block.start) + desired.len());
        updated.push_str(&content[..block.start]);
        updated.push_str(&desired);
        updated.push_str(&content[block.end..]);
        return Ok((updated, previous_target));
    }

    let mut updated = content.to_string();
    if !updated.is_empty() {
        if !updated.ends_with('\n') {
            updated.push_str(newline);
        }
        let double_newline = format!("{newline}{newline}");
        if !updated.ends_with(double_newline.as_str()) {
            updated.push_str(newline);
        }
    }
    updated.push_str(&desired);
    Ok((updated, None))
}

pub fn remove_claude_managed_import(content: &str) -> Result<(String, Option<String>), AppError> {
    let Some(block) = inspect_claude_managed_import(content)? else {
        return Ok((content.to_string(), None));
    };

    let mut updated = String::with_capacity(content.len() - (block.end - block.start));
    updated.push_str(&content[..block.start]);
    updated.push_str(&content[block.end..]);
    Ok((updated, Some(block.target)))
}

pub fn read_live_prompt_content(app: &AppType) -> Result<Option<String>, AppError> {
    let memory_path = prompt_file_path(app)?;
    if !memory_path.exists() {
        return Ok(None);
    }

    let memory_content =
        std::fs::read_to_string(&memory_path).map_err(|error| AppError::io(&memory_path, error))?;
    if !matches!(app, AppType::Claude) {
        return Ok(Some(memory_content));
    }

    let Some(block) = inspect_claude_managed_import(&memory_content)? else {
        return Ok(Some(memory_content));
    };
    let managed_path = claude_managed_prompt_path_from_target(&block.target)?;
    if !managed_path.exists() {
        return Err(AppError::InvalidInput(format!(
            "CC Switch 管理的 Claude 提示词文件不存在: {}",
            managed_path.display()
        )));
    }
    std::fs::read_to_string(&managed_path)
        .map(Some)
        .map_err(|error| AppError::io(&managed_path, error))
}

fn get_base_dir_with_fallback(
    primary_path: PathBuf,
    fallback_dir: &str,
) -> Result<PathBuf, AppError> {
    primary_path
        .parent()
        .map(|p| p.to_path_buf())
        .or_else(|| dirs::home_dir().map(|h| h.join(fallback_dir)))
        .ok_or_else(|| {
            AppError::localized(
                "home_dir_not_found",
                format!("无法确定 {fallback_dir} 配置目录：用户主目录不存在"),
                format!("Cannot determine {fallback_dir} config directory: user home not found"),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_import_preserves_surrounding_content() {
        let source = "# Existing\n\nKeep this.\n";
        let (updated, previous) =
            ensure_claude_managed_import(source, "prompt-one").expect("insert block");

        assert!(updated.starts_with(source));
        assert!(updated.contains(CLAUDE_MANAGED_IMPORT_START));
        assert!(updated.contains("@cc-switch/prompts/prompt-one-"));
        assert_eq!(previous, None);
    }

    #[test]
    fn managed_import_switch_replaces_only_owned_block() {
        let (first, _) =
            ensure_claude_managed_import("before\nafter\n", "first").expect("insert first block");
        let (second, previous) =
            ensure_claude_managed_import(&first, "second").expect("replace block");

        assert!(second.starts_with("before\nafter\n"));
        assert!(second.contains("@cc-switch/prompts/second-"));
        assert!(!second.contains("@cc-switch/prompts/first-"));
        assert!(previous.is_some_and(|target| target.contains("/first-")));
    }

    #[test]
    fn managed_import_removal_keeps_user_content() {
        let (managed, _) =
            ensure_claude_managed_import("# User content\n", "active").expect("insert block");
        let (updated, removed) = remove_claude_managed_import(&managed).expect("remove block");

        assert_eq!(updated, "# User content\n\n");
        assert!(removed.is_some());
    }

    #[test]
    fn malformed_or_external_managed_blocks_fail_closed() {
        assert!(ensure_claude_managed_import(CLAUDE_MANAGED_IMPORT_START, "p").is_err());
        let external =
            format!("{CLAUDE_MANAGED_IMPORT_START}\n@../outside.md\n{CLAUDE_MANAGED_IMPORT_END}\n");
        assert!(remove_claude_managed_import(&external).is_err());
    }

    #[test]
    fn unmanaged_or_forged_prompt_targets_fail_closed() {
        for target in [
            "@cc-switch/prompts/user-notes.md",
            "@cc-switch/prompts/forged-0123456789xz.md",
            "@cc-switch/prompts/forged-ABCDEF012345.md",
            "@cc-switch/prompts/../owned-0123456789ab.md",
        ] {
            let block =
                format!("{CLAUDE_MANAGED_IMPORT_START}\n{target}\n{CLAUDE_MANAGED_IMPORT_END}\n");
            assert!(
                remove_claude_managed_import(&block).is_err(),
                "unexpectedly accepted {target}"
            );
        }

        let generated = managed_prompt_target("valid prompt");
        assert!(claude_managed_prompt_path_from_target(&generated).is_ok());
    }

    #[test]
    fn inline_markers_fail_closed() {
        let inline_start = format!(
            "prefix {CLAUDE_MANAGED_IMPORT_START}\n@cc-switch/prompts/prompt-0123456789ab.md\n{CLAUDE_MANAGED_IMPORT_END}\n"
        );
        let inline_end = format!(
            "{CLAUDE_MANAGED_IMPORT_START}\n@cc-switch/prompts/prompt-0123456789ab.md\n{CLAUDE_MANAGED_IMPORT_END} suffix\n"
        );

        assert!(remove_claude_managed_import(&inline_start).is_err());
        assert!(remove_claude_managed_import(&inline_end).is_err());
    }
}
