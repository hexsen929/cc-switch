//! Sync one user-owned instruction file into managed blocks in each agent's
//! instruction file. The source file remains the user's source of truth.

use std::path::{Path, PathBuf};

use crate::app_config::AppType;
use crate::config::{get_home_dir, write_text_file};
use crate::error::AppError;
use crate::prompt_files::prompt_file_path;

const BLOCK_START: &str = "<!-- cc-switch:shared-instructions:start -->";
const BLOCK_END: &str = "<!-- cc-switch:shared-instructions:end -->";

fn expand_path(raw: &str) -> PathBuf {
    let trimmed = raw.trim();
    if let Some(rest) = trimmed.strip_prefix("~/") {
        get_home_dir().join(rest)
    } else if trimmed == "~" {
        get_home_dir()
    } else {
        PathBuf::from(trimmed)
    }
}

fn managed_block(content: &str) -> String {
    format!("{BLOCK_START}\n{content}\n{BLOCK_END}")
}

/// Replace the managed block while preserving every byte outside it.
pub(crate) fn replace_managed_block(existing: &str, shared: &str) -> Result<String, AppError> {
    let start = existing.find(BLOCK_START);
    let end = existing.find(BLOCK_END);
    if start.is_some() != end.is_some() || end.is_some_and(|end| end < start.unwrap_or(0)) {
        return Err(AppError::InvalidInput(
            "共享指令文件的 CC Switch 管理块不完整，请先手动修复目标文件".to_string(),
        ));
    }

    let without = match (start, end) {
        (Some(start), Some(end)) => {
            let end = end + BLOCK_END.len();
            let mut prefix = &existing[..start];
            let mut suffix = &existing[end..];
            if prefix.ends_with("\n\n") {
                prefix = &prefix[..prefix.len() - 1];
            }
            if suffix.starts_with('\n') {
                suffix = &suffix[1..];
            }
            let mut value = String::with_capacity(existing.len());
            value.push_str(prefix);
            value.push_str(suffix);
            value
        }
        _ => existing.to_string(),
    };

    let shared = shared.trim_end();
    if shared.is_empty() {
        return Ok(without);
    }

    let mut result = without;
    if !result.is_empty() && !result.ends_with('\n') {
        result.push('\n');
    }
    if !result.is_empty() {
        result.push('\n');
    }
    result.push_str(&managed_block(shared));
    result.push('\n');
    Ok(result)
}

fn target_paths() -> Vec<PathBuf> {
    [
        AppType::Claude,
        AppType::Codex,
        AppType::Gemini,
        AppType::GrokBuild,
        AppType::OpenCode,
        AppType::OpenClaw,
        AppType::Hermes,
        AppType::Pi,
        AppType::Mcode,
    ]
    .into_iter()
    .filter_map(|app| prompt_file_path(&app).ok())
    .collect()
}

fn read_target(path: &Path) -> Result<String, AppError> {
    match std::fs::read_to_string(path) {
        Ok(value) => Ok(value),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(AppError::io(path, error)),
    }
}

/// Sync the configured source file. Empty configuration removes blocks that
/// CC Switch previously wrote, leaving user-authored content intact.
pub fn sync_from_settings() -> Result<(), AppError> {
    let source = crate::settings::get_settings()
        .shared_instruction_file
        .map(|path| expand_path(&path));
    let shared = match source.as_deref() {
        None => String::new(),
        Some(path) => {
            if !path.is_absolute() {
                return Err(AppError::InvalidInput(
                    "共享指令文件必须使用绝对路径或 ~/ 开头".to_string(),
                ));
            }
            if !path.is_file() {
                return Err(AppError::InvalidInput(format!(
                    "共享指令文件不存在或不是文件: {}",
                    path.display()
                )));
            }
            std::fs::read_to_string(path).map_err(|error| AppError::io(path, error))?
        }
    };

    let mut changes = Vec::new();
    for target in target_paths() {
        if source.as_deref().is_some_and(|path| {
            path.canonicalize()
                .ok()
                .zip(target.canonicalize().ok())
                .is_some_and(|(left, right)| left == right)
        }) {
            return Err(AppError::InvalidInput(format!(
                "共享指令文件不能直接指向 Agent 目标文件: {}",
                target.display()
            )));
        }
        let existing = read_target(&target)?;
        let updated = replace_managed_block(&existing, &shared)?;
        if updated != existing {
            changes.push((target, existing, updated));
        }
    }
    // Validate every target before writing any of them. Restore the files already
    // changed if a later write fails, so the settings save remains all-or-nothing.
    for (index, (target, _, updated)) in changes.iter().enumerate() {
        if let Err(error) = write_text_file(target, updated) {
            for (previous_target, previous, _) in changes[..index].iter().rev() {
                if let Err(rollback) = write_text_file(previous_target, previous) {
                    log::error!(
                        "恢复共享指令目标文件失败 {}: {rollback}",
                        previous_target.display()
                    );
                }
            }
            return Err(error);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_user_content_and_replaces_only_our_block() {
        let first = replace_managed_block("# user\n", "Use short answers.").unwrap();
        assert!(first.starts_with("# user\n"));
        assert!(first.contains("Use short answers."));
        let second = replace_managed_block(&first, "Use tests first.").unwrap();
        assert!(!second.contains("Use short answers."));
        assert!(second.contains("Use tests first."));
        let cleared = replace_managed_block(&second, "").unwrap();
        assert_eq!(cleared, "# user\n");
    }

    #[test]
    fn rejects_incomplete_blocks() {
        assert!(replace_managed_block(BLOCK_START, "x").is_err());
        assert!(replace_managed_block(BLOCK_END, "x").is_err());
    }
}
