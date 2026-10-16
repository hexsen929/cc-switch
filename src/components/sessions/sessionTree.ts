import type { SessionMeta } from "@/types";
import { getProjectDirKey, getSessionKey } from "./utils";

// Corrupt or adversarial metadata must not cause recursive rendering or an
// unbounded parent walk. Overdeep branches become independent roots.
export const MAX_SESSION_TREE_DEPTH = 32;

export interface SessionTreeNode {
  key: string;
  session: SessionMeta;
  parent?: SessionTreeNode;
  children: SessionTreeNode[];
}

export interface SessionForest {
  nodes: Map<string, SessionTreeNode>;
  roots: SessionTreeNode[];
}

export interface SessionTreeRow {
  key: string;
  session: SessionMeta;
  depth: number;
  childCount: number;
  expanded: boolean;
}

const identity = (session: SessionMeta, sessionId = session.sessionId) =>
  JSON.stringify([
    session.providerId,
    sessionId,
    getProjectDirKey(session.projectDir?.trim() || ""),
  ]);

/** Preserve the supplied sort order; only unambiguous same-app parents attach. */
export function buildSessionForest(sessions: SessionMeta[]): SessionForest {
  const nodes = new Map<string, SessionTreeNode>();
  const identities = new Map<string, SessionTreeNode | null>();
  for (const session of sessions) {
    const key = getSessionKey(session);
    if (nodes.has(key)) continue;
    const node: SessionTreeNode = { key, session, children: [] };
    nodes.set(key, node);
    const id = identity(session);
    identities.set(id, identities.has(id) ? null : node);
  }
  const parents = new Map<string, SessionTreeNode>();
  for (const node of nodes.values()) {
    const parentId = node.session.parentSessionId;
    if (!parentId) continue;
    const parent = identities.get(identity(node.session, parentId));
    if (parent && parent !== node) parents.set(node.key, parent);
  }
  // Break a cycle at its repeated node. Use a bounded iterative walk so even a
  // many-thousand-node chain is safe before it reaches the virtual list.
  for (const node of nodes.values()) {
    const seen = new Set<string>();
    let current: SessionTreeNode | undefined = node;
    let depth = 0;
    while (current && parents.has(current.key)) {
      if (seen.has(current.key)) {
        parents.delete(current.key);
        break;
      }
      if (depth >= MAX_SESSION_TREE_DEPTH) {
        parents.delete(node.key);
        break;
      }
      seen.add(current.key);
      current = parents.get(current.key);
      depth += 1;
    }
  }
  const roots: SessionTreeNode[] = [];
  for (const node of nodes.values()) {
    const parent = parents.get(node.key);
    if (parent) {
      node.parent = parent;
      parent.children.push(node);
    } else {
      roots.push(node);
    }
  }
  return { nodes, roots };
}

/** Search retains matching subtrees and their ancestor path, without siblings. */
export function sessionSearchContext(
  forest: SessionForest,
  hits: SessionMeta[],
): Set<string> {
  const included = new Set<string>();
  const expandedSubtrees = new Set<string>();
  for (const hit of hits) {
    const node = forest.nodes.get(getSessionKey(hit));
    if (!node) continue;
    let ancestor: SessionTreeNode | undefined = node;
    while (ancestor && !included.has(ancestor.key)) {
      included.add(ancestor.key);
      ancestor = ancestor.parent;
    }
    const pending = [node];
    while (pending.length) {
      const next = pending.pop()!;
      if (expandedSubtrees.has(next.key)) continue;
      expandedSubtrees.add(next.key);
      included.add(next.key);
      for (const child of next.children) pending.push(child);
    }
  }
  return included;
}

/** Flat, fixed-height rows preserve the existing list virtualizer. */
export function flattenSessionTree(
  roots: SessionTreeNode[],
  included?: ReadonlySet<string>,
  expanded?: ReadonlySet<string>,
): SessionTreeRow[] {
  const rows: SessionTreeRow[] = [];
  const pending = roots.map((node) => ({ node, depth: 0 })).reverse();
  const seen = new Set<string>();
  while (pending.length) {
    const { node, depth } = pending.pop()!;
    if (seen.has(node.key) || (included && !included.has(node.key))) continue;
    seen.add(node.key);
    const children = included
      ? node.children.filter((child) => included.has(child.key))
      : node.children;
    const open = expanded === undefined || expanded.has(node.key);
    rows.push({
      key: node.key,
      session: node.session,
      depth,
      childCount: children.length,
      expanded: open && children.length > 0,
    });
    if (open) {
      for (let i = children.length - 1; i >= 0; i -= 1) {
        pending.push({ node: children[i], depth: depth + 1 });
      }
    }
  }
  return rows;
}
