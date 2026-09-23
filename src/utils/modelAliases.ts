/**
 * 精确模型别名（modelAliases）共享工具
 *
 * 存储形态：`settings_config.modelAliases = { "源模型名": "目标模型名" }`。
 * 与 modelCatalog / 家族映射解耦，代理转发前对请求 `model` 做严格等值改写，
 * 优先级最高（后端见 `proxy/model_mapper.rs`）。Codex 与 Claude 供应商共用。
 */

/** 表单内的一条别名（带稳定 id 以便 React 列表渲染不丢焦点） */
export interface ModelAliasEntry {
  id: string;
  /** 源模型名（客户端实际发出的模型，例如 codex-auto-review） */
  source: string;
  /** 目标模型名（改写后发往上游的模型，例如 gpt-5.6-sol） */
  target: string;
}

function randomId(): string {
  if (
    typeof crypto !== "undefined" &&
    typeof crypto.randomUUID === "function"
  ) {
    return crypto.randomUUID();
  }
  return `alias-${Math.random().toString(36).slice(2)}-${Date.now()}`;
}

export function createModelAliasEntry(
  seed?: Partial<Pick<ModelAliasEntry, "source" | "target">>,
): ModelAliasEntry {
  return {
    id: randomId(),
    source: seed?.source ?? "",
    target: seed?.target ?? "",
  };
}

/**
 * 从 `settings_config.modelAliases`（对象形态）解析为表单可编辑的有序数组。
 * 非对象、空值、非字符串条目都会被安全跳过。
 */
export function parseModelAliasesFromValue(raw: unknown): ModelAliasEntry[] {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return [];
  const entries: ModelAliasEntry[] = [];
  for (const [source, target] of Object.entries(raw as Record<string, unknown>)) {
    if (typeof target !== "string") continue;
    entries.push(createModelAliasEntry({ source, target }));
  }
  return entries;
}

/**
 * 把表单数组序列化回 `{ source: target }` 对象用于持久化。
 * - 两端 trim；源或目标为空的行丢弃。
 * - 同一 source 多次出现时“首个非空”生效（保序，后写不覆盖）。
 * - 全部为空时返回 undefined，调用方据此决定是否写入该字段。
 */
export function serializeModelAliases(
  entries: ModelAliasEntry[],
): Record<string, string> | undefined {
  const out: Record<string, string> = {};
  for (const entry of entries) {
    const source = entry.source.trim();
    const target = entry.target.trim();
    if (!source || !target) continue;
    if (Object.prototype.hasOwnProperty.call(out, source)) continue;
    out[source] = target;
  }
  return Object.keys(out).length > 0 ? out : undefined;
}

/** 判断两个别名对象是否等价（用于回填 dedupe，避免无意义 re-render） */
export function modelAliasesEqual(
  left: Record<string, string> | undefined,
  right: Record<string, string> | undefined,
): boolean {
  const l = left ?? {};
  const r = right ?? {};
  const lk = Object.keys(l);
  const rk = Object.keys(r);
  if (lk.length !== rk.length) return false;
  return lk.every((key) => l[key] === r[key]);
}
