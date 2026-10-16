import { describe, expect, it } from "vitest";
import type { SessionMeta } from "@/types";
import {
  buildSessionForest,
  flattenSessionTree,
  MAX_SESSION_TREE_DEPTH,
  sessionSearchContext,
} from "@/components/sessions/sessionTree";
import {
  canDeleteSession,
  getSessionKey,
  isReadOnlySession,
} from "@/components/sessions/utils";

const session = (
  id: string,
  parent?: string,
  providerId = "codex",
): SessionMeta => ({
  providerId,
  sessionId: id,
  parentSessionId: parent,
  sourcePath: `/sessions/${providerId}/${id}.jsonl`,
  ...(parent ? { role: "subagent" as const, readOnly: true } : {}),
});

describe("session trees", () => {
  it("keeps top-level order, nests same-app children, and leaves orphans readable", () => {
    const first = session("first");
    const child = session("child", "first");
    const grandchild = session("grandchild", "child");
    const other = session("other");
    const orphan = session("orphan", "missing");
    const foreign = session("foreign", "first", "claude");
    const forest = buildSessionForest([
      first,
      child,
      grandchild,
      other,
      orphan,
      foreign,
    ]);

    expect(forest.roots.map((node) => node.session.sessionId)).toEqual([
      "first",
      "other",
      "orphan",
      "foreign",
    ]);
    expect(
      flattenSessionTree(forest.roots).map((row) => [
        row.session.sessionId,
        row.depth,
      ]),
    ).toEqual([
      ["first", 0],
      ["child", 1],
      ["grandchild", 2],
      ["other", 0],
      ["orphan", 0],
      ["foreign", 0],
    ]);
    expect(
      flattenSessionTree(forest.roots, undefined, new Set()).map(
        (row) => row.session.sessionId,
      ),
    ).toEqual(["first", "other", "orphan", "foreign"]);
    expect(
      flattenSessionTree(
        forest.roots,
        undefined,
        new Set([getSessionKey(first)]),
      ).map((row) => row.session.sessionId),
    ).toEqual(["first", "child", "other", "orphan", "foreign"]);
    expect(isReadOnlySession(orphan)).toBe(true);
    expect(canDeleteSession(orphan)).toBe(false);
    expect(
      canDeleteSession({
        ...orphan,
        parentSessionId: undefined,
        role: undefined,
      }),
    ).toBe(false);
    expect(canDeleteSession(first)).toBe(true);
  });

  it("search includes the ancestor path and matched subtree, without unrelated siblings", () => {
    const root = session("root");
    const child = session("child", "root");
    const grandchild = session("grandchild", "child");
    const sibling = session("sibling", "root");
    const forest = buildSessionForest([root, child, grandchild, sibling]);
    const result = (hits: SessionMeta[]) =>
      flattenSessionTree(forest.roots, sessionSearchContext(forest, hits)).map(
        (row) => row.session.sessionId,
      );
    expect(result([grandchild])).toEqual(["root", "child", "grandchild"]);
    expect(result([child])).toEqual(["root", "child", "grandchild"]);
    expect(result([root])).toEqual(["root", "child", "grandchild", "sibling"]);
    expect(result([])).toEqual([]);
  });

  it("does not connect a copied parent ID in another project", () => {
    const parent = { ...session("root"), projectDir: "/project/a" };
    const copiedParent = {
      ...session("root"),
      projectDir: "/project/b",
      sourcePath: "/project/b/root.jsonl",
    };
    const child = { ...session("child", "root"), projectDir: "/project/a" };
    const orphan = { ...session("orphan", "root"), projectDir: "/project/c" };
    const forest = buildSessionForest([parent, copiedParent, child, orphan]);
    expect(forest.nodes.get(getSessionKey(child))?.parent?.key).toBe(
      getSessionKey(parent),
    );
    expect(forest.nodes.get(getSessionKey(orphan))?.parent).toBeUndefined();
    expect(forest.nodes.get(getSessionKey(copiedParent))?.children).toEqual([]);
  });

  it("breaks cycles, self-parent links, ambiguous parents and excessive depth without losing sessions", () => {
    const broken = [
      session("a", "b"),
      session("b", "a"),
      session("self", "self"),
    ];
    const duplicate = session("duplicate");
    broken.push(
      duplicate,
      { ...duplicate, sourcePath: "/archived/duplicate.jsonl" },
      session("ambiguous", "duplicate"),
    );
    for (let i = 0; i < 200; i += 1)
      broken.push(session(`deep-${i}`, i ? `deep-${i - 1}` : undefined));
    const forest = buildSessionForest(broken);
    const rows = flattenSessionTree(forest.roots);
    expect(rows).toHaveLength(broken.length);
    expect(new Set(rows.map((row) => row.key)).size).toBe(broken.length);
    expect(Math.max(...rows.map((row) => row.depth))).toBeLessThanOrEqual(
      MAX_SESSION_TREE_DEPTH,
    );
    expect(forest.nodes.get(getSessionKey(broken[5]))?.parent).toBeUndefined();
  });
});
