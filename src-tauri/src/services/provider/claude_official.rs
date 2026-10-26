//! Claude 官方卡绑定的账号。
//!
//! 官方卡可以在授权中心里绑一个 Claude 官方账号（`authBinding.authProvider = "claude_official"`，
//! `accountId` 是账号 key）。没绑账号的官方卡用 Claude Code 现在的登录（`~/.claude`）。
//!
//! - 从卡片「打开终端」：绑了账号就把它的目录放进那个终端的启动环境（所有平台）。
//! - 切到绑号卡时把账号的登录移进默认槽位（见 `claude_account::snapshot`），之后任何地方
//!   新启动的 `claude` 都是这个号。设了 Claude 配置目录覆盖时不切，只能从卡片打开终端。
//!
//! 以前的开发版把 `CLAUDE_CONFIG_DIR` 写进过 `~/.claude/settings.json`（Claude Code 只在启动
//! 环境里完整认它，写在 settings 里登录状态仍从默认目录读）。切换时照样删掉 CC Switch 写的值
//! （形如 `<数据目录>/claude-accounts/<id>/config`），用户自己写的不碰。

use serde_json::Value;
use std::sync::Mutex;

use crate::app_config::AppType;
use crate::database::Database;
use crate::error::AppError;
use crate::live::patch::json::JsonPatch;
use crate::live::patch::KeyPath;
use crate::provider::Provider;
use crate::services::claude_account::{self, layout, snapshot, ClaudeAccountManager};
use crate::store::AppState;

/// 授权中心里 Claude 官方账号的认证供应商标识。
pub const AUTH_PROVIDER: &str = "claude_official";
pub const CONFIG_DIR_ENV: &str = "CLAUDE_CONFIG_DIR";

// 保持整笔操作的方向一致：恢复的“读 journal → 判指针 → 回滚”不与新切换交错。
// 固定顺序：LOGIN_OPERATIONS → snapshot::SWITCH → 快照/账号文件锁。
// write 闭包只提交 live/指针，不递归进入本模块的登录操作。
static LOGIN_OPERATIONS: Mutex<()> = Mutex::new(());

fn with_login_operation<R>(operation: impl FnOnce() -> R) -> R {
    let _guard = LOGIN_OPERATIONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    operation()
}

/// Claude 官方卡（预设标了 `category: "official"`）。
pub fn is_official(provider: &Provider) -> bool {
    provider.category.as_deref() == Some("official")
}

/// 官方卡绑定的账号 key；没绑（默认登录）或不是官方卡时为 `None`。
pub fn bound_account(provider: &Provider) -> Option<String> {
    if !is_official(provider) {
        return None;
    }
    provider
        .meta
        .as_ref()?
        .managed_account_id_for(AUTH_PROVIDER)
        .filter(|id| !id.is_empty())
}

fn config_dir_key() -> KeyPath {
    KeyPath::new(&["env"]).child(CONFIG_DIR_ENV)
}

/// live 里这个值是不是 CC Switch 写的账号目录。
fn owned_by_cc_switch(value: &Value) -> bool {
    value.as_str().is_some_and(|text| {
        layout::is_account_config_dir(&crate::config::get_app_config_dir(), text)
    })
}

/// 删掉以前开发版写进 `settings.json` 的账号目录。
pub fn strip_from_patch(patch: &mut JsonPatch) {
    patch
        .remove_when
        .push((config_dir_key(), owned_by_cc_switch));
}

/// 写一张卡到 live（`write`）前后的登录切换：`target` 是要切到的卡（`None` 是不切登录），
/// 先把它绑的账号移进默认槽位，`write` 失败就换回原来的号，最后确认切换日志。
pub fn with_login_switch<R>(
    target: Option<&Provider>,
    write: impl FnOnce() -> Result<R, AppError>,
) -> Result<R, AppError> {
    with_login_operation(|| {
        let change = match target {
            Some(target) => switch_login(target)?,
            None => None,
        };
        let result = write();
        settle(change, result.is_ok());
        result
    })
}

