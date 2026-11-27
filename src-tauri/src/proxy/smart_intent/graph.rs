use super::{classify, engine::Decision, Protocol, SmartConfig, Target, Trace, Turn};
use crate::{
    provider::Provider,
    proxy::{handler_context::RequestContext, server::ProxyState},
};
use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

const MAX_GROUPS: usize = 16;
const DEPTH: usize = 8;
const MAX_LEAVES: usize = 64;
const ROOT: &str = "$root";

impl SmartConfig {
    pub fn valid(&self) -> bool {
        self.validate_graph().is_ok()
    }

    pub fn validate_graph(&self) -> Result<(), &'static str> {
        if self.groups.len() > MAX_GROUPS {
            return Err("too many routing groups (max 16)");
        }
        for (key, node) in &self.groups {
            if key.is_empty()
                || key.trim() != key
                || key.len() > 96
                || key == ROOT
                || key.starts_with('$')
                || key.contains(char::is_control)
                || !node.groups.is_empty()
            {
                return Err("invalid group id or nested registry");
            }
        }
        for node in std::iter::once(self).chain(self.groups.values()) {
            let static_group =
                node.ordered_only || node.classifiers.is_empty() && node.categories.is_empty();
            if static_group && node.members.is_empty() {
                return Err("an ordered group needs at least one member");
            }
            if !(200..=8000).contains(&node.timeout_ms)
                || node.min_confidence > 100
                || node.prompt.len() > 2048
                || node.members.len() > 16
                || (!static_group
                    && (!(1..=3).contains(&node.classifiers.len())
                        || !(2..=16).contains(&node.categories.len())))
            {
                return Err("invalid group settings, timeout, classifier or category count");
            }
            if !static_group && !node.classifiers.iter().all(Target::valid_leaf) {
                return Err("invalid classifier card or model");
            }
            for category in node.categories.iter().filter(|_| !static_group) {
                if category.description.trim().is_empty()
                    || category.description.len() > 512
                    || !(1..=8).contains(&category.targets.len())
                {
                    return Err("invalid category description or targets");
                }
            }
            for t in node.members.iter().chain(
                node.categories
                    .iter()
                    .filter(|_| !static_group)
                    .flat_map(|c| &c.targets),
            ) {
                if let Some(id) = &t.group_id {
                    if !self.groups.contains_key(id) {
                        return Err("referenced routing group does not exist");
                    }
                    if !t.provider_id.is_empty()
                        || !t.model.is_empty()
                        || t.effort.is_some()
                        || t.protocol != Protocol::Chat
                    {
                        return Err("a group reference cannot also specify a leaf model");
                    }
                } else if !t.valid_leaf() || t.protocol != Protocol::Chat {
                    return Err("invalid answer card or model");
                }
            }
        }
        fn height<'a>(
            id: &'a str,
            node: &'a SmartConfig,
            root: &'a SmartConfig,
            visiting: &mut HashSet<&'a str>,
            known: &mut HashMap<&'a str, usize>,
        ) -> Result<usize, &'static str> {
            if let Some(h) = known.get(id) {
                return Ok(*h);
            }
            if !visiting.insert(id) {
                return Err("routing group reference cycle");
            }
            let mut h = 1;
            for child in node
                .members
                .iter()
                .chain(
                    node.categories
                        .iter()
                        .filter(|_| !node.ordered_only)
                        .flat_map(|c| &c.targets),
                )
                .filter_map(|t| t.group_id.as_deref())
            {
                h = h.max(1 + height(child, &root.groups[child], root, visiting, known)?);
            }
            visiting.remove(id);
            if h > DEPTH {
                return Err("routing groups exceed maximum depth (8)");
            }
            known.insert(id, h);
            Ok(h)
        }
        let (mut visiting, mut known) = (HashSet::new(), HashMap::new());
        height(ROOT, self, self, &mut visiting, &mut known)?;
        for (id, g) in &self.groups {
            height(id, g, self, &mut visiting, &mut known)?;
        }
        Ok(())
    }
}

pub(crate) struct Plan {
    pub leaves: Vec<(Target, Provider)>,
    pub trace: Trace,
}

pub(crate) async fn resolve(
    ctx: &RequestContext,
    state: &ProxyState,
    root: &SmartConfig,
    scope: [u8; 32],
    turn: &Turn,
) -> Plan {
    enum Work<'a> {
        Node(&'a str, &'a SmartConfig),
        Target(&'a Target),
    }
    let start = tokio::time::Instant::now();
    let deadline = start + Duration::from_millis(root.timeout_ms);
    let mut work = vec![Work::Node(ROOT, root)];
    let mut seen = HashSet::new();
    let mut seen_groups = HashSet::new();
    let mut leaves = Vec::new();
    let mut trace = Decision::new("noMatch", None, None).trace;
    while let Some(w) = work.pop() {
        match w {
            Work::Node(id, cfg) => {
                if !seen_groups.insert(id) || !cfg.enabled {
                    continue;
                }
                let scope_name = format!("{}:{id}", ctx.app_type_str);
                let mut result = if cfg.ordered_only || cfg.classifiers.is_empty() {
                    Decision::new("members", None, None)
                } else {
                    // A child does not acquire a fresh full timeout, even on a
                    // classifier failure or a list of nested fallback groups.
                    let pending = state.provider_router.smart_intent.decide(
                        cfg,
                        &scope_name,
                        scope,
                        turn,
                        |i, prev| classify(ctx, state, cfg, i, turn, prev),
                    );
                    tokio::time::timeout_at(deadline, pending)
                        .await
                        .unwrap_or_else(|_| Decision::new("timeout", None, None))
                };
                let chosen = result.category.and_then(|i| cfg.categories.get(i));
                if id == ROOT {
                    trace = result.trace;
                } else {
                    if let Some(receipt) = result.trace.receipt.take() {
                        trace.more_receipts.push(receipt);
                    }
                    trace.groups.push(super::engine::GroupStep {
                        group: id.to_owned(),
                        status: result.trace.status,
                        category: result.trace.category,
                        classifier: result.trace.classifier,
                        elapsed_ms: result.trace.elapsed_ms,
                    });
                }
                work.extend(cfg.members.iter().rev().map(Work::Target));
                if let Some(c) = chosen {
                    work.extend(c.targets.iter().rev().map(Work::Target));
                }
            }
            Work::Target(t) => {
                if let Some(id) = &t.group_id {
                    if let Some(g) = root.groups.get(id) {
                        work.push(Work::Node(id, g));
                    }
                } else if seen.insert((&t.provider_id, &t.model, &t.effort)) {
                    if let Ok(Some(card)) =
                        state.db.get_provider_by_id(&t.provider_id, &ctx.app_type)
                    {
                        leaves.push((t.clone(), card));
                        if !ctx.app_config.auto_failover_enabled || leaves.len() >= MAX_LEAVES {
                            break;
                        }
                    }
                }
            }
        }
    }
    if leaves.is_empty() && trace.category.is_some() {
        trace.status = "missingTarget";
    }
    trace.elapsed_ms = start.elapsed().as_millis() as u64;
    Plan { leaves, trace }
}
