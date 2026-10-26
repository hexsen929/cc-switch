//! 恢复：补完崩溃时没做完的切换。

use super::creds::{account_fingerprint, credential_org};
use super::files::{read_profile, refresh_locks};
use super::store::Slot;
use super::switch::finish;
use super::{default_owner, identity, load_state, update_state, Ctx, Journal, Step, SWITCH};
use crate::error::AppError;

/// 补完上一次没做完的切换（启动时、每次切换前）。补完的切换停在 [`Step::Done`]，等调用方确认。
pub fn recover(ctx: &Ctx<'_>) -> Result<(), AppError> {
    recover_receipted(ctx).map(|_| ())
}

/// Capture only a journal completed by this recovery, under SWITCH. An
/// already-completed journal belongs to the caller's initial snapshot.
pub fn recover_receipted(ctx: &Ctx<'_>) -> Result<Option<Journal>, AppError> {
    let _serial = SWITCH
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let before = load_state(ctx.data_dir).journal;
    recover_locked(ctx)?;
    let Some(before) = before.filter(|journal| journal.step != Step::Done) else {
        return Ok(None);
    };
    Ok(load_state(ctx.data_dir).journal.filter(|journal| {
        journal.step == Step::Done
            && *journal
                == Journal {
                    target_dir: journal.target_dir.clone(),
                    step: Step::Done,
                    ..before.clone()
                }
    }))
}

pub(super) fn recover_locked(ctx: &Ctx<'_>) -> Result<(), AppError> {
    let Some(journal) = load_state(ctx.data_dir)
        .journal
        .filter(|journal| journal.step != Step::Done)
    else {
        return Ok(());
    };
    let _refresh = refresh_locks(&ctx.default)?;
    let now = ctx.store.read(&ctx.default)?;
    let now_account = now.as_ref().map(account_fingerprint);
    // Exact account fingerprints are sufficient to finish an interrupted
    // commit. Once a token changes, an organization alone does not prove its
    // user: the original user could have signed in again to the same Team.
    // Only a published target profile permits refreshed-token recovery.
    let same_org =
        journal.target_org.is_some() && now.as_ref().and_then(credential_org) == journal.target_org;
    let owner = default_owner(&ctx.default);
    let refreshed = journal.step == Step::Committed
        && same_org
        && owner.as_deref() == Some(journal.target.as_str());
    if journal.step == Step::Committed
        && same_org
        && now_account.as_deref() != Some(journal.target_account.as_str())
        && !refreshed
        && (owner == journal.before_owner || owner.is_none())
    {
        // Keep the journal and all slots intact. Returning an error also stops
        // run()/vacate() from planning another switch over this ambiguity.
        return Err(super::creds::changed());
    }
    if now_account.as_deref() == Some(journal.target_account.as_str()) || refreshed {
        // T 之后在授权中心重新登录过：旧目录已删，资料在它现在的目录里
        let found = [
            Some(journal.target_dir.clone()),
            ctx.accounts.dir_of(&journal.target),
        ]
        .into_iter()
        .flatten()
        .find_map(|dir| {
            let profile = read_profile(&Slot::Account(dir.clone()).global_config())?;
            let owner = identity::identity_from_profile(&profile)?;
            (owner.key == journal.target).then_some((dir, profile))
        });
        let Some((target_dir, profile)) = found else {
            // 读不到 T 的资料就不收尾：换不了资料，也不清 T 的登录，日志留着
            log::warn!("[ClaudeAccount] 上次切换没做完，读不到目标账号的资料，暂不补完");
            return Ok(());
        };
        log::info!("[ClaudeAccount] 补完上次没做完的切换");
        let journal = Journal {
            target_dir,
            ..journal
        };
        return finish(ctx, &journal, &profile);
    }
    if now_account != journal.before_account {
        // 既不是写之前的、也不是 T：Claude Code 在这之后登录过。账号槽位都还在，不补。
        log::warn!("[ClaudeAccount] 上次切换没做完，默认登录之后又变了，按现状处理");
    }
    // 提交点没写进去：收回那一步写进 O 账号槽位的是 O 当时的登录，和默认槽位里的是同一份，
    // 下次切走时会再覆盖一次，不影响使用。
    update_state(ctx.data_dir, |state| state.journal = None)?;
    Ok(())
}