/// 换掉登录之前默认槽位里是什么：切换的后续步骤失败时用来换回去。
enum PreviousLogin {
    /// 原来的号和它被收回到的目录
    Account {
        account: String,
        dir: std::path::PathBuf,
    },
    /// 原来没有登录：把刚移进去的 `target` 移回它的目录
    Empty {
        target: String,
        dir: std::path::PathBuf,
    },
}

/// 只确认本次实际切换产生的日志；无关设置保存、AlreadyLive 不拥有旧日志。
struct LoginSwitch {
    previous: PreviousLogin,
    receipt: Option<snapshot::Journal>,
}

/// 切到（或重新应用）`target` 之前：`target` 绑了本机账号时，把这个账号的登录移进
/// 默认槽位。账号不在这台电脑上（别的设备同步来的卡）时不动登录，只记日志。
/// 真的换了号时返回原来的号。
fn switch_login(target: &Provider) -> Result<Option<LoginSwitch>, AppError> {
    if !snapshot::active() {
        return Ok(None);
    }
    let Some(account) = bound_account(target) else {
        return Ok(None);
    };
    let data_dir = crate::config::get_app_config_dir();
    let Some(dir) = claude_account::config_dir_on_disk(&data_dir, &account) else {
        log::warn!("[ClaudeAccount] 卡片绑定的账号不在这台电脑上，不切换登录");
        return Ok(None);
    };
    // 切过去之前把共享层同步到最新（默认的设置、MCP 可能在这之后改过）
    claude_account::sync_shared(&dir);
    let (outcome, receipt) = with_snapshot_ctx(&data_dir, |ctx| {
        snapshot::check_out_receipted(ctx, &account, &dir)
    })?;
    Ok(match outcome {
        snapshot::Outcome::Switched {
            previous: Some((account, dir)),
            ..
        } => Some(LoginSwitch {
            previous: PreviousLogin::Account { account, dir },
            receipt,
        }),
        snapshot::Outcome::Switched { previous: None, .. } => Some(LoginSwitch {
            previous: PreviousLogin::Empty {
                target: account,
                dir,
            },
            receipt,
        }),
        snapshot::Outcome::AlreadyLive => None,
    })
}

/// 换完登录之后写卡片的结果出来了：失败就按配置的走向决定换不换回。写成了、或换回成功（或
/// 不用换回）就确认切换日志；换回失败时留着，启动时按卡片指针再核对一次。
fn settle(change: Option<LoginSwitch>, written: bool) {
    settle_at(
        &crate::config::get_app_config_dir(),
        change,
        written,
        restore_login,
    );
}

fn settle_at(
    data_dir: &std::path::Path,
    change: Option<LoginSwitch>,
    written: bool,
    restore: impl FnOnce(PreviousLogin) -> bool,
) {
    let Some(change) = change else {
        return;
    };
    if written || restore(change.previous) {
        if let Some(receipt) = change.receipt {
            snapshot::confirm_receipt(data_dir, &receipt);
        }
    }
}

/// 登录换过去了、但 settings.json 或卡片指针没写成：把登录换回原来的号，免得界面说没切、
/// 实际已经换了号。换不回去只记日志（原来的号在它的账号目录里，下次切过去就回来了）。
///
/// 配置操作已经开始发布时不换回：它会在下次操作或启动时前滚到这张卡，登录要和它同向。
/// 返回登录是否已经和卡片一致（换回了，或不用换回）。
fn restore_login(previous: PreviousLogin) -> bool {
    let store = crate::live::engine::DeviceStore::for_device();
    if matches!(
        crate::mode::operation::pending_published(&store, AppType::Claude.as_str()),
        Ok(Some(true))
    ) {
        log::info!("[ClaudeAccount] 配置写入会前滚补完，登录保持切换后的账号");
        return true;
    }
    let data_dir = crate::config::get_app_config_dir();
    let result = with_snapshot_ctx(&data_dir, |ctx| match &previous {
        PreviousLogin::Account { account, dir } => {
            snapshot::check_out_receipted(ctx, account, dir).map(|(_, receipt)| receipt)
        }
        PreviousLogin::Empty { target, dir } => snapshot::vacate_receipted(ctx, target, dir),
    });
    match result {
        Ok(receipt) => {
            if let Some(receipt) = receipt {
                snapshot::confirm_receipt(&data_dir, &receipt);
            }
            true
        }
        Err(error) => {
            log::warn!("[ClaudeAccount] 切换失败后换回原来的登录也失败，启动时再试: {error}");
            false
        }
    }
}

