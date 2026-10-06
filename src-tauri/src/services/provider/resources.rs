//! Independent provider-resource projections, separate from committing config/routes.
//! A failed projection must neither skip the remaining resources nor turn a
//! committed provider save into an apparent save failure.

use crate::app_config::AppType;
use crate::error::AppError;
use crate::store::AppState;

#[cfg(test)]
thread_local! {
    static SYNC_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn sync_count() -> usize {
    SYNC_COUNT.with(|count| count.get())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Resource {
    Instructions,
    Mcp,
    Skills,
    Prompt,
}

fn run_all(
    app: &AppType,
    mut run: impl FnMut(Resource) -> Result<(), AppError>,
) -> Result<(), AppError> {
    let mut failures = Vec::new();
    for resource in [
        Resource::Instructions,
        Resource::Mcp,
        Resource::Skills,
        Resource::Prompt,
    ] {
        if (resource == Resource::Instructions && *app != AppType::Claude)
            || (resource == Resource::Prompt && *app == AppType::ClaudeDesktop)
        {
            continue;
        }
        if let Err(error) = run(resource) {
            failures.push(format!("{resource:?}: {error}"));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(AppError::Message(failures.join("; ")))
    }
}

/// Explicit retry/sync callers receive the aggregate error. Callers hold the
/// app switch lock so all projections use the same live provider.
pub(crate) fn sync(state: &AppState, app: &AppType) -> Result<(), AppError> {
    #[cfg(test)]
    SYNC_COUNT.with(|count| count.set(count.get() + 1));
    run_all(app, |resource| match resource {
        Resource::Instructions => {
            crate::claude_append_instructions::sync_current_provider_projection(&state.db)
                .map(|_| ())
        }
        Resource::Mcp => crate::services::mcp::McpService::sync_enabled_for_app(state, app),
        Resource::Skills => {
            let failed = crate::services::skill::SkillService::sync_to_app_report(&state.db, app)
                .map_err(|error| AppError::Message(error.to_string()))?;
            if failed.is_empty() {
                Ok(())
            } else {
                Err(AppError::Message(
                    failed
                        .iter()
                        .map(|failure| format!("{}: {}", failure.directory, failure.error))
                        .collect::<Vec<_>>()
                        .join("; "),
                ))
            }
        }
        Resource::Prompt => crate::services::prompt::PromptService::sync_effective_prompt_to_file(
            state,
            app.clone(),
        ),
    })
}

fn warn(state: &AppState, app: &AppType, error: &AppError) {
    log::warn!(
        "{} 配置已提交，部分供应商资源同步失败: {error}",
        app.as_str()
    );
    futures::executor::block_on(state.proxy_service.emit(
        "provider-resources-warning",
        serde_json::json!({"appType": app.as_str(), "error": error.to_string()}),
    ));
}

/// Config and route are already committed: report failure separately, never
/// propagate it as a failed save/switch or roll back the provider row.
pub(crate) fn after_commit(state: &AppState, app: &AppType) {
    if let Err(error) = sync(state, app) {
        warn(state, app, &error);
    }
}

/// Proxy route operations already perform their own resource sync. Editor
/// finalization owns resources only when the save did not rewrite a proxy route.
pub(crate) fn after_save(state: &AppState, app: &AppType, provider_id: &str) {
    let mode = crate::mode::current::mode_state(app);
    if mode.routes_to(provider_id) {
        return;
    }
    match crate::mode::current::provider_for(&state.db, app, crate::mode::current::Purpose::Live) {
        Ok(Some(current)) if current == provider_id => after_commit(state, app),
        Ok(_) => {}
        Err(error) => warn(state, app, &error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_do_not_skip_later_resources_and_are_aggregated() {
        let mut visited = Vec::new();
        let error = run_all(&AppType::Claude, |resource| {
            visited.push(resource);
            if matches!(resource, Resource::Instructions | Resource::Mcp) {
                Err(AppError::Message(format!("failed {resource:?}")))
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert_eq!(
            visited,
            [
                Resource::Instructions,
                Resource::Mcp,
                Resource::Skills,
                Resource::Prompt
            ]
        );
        assert!(error.to_string().contains("failed Instructions"));
        assert!(error.to_string().contains("failed Mcp"));
    }

    #[test]
    fn unsupported_resources_are_not_projected() {
        let mut visited = Vec::new();
        run_all(&AppType::ClaudeDesktop, |resource| {
            visited.push(resource);
            Ok(())
        })
        .unwrap();
        assert_eq!(visited, [Resource::Mcp, Resource::Skills]);
    }
}
