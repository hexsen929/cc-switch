import type { IntentTarget, SmartIntentConfig } from "@/types/proxy";

export function validateSmartGraph(
  root: SmartIntentConfig,
  cards: string[],
): string | undefined {
  const groups = new Map(Object.entries(root.groups ?? {}));
  if (groups.size > 16) return "groupLimit";
  const edges = new Map<string, string[]>();
  for (const [id, node] of [["$root", root] as const, ...groups]) {
    const next: string[] = [];
    const leaf = (t: IntentTarget) =>
      !t.groupId && cards.includes(t.providerId) && !!t.model.trim();
    if (
      !Number.isInteger(node.timeoutMs) ||
      node.timeoutMs < 200 ||
      node.timeoutMs > 8000
    )
      return "invalidTimeout";
    const threshold = node.minConfidence ?? 40;
    if (!Number.isInteger(threshold) || threshold < 0 || threshold > 100)
      return "invalidConfidence";
    const fixed =
      node.orderedOnly || (!node.classifiers.length && !node.categories.length);
    if (fixed && !node.members?.length) return "emptyGroup";
    if (
      !fixed &&
      (!node.classifiers.length ||
        !node.classifiers.every(leaf) ||
        node.categories.length < 2 ||
        node.categories.some((c) => !c.description.trim() || !c.targets.length))
    )
      return "invalidTargets";
    const targets = [
      ...(node.members ?? []),
      ...(fixed ? [] : node.categories.flatMap((c) => c.targets)),
    ];
    for (const t of targets) {
      if (t.groupId) {
        if (!groups.has(t.groupId)) return "missingGroup";
        next.push(t.groupId);
      } else if (!leaf(t)) return "invalidTargets";
    }
    edges.set(id, next);
  }
  const heights = new Map<string, number>();
  const path = new Set<string>();
  const height = (id: string): number => {
    if (path.has(id)) return Infinity;
    const known = heights.get(id);
    if (known !== undefined) return known;
    path.add(id);
    const h = 1 + Math.max(0, ...(edges.get(id) ?? []).map(height));
    path.delete(id);
    heights.set(id, h);
    return h;
  };
  for (const id of edges.keys()) {
    const h = height(id);
    if (!Number.isFinite(h)) return "groupCycle";
    if (h > 8) return "groupDepth";
  }
}