/// 启动时补完上次没做完的登录切换（崩溃、断电）。要在补完配置操作之后调用（指针已落定）。
///
/// 登录换过去以后、卡片指针更新之前崩溃的话，指针还停在原来的卡上：这时把登录换回切换前
/// 的号，和指针一致。
pub fn recover_login_switch(db: &Database) {
    with_login_operation(|| recover_login_switch_locked(db));
}

fn recover_login_switch_locked(db: &Database) {
    let data_dir = crate::config::get_app_config_dir();
    let Some(initial) = snapshot::load_state(&data_dir).journal else {
        return;
    };
    let journal = if initial.step == snapshot::Step::Done {
        initial
    } else {
        match with_snapshot_ctx(&data_dir, snapshot::recover_receipted) {
            Ok(Some(journal)) if journal.step == snapshot::Step::Done => journal,
            Ok(_) => return,
            Err(error) => {
                log::warn!("[ClaudeAccount] 补完上次的登录切换失败: {error}");
                return;
            }
        }
    };
    // 恢复期间可能有另一笔登录操作：不要按过时快照作决定，更不清掉新日志。
    if snapshot::load_state(&data_dir).journal.as_ref() != Some(&journal) {
        return;
    }
    // 指针已经是这个号的卡，或者用户之后在 Claude Code 里换了别的号：都不用动。换回失败时
    // 日志留着，下次启动再试
    let pointer_matches = pointer_account(db).as_deref() == Some(journal.target.as_str());
    let mut settled = true;
    if !pointer_matches && snapshot::is_live(&journal.target) {
        log::info!("[ClaudeAccount] 上次切换没来得及更新卡片，把登录换回原来的号");
        let previous = match journal.before_owner.as_deref() {
            Some(account) => claude_account::config_dir_on_disk(&data_dir, account).map(|dir| {
                PreviousLogin::Account {
                    account: account.to_string(),
                    dir,
                }
            }),
            None => Some(PreviousLogin::Empty {
                target: journal.target.clone(),
                dir: journal.target_dir.clone(),
            }),
        };
        if let Some(previous) = previous {
            settled = restore_login(previous);
        }
    }
    if settled {
        snapshot::confirm_receipt(&data_dir, &journal);
    }
}

/// 账号 `account` 刚在授权中心重新登录（新登录在 `dir`）：它正在默认槽位里时，
/// 用新登录换掉默认槽位里的旧登录（旧的可能已过期或被吊销，否则重新登录了 `claude` 还是用
/// 不了）。登录成功本身不受影响，这里失败只记日志。
pub fn replace_live_login(account: &str, dir: &std::path::Path) {
    with_login_operation(|| replace_live_login_locked(account, dir));
}

fn replace_live_login_locked(account: &str, dir: &std::path::Path) {
    if !snapshot::active() {
        return;
    }
    let data_dir = crate::config::get_app_config_dir();
    match with_snapshot_ctx(&data_dir, |ctx| {
        snapshot::replace_live_receipted(ctx, account, dir)
    }) {
        Ok((true, receipt)) => {
            // 同一个号换了一份登录，卡片不用动
            if let Some(receipt) = receipt {
                snapshot::confirm_receipt(&data_dir, &receipt);
            }
            log::info!("[ClaudeAccount] 重新登录的账号正在使用，已换上新登录");
        }
        Ok((false, _)) => {}
        Err(error) => log::warn!("[ClaudeAccount] 重新登录后换上新登录失败: {error}"),
    }
}

/// 直连指针指着的那张卡绑的账号。
fn pointer_account(db: &Database) -> Option<String> {
    let app = AppType::Claude;
    let current =
        crate::mode::current::provider_for(db, &app, crate::mode::current::Purpose::Direct)
            .ok()??;
    let providers = db.get_all_providers(app.as_str()).ok()?;
    bound_account(providers.get(&current)?)
}

