//! A shared instruction overlay, separate from each application's ordinary Prompt.
//!
//! Native locks must be acquired before `PROJECTION_LOCK`: MCode, Pi, projection.
//! Ordinary Prompt writers take their own native lock and then projection; pure
//! compose/strip helpers never acquire either lock recursively.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use crate::app_config::AppType;
use crate::config::{get_home_dir, write_text_file};
use crate::error::AppError;
use crate::prompt_files::{prompt_file_path, validate_prompt_content};
use crate::services::pi_prompt_files::PiAgentsFileGuard;

const BLOCK_START: &str = "<!-- cc-switch:shared-instructions:start -->";
const BLOCK_END: &str = "<!-- cc-switch:shared-instructions:end -->";
#[cfg(test)]
const PI_MAX_BYTES: usize = 1024 * 1024;
static PROJECTION_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn lock_projection() -> Result<MutexGuard<'static, ()>, AppError> {
    PROJECTION_LOCK
        .lock()
        .map_err(|error| AppError::Message(format!("共享指令投影锁不可用: {error}")))
}

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

fn invalid_block() -> AppError {
    AppError::InvalidInput(
        "共享指令文件的 CC Switch 管理块不完整、重复或标记未独占一行，请先修复目标文件".to_string(),
    )
}

fn unique_marker(content: &str, marker: &str) -> Result<Option<usize>, AppError> {
    let mut matches = content.match_indices(marker);
    let first = matches.next().map(|(index, _)| index);
    if matches.next().is_some() {
        return Err(invalid_block());
    }
    Ok(first)
}

fn marker_is_own_line(content: &str, start: usize, marker: &str) -> bool {
    let at_start = start == 0 || content.as_bytes().get(start - 1) == Some(&b'\n');
    let after = &content[start + marker.len()..];
    at_start && (after.is_empty() || after.starts_with('\n') || after.starts_with("\r\n"))
}

/// The block owns its closing line ending, but never whitespace before or after
/// that line. New blocks are prefixed so stripping round-trips even a file with
/// no trailing newline, which Pi uses to determine its active ordinary Prompt.
fn block_range(existing: &str) -> Result<Option<std::ops::Range<usize>>, AppError> {
    match (
        unique_marker(existing, BLOCK_START)?,
        unique_marker(existing, BLOCK_END)?,
    ) {
        (None, None) => Ok(None),
        (Some(start), Some(end))
            if end > start + BLOCK_START.len()
                && marker_is_own_line(existing, start, BLOCK_START)
                && marker_is_own_line(existing, end, BLOCK_END) =>
        {
            let mut after = end + BLOCK_END.len();
            if existing[after..].starts_with("\r\n") {
                after += 2;
            } else if existing[after..].starts_with('\n') {
                after += 1;
            }
            Ok(Some(start..after))
        }
        _ => Err(invalid_block()),
    }
}

pub(crate) fn strip_managed_block(existing: &str) -> Result<String, AppError> {
    let Some(range) = block_range(existing)? else {
        return Ok(existing.to_string());
    };
    let mut plain = String::with_capacity(existing.len() - range.len());
    plain.push_str(&existing[..range.start]);
    plain.push_str(&existing[range.end..]);
    Ok(plain)
}

/// Recognize only the exact append layout written by releases through -93.
/// The padding is ambiguous without a known saved Prompt, so never trim it
/// heuristically; the caller provides the candidate ordinary content.
pub(crate) fn matches_legacy_prompt(existing: &str, plain: &str) -> Result<bool, AppError> {
    let Some(range) = block_range(existing)? else {
        return Ok(false);
    };
    if range.end != existing.len()
        || !existing[range.start + BLOCK_START.len()..].starts_with('\n')
        || !existing.ends_with(&format!("{BLOCK_END}\n"))
    {
        return Ok(false);
    }
    let mut prefix = plain.to_string();
    if !prefix.is_empty() && !prefix.ends_with('\n') {
        prefix.push('\n');
    }
    if !prefix.is_empty() {
        prefix.push('\n');
    }
    Ok(existing[..range.start] == prefix)
}

