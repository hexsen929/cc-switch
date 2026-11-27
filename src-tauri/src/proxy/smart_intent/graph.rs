use super::{classify, engine::Decision, Protocol, SmartConfig, Target, Trace, Turn};
use crate::{
    provider::Provider,
    proxy::{handler_context::RequestContext, server::ProxyState},
};
use std::{
    borrow::Cow,
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
        if self.decision_pool {
            return Err("main group must route answers");
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
            let static_group = node.decision_pool
                || node.ordered_only
                || node.classifiers.is_empty() && node.categories.is_empty();
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
            for category in node.categories.iter().filter(|_| !static_group) {
                if category.description.trim().is_empty()
                    || category.description.len() > 512
                    || !(1..=8).contains(&category.targets.len())
                {
                    return Err("invalid category description or targets");
                }
            }
            let refs = node
                .members
                .iter()
                .map(|t| (t, node.decision_pool))
                .chain(
                    node.categories
                        .iter()
                        .filter(|_| !static_group)
                        .flat_map(|c| &c.targets)
                        .map(|t| (t, false)),
                )
                .chain(
                    node.classifiers
                        .iter()
                        .filter(|_| !static_group)
                        .map(|t| (t, true)),
                );
            for (t, judge) in refs {
                if let Some(id) = &t.group_id {
                    let child = self
                        .groups
                        .get(id)
                        .ok_or("referenced routing group does not exist")?;
                    if child.decision_pool != judge {
                        return Err(
                            "classifier pools and answer groups cannot substitute for each other",
                        );
                    }
                    if !t.provider_id.is_empty()
                        || !t.model.is_empty()
                        || t.effort.is_some()
                        || t.protocol != Protocol::Chat
                    {
                        return Err("a group reference cannot also specify a leaf model");
                    }
                } else if !t.valid_leaf() || !judge && t.protocol != Protocol::Chat {
                    return Err("invalid card or model");
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
                        .filter(|_| !node.ordered_only && !node.decision_pool)
                        .flat_map(|c| &c.targets),
                )
                .chain(
                    node.classifiers
                        .iter()
                        .filter(|_| !node.ordered_only && !node.decision_pool),
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
        for g in std::iter::once(self)
            .chain(self.groups.values())
            .filter(|g| !g.ordered_only && !g.decision_pool)
        {
            if self.with_classifiers(g).classifiers.len() > 16 {
                return Err("more than 16 distinct classifiers after expanding pools");
            }
        }
        Ok(())
    }

    /// Keep the parent question and confidence threshold. A pool changes only
    /// which card answers that question. Expand each pool and unique leaf once.
    fn with_classifiers<'a>(&'a self, node: &'a Self) -> Cow<'a, Self> {
        if !node.classifiers.iter().any(|t| t.group_id.is_some()) {
            return Cow::Borrowed(node);
        }
        let mut pending: Vec<_> = node.classifiers.iter().rev().collect();
        let mut pools = HashSet::new();
        let mut seen = HashSet::new();
        let mut classifiers = Vec::new();
        while let Some(t) = pending.pop() {
            if let Some(id) = &t.group_id {
                if pools.insert(id) {
                    if let Some(pool) = self.groups.get(id).filter(|g| g.decision_pool && g.enabled)
                    {
                        pending.extend(pool.members.iter().rev());
                    }
                }
            } else if seen.insert((&t.provider_id, &t.model, t.protocol)) {
                classifiers.push(t.clone());
            }
        }
        Cow::Owned(Self {
            enabled: true,
            classifiers,
            categories: node.categories.clone(),
            prompt: node.prompt.clone(),
            timeout_ms: node.timeout_ms,
            keep_warm: node.keep_warm,
            min_confidence: node.min_confidence,
            ..Default::default()
        })
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
                if !seen_groups.insert(id) || !cfg.enabled || cfg.decision_pool {
                    continue;
                }
                let scope_name = format!("{}:{id}", ctx.app_type_str);
                let mut result = if cfg.ordered_only || cfg.classifiers.is_empty() {
                    Decision::new("members", None, None)
                } else {
                    // A child does not acquire a fresh full timeout, even on a
                    // classifier failure or a list of nested fallback groups.
                    let effective = root.with_classifiers(cfg);
                    let node = effective.as_ref();
                    state
                        .provider_router
                        .smart_intent
                        .decide_until(node, &scope_name, scope, turn, deadline, |i, prev| {
                            classify(ctx, state, node, i, turn, prev)
                        })
                        .await
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