/// 账号在默认槽位、要用默认目录开终端时：Claude Code 现在用的必须是官方卡，否则
/// `settings.json` 里那一家的地址、Key（或本地路由）会盖过这个账号的登录。
pub fn ensure_default_terminal_is_official(db: &Database) -> Result<(), AppError> {
    let app = AppType::Claude;
    let in_use = if crate::mode::current::is_proxy(&app) {
        None
    } else {
        crate::mode::current::provider_for(db, &app, crate::mode::current::Purpose::Direct)?
            .and_then(|id| db.get_all_providers(app.as_str()).ok()?.get(&id).cloned())
    };
    if in_use.as_ref().is_some_and(is_official) {
        return Ok(());
    }
    Err(AppError::localized(
        "claude.account.default_taken",
        "这个账号现在在 Claude Code 的默认登录里，但 Claude Code 当前用的是其他供应商或本地路由，打开的终端不会用这个账号。请先切到这张卡（或任一官方卡）再打开终端",
        "This account is Claude Code's current sign-in, but Claude Code is set to another provider or the local router, so a new terminal wouldn't use this account. Switch to this card (or any official card) first, then open the terminal.",
    ))
}

fn with_snapshot_ctx<R>(
    data_dir: &std::path::Path,
    f: impl FnOnce(&snapshot::Ctx<'_>) -> Result<R, AppError>,
) -> Result<R, AppError> {
    let store = snapshot::store::platform_store();
    let accounts = claude_account::DiskAccounts {
        data_dir: data_dir.to_path_buf(),
    };
    f(&snapshot::Ctx {
        data_dir,
        default: snapshot::store::Slot::default_slot(),
        store: store.as_ref(),
        accounts: &accounts,
        adopter: &accounts,
    })
}

/// 账号增删、重新登录之后：当前直连的是官方卡时按新的账号表重写一次 live（账号目录变了、
/// 或绑定的账号没了要退回默认登录）。代理模式下 live 是代理契约，不碰。
pub async fn refresh_live_after_account_change(
    state: &AppState,
    _accounts: &ClaudeAccountManager,
) -> Result<(), String> {
    let app = AppType::Claude;
    let _guard = crate::mode::controller::lock_settled(state, &app)
        .await
        .map_err(|error| error.to_string())?;
    if crate::mode::current::is_proxy(&app) {
        return Ok(());
    }
    let current =
        crate::mode::current::provider_for(&state.db, &app, crate::mode::current::Purpose::Direct)
            .map_err(|error| error.to_string())?;
    let Some(current) = current else {
        return Ok(());
    };
    let providers = state
        .db
        .get_all_providers(app.as_str())
        .map_err(|error| error.to_string())?;
    let Some(provider) = providers.get(&current) else {
        return Ok(());
    };
    if !is_official(provider) {
        return Ok(());
    }
    // 可能要等 Claude Code 的锁、访问钥匙串：别占着异步线程
    let db = state.db.clone();
    let provider = provider.clone();
    tokio::task::spawn_blocking(move || {
        super::claude_direct::reapply(db.as_ref(), Some(&provider), &provider).map(|_| ())
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())
}

/// 从卡片打开终端时，绑号卡的 `claude` 用哪里的登录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountTerminal {
    /// 账号自己的目录（`CLAUDE_CONFIG_DIR=<目录>`）
    Dir(std::path::PathBuf),
    /// 账号正在默认槽位里（切卡时移进去的，它自己的目录这时是空的）：用默认目录
    Default,
}

/// 绑了本机账号的官方卡打开终端时用哪里的登录（所有平台）；没绑为 `None`。
/// 绑的账号不在这台电脑上时报错，免得悄悄用错号。
pub fn terminal_account(provider: &Provider) -> Result<Option<AccountTerminal>, AppError> {
    let Some(account) = bound_account(provider) else {
        return Ok(None);
    };
    let dir = claude_account::config_dir_on_disk(&crate::config::get_app_config_dir(), &account);
    // 账号目录里有自己的登录就用它；没有、而这个号在默认槽位里（切卡时移进去的）才用默认目录。
    // 只看身份不够：默认目录和账号目录可能各登着同一个号（用户自己在 Claude Code 里登的）
    if let Some(dir) = dir.as_ref().filter(|dir| snapshot::dir_has_login(dir)) {
        return Ok(Some(AccountTerminal::Dir(dir.clone())));
    }
    if snapshot::is_live(&account) {
        return Ok(Some(AccountTerminal::Default));
    }
    match dir {
        Some(dir) => Ok(Some(AccountTerminal::Dir(dir))),
        None => Err(AppError::localized(
            "claude.account.not_on_device",
            format!(
                "「{}」绑定的 Claude 账号不在这台电脑上。请在授权中心添加它，或编辑卡片换一个账号",
                provider.name
            ),
            format!(
                "The Claude account bound to \"{}\" isn't on this computer. Add it in the Auth Center, or edit the card to choose another account.",
                provider.name
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{AuthBinding, AuthBindingSource, ProviderMeta};
    use serde_json::json;

    #[test]
    fn recovery_and_new_switch_keep_one_operation_order() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{mpsc, Arc};
        use std::time::Duration;

        let live = Arc::new(AtomicUsize::new(1));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (attempt_tx, attempt_rx) = mpsc::channel();
        let (switched_tx, switched_rx) = mpsc::channel();

        let recovery_live = live.clone();
        let recovery = std::thread::spawn(move || {
            with_login_operation(|| {
                // 模拟旧恢复已读到 journal，尚未作出回滚动作。
                assert_eq!(recovery_live.load(Ordering::SeqCst), 1);
                entered_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                recovery_live.store(0, Ordering::SeqCst);
            });
        });
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(
            LOGIN_OPERATIONS.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        ));

        let switch_live = live.clone();
        let switching = std::thread::spawn(move || {
            attempt_tx.send(()).unwrap();
            with_login_operation(|| {
                switch_live.store(2, Ordering::SeqCst);
                switched_tx.send(()).unwrap();
            });
        });
        attempt_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(
            switched_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        release_tx.send(()).unwrap();
        recovery.join().unwrap();
        switching.join().unwrap();
        switched_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(live.load(Ordering::SeqCst), 2);
    }

    fn done_receipt(data_dir: &std::path::Path, target: &str) -> snapshot::Journal {
        snapshot::Journal {
            target: target.to_string(),
            target_dir: data_dir.join(target),
            target_org: Some("fixture-org".into()),
            target_account: format!("fingerprint-{target}"),
            before_account: Some("fingerprint-before".into()),
            before_owner: Some("before:fixture-org".into()),
            step: snapshot::Step::Done,
        }
    }

    fn save_receipt(data_dir: &std::path::Path, receipt: &snapshot::Journal) {
        snapshot::update_state(data_dir, |state| state.journal = Some(receipt.clone()))
            .expect("fixture journal");
    }

    fn owned_change(receipt: snapshot::Journal) -> LoginSwitch {
        LoginSwitch {
            previous: PreviousLogin::Account {
                account: "before:fixture-org".into(),
                dir: receipt.target_dir.join("previous"),
            },
            receipt: Some(receipt),
        }
    }

    #[test]
    fn unrelated_settings_after_failed_restore_keep_the_recovery_journal() {
        let tmp = tempfile::tempdir().unwrap();
        let receipt = done_receipt(tmp.path(), "target");
        save_receipt(tmp.path(), &receipt);

        // checkout 已完成，卡片发布失败，回滚也失败：恢复线索必须保留。
        settle_at(
            tmp.path(),
            Some(owned_change(receipt.clone())),
            false,
            |_| false,
        );
        assert_eq!(
            snapshot::load_state(tmp.path()).journal,
            Some(receipt.clone())
        );

        // 后续无关全局编辑没有切号收据，成功或失败都不拥有这笔 journal。
        for written in [true, false] {
            settle_at(tmp.path(), None, written, |_| {
                panic!("an unrelated edit must not restore a login")
            });
            assert_eq!(
                snapshot::load_state(tmp.path()).journal,
                Some(receipt.clone())
            );
        }
    }

    #[test]
    fn successful_write_confirms_only_its_exact_receipt() {
        let tmp = tempfile::tempdir().unwrap();
        let owned = done_receipt(tmp.path(), "owned");
        let later = done_receipt(tmp.path(), "later");
        save_receipt(tmp.path(), &later);
        settle_at(tmp.path(), Some(owned_change(owned.clone())), true, |_| {
            panic!("a successful write must not restore a login")
        });
        assert_eq!(snapshot::load_state(tmp.path()).journal, Some(later));

        save_receipt(tmp.path(), &owned);
        settle_at(tmp.path(), Some(owned_change(owned)), true, |_| {
            panic!("a successful write must not restore a login")
        });
        assert_eq!(snapshot::load_state(tmp.path()).journal, None);
    }

    #[test]
    fn successful_restore_confirms_its_owned_done_receipt() {
        let tmp = tempfile::tempdir().unwrap();
        let receipt = done_receipt(tmp.path(), "target");
        save_receipt(tmp.path(), &receipt);
        settle_at(tmp.path(), Some(owned_change(receipt)), false, |_| true);
        assert_eq!(snapshot::load_state(tmp.path()).journal, None);
    }

    #[test]
    fn a_switch_without_a_receipt_does_not_confirm_a_later_journal() {
        let tmp = tempfile::tempdir().unwrap();
        let later = done_receipt(tmp.path(), "later");
        save_receipt(tmp.path(), &later);
        let mut change = owned_change(done_receipt(tmp.path(), "owned"));
        change.receipt = None;
        settle_at(tmp.path(), Some(change), true, |_| {
            panic!("a successful write must not restore a login")
        });
        assert_eq!(snapshot::load_state(tmp.path()).journal, Some(later));
    }

    fn official(binding: Option<&str>) -> Provider {
        let mut provider = Provider::with_id(
            "official".into(),
            "Claude Official".into(),
            json!({"env": {}}),
            None,
        );
        provider.category = Some("official".into());
        if let Some(account) = binding {
            provider.meta = Some(ProviderMeta {
                auth_binding: Some(AuthBinding {
                    source: AuthBindingSource::ManagedAccount,
                    auth_provider: Some(AUTH_PROVIDER.into()),
                    account_id: Some(account.into()),
                }),
                ..Default::default()
            });
        }
        provider
    }

    #[test]
    fn only_official_cards_have_bound_accounts() {
        assert_eq!(bound_account(&official(None)), None);
        assert_eq!(
            bound_account(&official(Some("acc:org"))).as_deref(),
            Some("acc:org")
        );
        let mut third_party = official(Some("acc:org"));
        third_party.category = Some("third_party".into());
        assert_eq!(bound_account(&third_party), None);
    }

    #[test]
    fn patch_removes_only_cc_switch_owned_dirs() {
        let data_dir = crate::config::get_app_config_dir();
        let ours = layout::config_dir(&data_dir, "0123456789abcdef0123456789abcdef");
        let ours = ours.to_string_lossy().into_owned();
        let mut patch = JsonPatch::default();
        strip_from_patch(&mut patch);
        // 只删以前写的指针，不再写新的
        assert!(patch.set.is_empty());
        let path = std::path::Path::new("settings.json");

        let mut doc = json!({"env": {"CLAUDE_CONFIG_DIR": ours, "KEEP": "1"}});
        patch.apply_to(path, &mut doc).unwrap();
        assert_eq!(doc, json!({"env": {"KEEP": "1"}}));

        let mut doc = json!({"env": {"CLAUDE_CONFIG_DIR": "/home/me/work-claude"}});
        patch.apply_to(path, &mut doc).unwrap();
        assert_eq!(
            doc,
            json!({"env": {"CLAUDE_CONFIG_DIR": "/home/me/work-claude"}})
        );
    }

    #[test]
    fn unbound_card_on_unknown_account_writes_nothing() {
        // 绑了但本机没有这个账号：打开终端报错，默认登录的卡不设目录
        assert!(terminal_account(&official(Some("missing:acc"))).is_err());
        assert!(terminal_account(&official(None)).unwrap().is_none());
    }
}