fn validate_shared_source(shared: &str) -> Result<(), AppError> {
    // The same target may also contain Claude's managed Prompt import. Never
    // let shared text impersonate any reserved CC Switch block namespace.
    if shared.contains("<!-- cc-switch:") {
        return Err(AppError::InvalidInput(
            "共享指令源文件不能包含 CC Switch 保留的托管标记".to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn replace_managed_block(existing: &str, shared: &str) -> Result<String, AppError> {
    validate_shared_source(shared)?;
    let range = block_range(existing)?;
    let shared = shared.trim_end_matches(['\r', '\n']);
    if shared.trim().is_empty() {
        return strip_managed_block(existing);
    }

    let crlf = match &range {
        Some(range) => existing[range.start + BLOCK_START.len()..].starts_with("\r\n"),
        None => existing.contains("\r\n"),
    };
    let newline = if crlf { "\r\n" } else { "\n" };
    let block = format!("{BLOCK_START}{newline}{shared}{newline}{BLOCK_END}{newline}");
    match range {
        Some(range) => {
            let mut updated = String::with_capacity(existing.len() + block.len());
            updated.push_str(&existing[..range.start]);
            updated.push_str(&block);
            updated.push_str(&existing[range.end..]);
            Ok(updated)
        }
        None => Ok(format!("{block}{existing}")),
    }
}

fn target_paths() -> Result<Vec<(AppType, PathBuf)>, AppError> {
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
        AppType::Kimi,
        AppType::Dsh,
        AppType::Agy,
        AppType::Omp,
    ]
    .into_iter()
    .map(|app| prompt_file_path(&app).map(|path| (app, path)))
    .collect()
}

fn read_target(path: &Path) -> Result<Option<String>, AppError> {
    match std::fs::read_to_string(path) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(AppError::io(path, error)),
    }
}

fn read_source(raw: Option<&str>) -> Result<(Option<PathBuf>, String), AppError> {
    let Some(raw) = raw.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return Ok((None, String::new()));
    };
    let path = expand_path(raw);
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
    let content = std::fs::read_to_string(&path).map_err(|error| AppError::io(&path, error))?;
    validate_shared_source(&content)?;
    Ok((Some(path), content))
}

fn reject_source_target(source: Option<&Path>, target: &Path) -> Result<(), AppError> {
    if source.is_some_and(|path| {
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
    Ok(())
}

fn validate_target(app: &AppType, updated: &str) -> Result<(), AppError> {
    validate_prompt_content(app, updated)
}

/// Compose an ordinary Prompt without ever persisting the overlay in the DB.
/// The caller already holds projection (and its native lock, where applicable).
pub(crate) fn compose_prompt(app: &AppType, plain: &str) -> Result<String, AppError> {
    let settings = crate::settings::get_settings();
    let (source, shared) = read_source(settings.shared_instruction_file.as_deref())?;
    reject_source_target(source.as_deref(), &prompt_file_path(app)?)?;
    let plain = strip_managed_block(plain)?;
    let updated = replace_managed_block(&plain, &shared)?;
    validate_target(app, &updated)?;
    Ok(updated)
}

#[derive(Debug)]
struct Change {
    app: AppType,
    target: PathBuf,
    before: Option<String>,
    after: String,
}

fn prepare_changes(
    targets: Vec<(AppType, PathBuf)>,
    source: Option<&Path>,
    shared: &str,
    state: Option<&crate::store::AppState>,
    mut read: impl FnMut(&AppType, &Path) -> Result<Option<String>, AppError>,
) -> Result<Vec<Change>, AppError> {
    let mut changes: Vec<Change> = Vec::new();
    for (app, target) in targets {
        reject_source_target(source, &target)?;
        let before = read(&app, &target)?;
        let legacy_plain = match (state, before.as_deref()) {
            (Some(state), Some(raw)) => {
                crate::services::prompt::legacy_plain_for_app(state, &app, raw)?
            }
            _ => None,
        };
        let base = legacy_plain
            .as_deref()
            .or(before.as_deref())
            .unwrap_or_default();
        let after = replace_managed_block(base, shared)?;
        validate_target(&app, &after)?;
        if before.as_deref().unwrap_or_default() == after {
            continue;
        }
        // Directory overrides may make multiple apps share AGENTS.md. Validate
        // every app's limit, but publish once; a Pi alias must retain native CAS.
        if let Some(change) = changes.iter_mut().find(|change| change.target == target) {
            if change.after != after {
                return Err(AppError::Conflict(format!(
                    "多个 Agent 对同一指令文件的普通 Prompt 不一致: {}",
                    target.display()
                )));
            }
            if app == AppType::Pi {
                change.app = app;
            }
            continue;
        }
        changes.push(Change {
            app,
            target,
            before,
            after,
        });
    }
    Ok(changes)
}

fn write_state(
    change: &Change,
    expected: Option<&str>,
    next: Option<&str>,
    pi: Option<&PiAgentsFileGuard>,
) -> Result<(), AppError> {
    if change.app == AppType::Pi {
        let guard = pi.ok_or_else(|| AppError::Config("Pi instruction lock is missing".into()))?;
        let current = guard.read()?;
        if current.content.as_deref() != expected {
            return Err(AppError::Conflict(format!(
                "共享指令目标文件已被修改，请重试: {}",
                change.target.display()
            )));
        }
        return match next {
            Some(content) => guard.replace(&current.revision, content),
            None => guard.delete(&current.revision),
        };
    }
    if read_target(&change.target)?.as_deref() != expected {
        return Err(AppError::Conflict(format!(
            "共享指令目标文件已被修改，请重试: {}",
            change.target.display()
        )));
    }
    match next {
        Some(content) => write_text_file(&change.target, content),
        None => std::fs::remove_file(&change.target)
            .map_err(|error| AppError::io(&change.target, error)),
    }
}

fn publish_changes(
    changes: &[Change],
    mut publish: impl FnMut(&Change, Option<&str>, Option<&str>) -> Result<(), AppError>,
    commit: impl FnOnce() -> Result<(), AppError>,
) -> Result<(), AppError> {
    let mut applied = 0;
    let outcome = (|| {
        for change in changes {
            publish(change, change.before.as_deref(), Some(&change.after))?;
            applied += 1;
        }
        commit()
    })();
    if let Err(error) = outcome {
        let mut failures = Vec::new();
        for change in changes[..applied].iter().rev() {
            if let Err(rollback) = publish(change, Some(&change.after), change.before.as_deref()) {
                failures.push(format!("{}: {rollback}", change.target.display()));
            }
        }
        if !failures.is_empty() {
            return Err(AppError::Message(format!(
                "共享指令同步失败 ({error}); 部分文件回滚失败: {}",
                failures.join("; ")
            )));
        }
        return Err(error);
    }
    Ok(())
}

/// Publish files and then settings; a rejected settings commit restores both
/// prior bytes and prior absence. Neither this nor startup writes ordinary DB
/// Prompts. The caller must not already hold a native/projection lock.
fn sync_with_source_impl(
    state: Option<&crate::store::AppState>,
    source: Option<&str>,
    commit: impl FnOnce() -> Result<(), AppError>,
) -> Result<(), AppError> {
    let _mcode = crate::services::prompt::MCODE_PROMPT_LOCK
        .lock()
        .map_err(|error| AppError::Message(error.to_string()))?;
    let pi = PiAgentsFileGuard::acquire()?;
    let _projection = lock_projection()?;
    let (source, shared) = read_source(source)?;
    let changes = prepare_changes(
        target_paths()?,
        source.as_deref(),
        &shared,
        state,
        |app, path| {
            if *app == AppType::Pi {
                Ok(pi.read()?.content)
            } else {
                read_target(path)
            }
        },
    )?;
    publish_changes(
        &changes,
        |change, expected, next| write_state(change, expected, next, Some(&pi)),
        commit,
    )
}

pub(crate) fn sync_with_source_for_state(
    state: &crate::store::AppState,
    source: Option<&str>,
    commit: impl FnOnce() -> Result<(), AppError>,
) -> Result<(), AppError> {
    sync_with_source_impl(Some(state), source, commit)
}

pub(crate) fn sync_from_settings_for_state(state: &crate::store::AppState) -> Result<(), AppError> {
    let source = crate::settings::get_settings().shared_instruction_file;
    sync_with_source_for_state(state, source.as_deref(), || Ok(()))
}

#[cfg(test)]
pub(crate) fn sync_with_source(
    source: Option<&str>,
    commit: impl FnOnce() -> Result<(), AppError>,
) -> Result<(), AppError> {
    sync_with_source_impl(None, source, commit)
}

#[cfg(test)]
pub(crate) fn sync_from_settings() -> Result<(), AppError> {
    let source = crate::settings::get_settings().shared_instruction_file;
    sync_with_source(source.as_deref(), || Ok(()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_overlay_round_trips_every_byte_of_plain_content() {
        for plain in ["", "# user", "# user\n", "# user\n\n", "# user\r\n\r\n"] {
            for shared in [
                "Use short answers.\n",
                "line one\r\nline two\r\n",
                "line one\nline two\n",
            ] {
                let first = replace_managed_block(plain, shared).unwrap();
                assert!(first.starts_with(BLOCK_START));
                assert_eq!(strip_managed_block(&first).unwrap(), plain);
                assert_eq!(replace_managed_block(&first, "").unwrap(), plain);
                assert_eq!(replace_managed_block(&first, shared).unwrap(), first);
            }
        }
    }

    #[test]
    fn replacement_stays_in_place_and_keeps_user_whitespace() {
        let existing = format!("head\r\n\r\n{BLOCK_START}\r\nold\r\n{BLOCK_END}\r\n\r\ntail\r\n");
        let updated = replace_managed_block(&existing, "new\r\n").unwrap();
        assert_eq!(
            updated,
            format!("head\r\n\r\n{BLOCK_START}\r\nnew\r\n{BLOCK_END}\r\n\r\ntail\r\n")
        );
        assert_eq!(
            strip_managed_block(&updated).unwrap(),
            "head\r\n\r\n\r\ntail\r\n"
        );
        assert_eq!(replace_managed_block(&updated, "new\r\n").unwrap(), updated);
    }

    #[test]
    fn rejects_partial_duplicate_nested_reversed_and_inline_target_markers() {
        let good = format!("{BLOCK_START}\nx\n{BLOCK_END}\n");
        for invalid in [
            BLOCK_START.to_string(),
            BLOCK_END.to_string(),
            format!("{good}{good}"),
            format!("{BLOCK_START}\n{BLOCK_START}\nx\n{BLOCK_END}\n{BLOCK_END}"),
            format!("{BLOCK_END}\nx\n{BLOCK_START}"),
            format!("prefix{BLOCK_START}\nx\n{BLOCK_END}"),
            format!("{BLOCK_START}\nx\n{BLOCK_END}suffix"),
        ] {
            assert!(replace_managed_block(&invalid, "new").is_err(), "{invalid}");
            assert!(strip_managed_block(&invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn reserved_source_markers_are_rejected_before_wrapping() {
        for marker in [
            BLOCK_START,
            BLOCK_END,
            "<!-- cc-switch:prompt:start -->",
            "<!-- cc-switch:prompt:end -->",
        ] {
            assert!(replace_managed_block("untouched", marker).is_err());
        }
    }

    #[test]
    fn legacy_layout_matches_only_the_exact_saved_plain_prefix() {
        for plain in ["plain", "plain\n", "plain\n\n", ""] {
            let mut prefix = plain.to_string();
            if !prefix.is_empty() && !prefix.ends_with('\n') {
                prefix.push('\n');
            }
            if !prefix.is_empty() {
                prefix.push('\n');
            }
            let raw = format!("{prefix}{BLOCK_START}\nshared old source\n{BLOCK_END}\n");
            assert!(matches_legacy_prompt(&raw, plain).unwrap());
            assert!(!matches_legacy_prompt(&format!("external edit\n{raw}"), plain).unwrap());
            assert!(!matches_legacy_prompt(&format!("{raw}external edit"), plain).unwrap());
            assert!(!matches_legacy_prompt(
                &raw.replace("shared old source", "updated source"),
                &format!("{plain}external edit")
            )
            .unwrap());
            assert!(!matches_legacy_prompt(&raw.replace("\n", "\r\n"), plain).unwrap());
        }
        assert!(!matches_legacy_prompt("no managed block", "no managed block").unwrap());
        let prefixed = replace_managed_block("plain", "shared").unwrap();
        assert!(!matches_legacy_prompt(&prefixed, "plain").unwrap());
    }

    #[test]
    fn validates_final_utf8_bytes_for_native_targets() {
        assert!(validate_target(&AppType::Mcode, &"x".repeat(32768)).is_ok());
        assert!(validate_target(&AppType::Mcode, &"中".repeat(10923)).is_err());
        let combined = replace_managed_block(&"p".repeat(110), &"s".repeat(32700)).unwrap();
        assert!(validate_target(&AppType::Mcode, &combined).is_err());
        assert!(validate_target(&AppType::Pi, &"x".repeat(PI_MAX_BYTES)).is_ok());
        assert!(validate_target(&AppType::Pi, &"x".repeat(PI_MAX_BYTES + 1)).is_err());
    }

    fn fixtures(root: &Path) -> Vec<(AppType, PathBuf)> {
        vec![
            (AppType::Claude, root.join("existing.md")),
            (AppType::Codex, root.join("new.md")),
        ]
    }

    #[test]
    fn validates_every_target_before_any_publish() {
        let temp = tempfile::tempdir().unwrap();
        let targets = fixtures(temp.path());
        std::fs::write(&targets[0].1, "original").unwrap();
        std::fs::write(&targets[1].1, BLOCK_START).unwrap();
        assert!(
            prepare_changes(targets, None, "shared", None, |_, path| read_target(path)).is_err()
        );
        assert_eq!(
            std::fs::read_to_string(temp.path().join("existing.md")).unwrap(),
            "original"
        );
    }

    #[test]
    fn late_native_size_violation_prevents_all_publication() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first.md");
        let mcode = temp.path().join("mcode.md");
        std::fs::write(&first, "original").unwrap();
        std::fs::write(&mcode, "p".repeat(200)).unwrap();
        let result = prepare_changes(
            vec![
                (AppType::Claude, first.clone()),
                (AppType::Mcode, mcode.clone()),
            ],
            None,
            &"s".repeat(32700),
            None,
            |_, path| read_target(path),
        );
        assert!(result.is_err());
        assert_eq!(std::fs::read_to_string(first).unwrap(), "original");
        assert_eq!(std::fs::read_to_string(mcode).unwrap(), "p".repeat(200));
    }

    #[test]
    fn failed_settings_commit_restores_bytes_and_absence() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("existing.md"), "original\r\n").unwrap();
        let changes = prepare_changes(fixtures(temp.path()), None, "shared", None, |_, path| {
            read_target(path)
        })
        .unwrap();
        let result = publish_changes(
            &changes,
            |change, expected, next| write_state(change, expected, next, None),
            || Err(AppError::Message("settings rejected".into())),
        );
        assert!(result.is_err());
        assert_eq!(
            std::fs::read_to_string(temp.path().join("existing.md")).unwrap(),
            "original\r\n"
        );
        assert!(!temp.path().join("new.md").exists());
    }

    #[test]
    fn later_write_failure_restores_prior_created_file() {
        let temp = tempfile::tempdir().unwrap();
        let changes = prepare_changes(fixtures(temp.path()), None, "shared", None, |_, path| {
            read_target(path)
        })
        .unwrap();
        let mut writes = 0;
        let result = publish_changes(
            &changes,
            |change, expected, next| {
                writes += 1;
                if writes == 2 {
                    return Err(AppError::Message("injected write failure".into()));
                }
                write_state(change, expected, next, None)
            },
            || Ok(()),
        );
        assert!(result.is_err());
        assert!(!temp.path().join("existing.md").exists());
        assert!(!temp.path().join("new.md").exists());
    }

    #[test]
    fn failed_rollback_is_reported_and_does_not_overwrite_external_edits() {
        let temp = tempfile::tempdir().unwrap();
        let changes = prepare_changes(fixtures(temp.path()), None, "shared", None, |_, path| {
            read_target(path)
        })
        .unwrap();
        let result = publish_changes(
            &changes,
            |change, expected, next| write_state(change, expected, next, None),
            || {
                std::fs::write(temp.path().join("existing.md"), "external edit").unwrap();
                Err(AppError::Message("settings rejected".into()))
            },
        );
        assert!(result.unwrap_err().to_string().contains("回滚失败"));
        assert_eq!(
            std::fs::read_to_string(temp.path().join("existing.md")).unwrap(),
            "external edit"
        );
        assert!(!temp.path().join("new.md").exists());
    }

    #[test]
    fn source_target_alias_is_rejected_and_duplicate_targets_are_published_once() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("instructions.md");
        std::fs::write(&path, "plain").unwrap();
        assert!(prepare_changes(
            vec![(AppType::Codex, path.clone())],
            Some(&path),
            "plain",
            None,
            |_, target| read_target(target)
        )
        .is_err());
        let changes = prepare_changes(
            vec![(AppType::Codex, path.clone()), (AppType::Mcode, path)],
            None,
            "shared",
            None,
            |_, target| read_target(target),
        )
        .unwrap();
        assert_eq!(changes.len(), 1);
    }
}
