export interface ProxyConfig {
  listen_address: string;
  listen_port: number;
  max_retries: number;
  request_timeout: number;
  enable_logging: boolean;
  live_takeover_active?: boolean;
  // 超时配置
  streaming_first_byte_timeout: number;
  streaming_idle_timeout: number;
  non_streaming_timeout: number;
}

export interface ProxyStatus {
  running: boolean;
  address: string;
  port: number;
  active_connections: number;
  total_requests: number;
  success_requests: number;
  failed_requests: number;
  success_rate: number;
  uptime_seconds: number;
  current_provider: string | null;
  current_provider_id: string | null;
  last_request_at: string | null;
  last_error: string | null;
  failover_count: number;
  active_targets?: ActiveTarget[];
}

export interface ActiveTarget {
  app_type: string;
  provider_name: string;
  provider_id: string;
}

export interface ProxyServerInfo {
  address: string;
  port: number;
  started_at: string;
}

export interface ProxyTakeoverStatus {
  claude: boolean;
  "claude-desktop"?: boolean;
  codex: boolean;
  gemini: boolean;
  grokbuild: boolean;
  opencode: boolean;
  openclaw: boolean;
  hermes: boolean;
}

/** 应用当前的连接方式。 */
export type AppMode = "direct" | "route" | "stack";

/** 应用页模式行用的状态（后端 `get_app_mode`）。 */
export interface AppModeView {
  mode: AppMode;
  /** 客户端文件指着代理（CC Switch 运行时为真） */
  attached: boolean;
  /** 路由目标；直连模式下是上次路由的那家 */
  routeProviderId: string | null;
  /** 直连那家：直连时写进客户端的、回到直连时写回的 */
  directProviderId: string | null;
}

/** 启动时没能接上代理、已退回直连的应用。 */
export interface StartupAttachFailure {
  appType: string;
  stack: boolean;
  error: string;
}

/** Stack 模型：名单里的一家和它发布给客户端的模型 id。 */
export interface ProxyStackMember {
  providerId: string;
  modelIds: string[];
  /** 这家是默认那家（代理路由）：模型走默认路由，`modelIds` 等默认换到别家后才发布。 */
  route: boolean;
  /** 这家不做默认、只做成员时发布几个模型（换默认的预览用；默认那家的 `modelIds` 可能为空）。 */
  memberModels: number;
}

/**
 * Codex Stack 模型客户端看不到或看不全：`routeOwnsCatalog` 路由供应商使用自己的模型目录文件，
 * Stack 模型不发布；官方做路由时官方模型列表暂未取到：`officialModelsBundled` 暂用 Codex
 * 自带的列表（可能缺账号专属的模型），`officialModelsUnavailable` Stack 模型暂不可用；
 * `officialModelsOutdated` 本机 Codex 太旧，拉到的官方列表里没有能选的模型。
 */
export type ProxyStackNotice =
  | "routeOwnsCatalog"
  | "officialModelsBundled"
  | "officialModelsUnavailable"
  | "officialModelsOutdated";

/**
 * 官方模型列表没取到的原因（`officialModelsBundled` / `officialModelsUnavailable` 时）：先是官方
 * 列表的，不可用时再跟 Codex 自带列表的。
 */
export type ProxyStackNoticeReason =
  | "noLogin"
  | "loginStale"
  | "loginIncomplete"
  | "noVersion"
  | "fetchFailed"
  | "invalidList"
  | "noCli"
  | "cliFailed"
  | "nothingUsable";

/**
 * 还在用旧模型列表的 Codex 客户端（它们只在启动时读模型目录）：`daemon` 是 `codex` 命令行连的
 * 托管守护进程，可以一键重启；`others` 是桌面版、编辑器插件，要用户自己彻底退出再开。
 */
export interface CodexStaleClients {
  daemon: boolean;
  others: boolean;
  /** File-store account changed after startup; the process may cache the old login. */
  auth?: boolean;
  /**
   * 看不到桌面版、编辑器插件的进程（Windows）：`others` 只表示上次确认之后目录或登录变了，
   * 不知道有没有开着。关掉提示时要告诉后端（`acknowledgeCodexStaleClients`）。
   */
  unverified?: boolean;
}

/** Codex 聚合设置里「子代理模型」下拉的一项：发布的模型和它在合并目录里的推理强度档位。 */
export interface ProxyStackSubagentModel {
  /** 发布给客户端的模型 id（`ccs-<key>/<model>`），也是写进 `[agents]` 的值。 */
  id: string;
  providerId: string;
  /** `<模型显示名>（<供应商名>）`，和 Codex 模型选择器里一样。 */
  displayName: string;
  efforts: string[];
  defaultEffort?: string;
}

/** Stack 模式的状态、名单和提示。 */
export interface ProxyStack {
  /** 在 Stack 模式（代理模式且 Stack 模式开着）。 */
  active: boolean;
  members: ProxyStackMember[];
  notice?: ProxyStackNotice;
  noticeReasons?: ProxyStackNoticeReason[];
  staleClients?: CodexStaleClients;
  /** `staleClients` 对应的目录和登录：关掉提示后这个值变了（同一个供应商上又改了目录），重新显示。 */
  staleRevision?: string;
  /** Codex：聚合设置里「子代理模型」下拉的选项，发布 Stack 模型的成员按名单顺序。 */
  subagentModels?: ProxyStackSubagentModel[];
}

/** 重启 Codex 守护进程的结果：`notRunning` 表示它没在运行，什么都没做。 */
export type CodexDaemonRestartOutcome = "restarted" | "notRunning";

/** 增删 Stack 模型失败。`partial` 为真：已部分写入，下次操作或重启 CC Switch 时补完。 */
export interface ProxyStackWriteError {
  partial: boolean;
  message: string;
}

export interface ProviderHealth {
  provider_id: string;
  app_type: string;
  is_healthy: boolean;
  consecutive_failures: number;
  last_success_at: string | null;
  last_failure_at: string | null;
  last_error: string | null;
  updated_at: string;
}

// 熔断器相关类型
export interface CircuitBreakerConfig {
  failureThreshold: number;
  successThreshold: number;
  timeoutSeconds: number;
  errorRateThreshold: number;
  minRequests: number;
}

export type CircuitState = "closed" | "open" | "half_open";

export interface CircuitBreakerStats {
  state: CircuitState;
  consecutiveFailures: number;
  consecutiveSuccesses: number;
  totalRequests: number;
  failedRequests: number;
}

// 供应商健康状态枚举
export enum ProviderHealthStatus {
  Healthy = "healthy",
  Degraded = "degraded",
  Failed = "failed",
  Unknown = "unknown",
}

// 扩展 ProviderHealth 以包含前端计算的状态
export interface ProviderHealthWithStatus extends ProviderHealth {
  status: ProviderHealthStatus;
  circuitState?: CircuitState;
}

export interface ProxyUsageRecord {
  provider_id: string;
  app_type: string;
  endpoint: string;
  request_tokens: number | null;
  response_tokens: number | null;
  status_code: number;
  latency_ms: number;
  error: string | null;
  timestamp: string;
}

export interface ForkFailoverChainItem {
  nodeType: "provider" | "route_mode";
  nodeId: string;
  providerName?: string | null;
  sortIndex?: number;
}

// 故障转移队列条目
export interface FailoverQueueItem {
  providerId: string;
  providerName: string;
  providerNotes?: string;
  sortIndex?: number;
}

// 全局代理配置（统一字段，三行镜像）
export interface GlobalProxyConfig {
  proxyEnabled: boolean;
  listenAddress: string;
  listenPort: number;
  enableLogging: boolean;
}

export type ClaudeModelKey = "sonnet" | "opus" | "haiku" | "custom" | "unknown";
export type ClaudeModelFailoverMode = "round_robin" | "random";

export interface ClaudeModelRoutingSettings {
  routeEnabled: boolean;
  modelFailoverEnabled: boolean;
}

export interface ClaudeModelRoutePolicy {
  appType: string;
  modelKey: ClaudeModelKey;
  enabled: boolean;
  defaultProviderId: string | null;
  modelFailoverEnabled: boolean;
  modelFailoverMode: ClaudeModelFailoverMode;
  updatedAt: string;
}

// Provider 级路由策略（Fork 扩展，灵感来自 magpie 的 routing group 模式）
// order=队列顺序；rotate=轮询均摊；usage=最少使用优先；
// smart=额度感知·优先最快重置；pace=额度感知·按每小时剩余额度配速
export type ProviderRoutingStrategy =
  | "order"
  | "rotate"
  | "usage"
  | "smart"
  | "pace";

export const PROVIDER_ROUTING_STRATEGIES: ProviderRoutingStrategy[] = [
  "order",
  "rotate",
  "usage",
  "smart",
  "pace",
];

// 应用级代理配置（每个 app 独立）
export interface AppProxyConfig {
  appType: string;
  enabled: boolean;
  autoFailoverEnabled: boolean;
  /** @deprecated Read-only mirror of settings.preserveCodexOfficialAuthOnSwitch. */
  codexChatgptAuthTakeover: boolean;
  maxRetries: number;
  streamingFirstByteTimeout: number;
  streamingIdleTimeout: number;
  nonStreamingTimeout: number;
  circuitFailureThreshold: number;
  circuitSuccessThreshold: number;
  circuitTimeoutSeconds: number;
  circuitErrorRateThreshold: number;
  circuitMinRequests: number;
}

/** 声明式「现成请求中间件」（无代码预设）——与后端 request_rewrite.rs 对齐（camelCase）。 */
export interface ModelMapRule {
  from: string;
  to: string;
}

export type SystemPromptMode = "off" | "prepend" | "append" | "replace";

export interface SystemPromptRule {
  mode: SystemPromptMode;
  text: string;
}

export interface RequestRewriteConfig {
  enabled: boolean;
  modelMap: ModelMapRule[];
  /** 顶层参数覆盖（value=null 表示删除该键） */
  paramOverrides: Record<string, unknown>;
  systemPrompt: SystemPromptRule;
}

export const EMPTY_REQUEST_REWRITE: RequestRewriteConfig = {
  enabled: false,
  modelMap: [],
  paramOverrides: {},
  systemPrompt: { mode: "off", text: "" },
};

/** 响应侧单个钩子（onResponse / onEvent 各一份），默认关闭、脚本为空。 */
export interface ResponseHookConfig {
  enabled: boolean;
  /** 求值得到 `(x, meta) => any` 的表达式；x 为响应体 / 事件 JSON */
  script: string;
}

/** 响应侧可编程中间件配置（Fork 扩展，onResponse 非流式 / onEvent 流式）。 */
export interface ResponseMiddlewareConfig {
  onResponse: ResponseHookConfig;
  onEvent: ResponseHookConfig;
}

export const EMPTY_RESPONSE_MIDDLEWARE: ResponseMiddlewareConfig = {
  onResponse: { enabled: false, script: "" },
  onEvent: { enabled: false, script: "" },
};

/** 意图路由（Fork 扩展）——与后端 intent_routing.rs 对齐（camelCase + tagged matcher）。 */
export type IntentKind =
  | "background"
  | "thinking"
  | "long-context"
  | "web-search"
  | "subagent";

/** 按点号路径取 body 字段后做 equals / contains（两者都给时同时满足）。 */
export interface IntentBodyFieldMatch {
  path: string;
  equals?: string | null;
  contains?: string | null;
}

/** 本地时间窗：days 空=每天（0=周一..6=周日）；start/end 为 "HH:MM"（留空=全天）。 */
export interface IntentTimeWindow {
  days: number[];
  start: string;
  end: string;
}

/** 自定义谓词：各字段可选且 AND，全部提供的条件都满足才命中（空谓词不命中）。 */
export interface IntentPredicate {
  modelGlob?: string | null;
  bodyField?: IntentBodyFieldMatch | null;
  minInputTokens?: number | null;
  hasTool?: string | null;
  /** 命中 Claude Code 上下文压缩（compact）请求。 */
  compaction?: boolean | null;
  /** 请求体是否带图片内容（多模态）。 */
  hasImage?: boolean | null;
  /** 本地时间窗。 */
  timeWindow?: IntentTimeWindow | null;
}

/** matcher：内置语义意图 或 自定义谓词（serde internally tagged by `type`）。 */
export type IntentMatcher =
  | { type: "intent"; intent: IntentKind }
  | { type: "predicate"; predicate: IntentPredicate };

/** 单条意图路由规则：命中后把 targetProviderId 软置顶为失败转移链 P1，可选改写上游模型。 */
export interface IntentRule {
  id: string;
  name: string;
  enabled: boolean;
  matcher: IntentMatcher;
  targetProviderId: string;
  model?: string | null;
}

export interface IntentRoutingConfig {
  enabled: boolean;
  rules: IntentRule[];
}

export const EMPTY_INTENT_ROUTING: IntentRoutingConfig = {
  enabled: false,
  rules: [],
};

// ==================== 局域网网关（Fork 扩展，默认关闭） ====================

/** 列表视图：token 已脱敏为末 4 位（`••••abcd`），用于管理界面展示。 */
export interface GatewayKeyView {
  id: string;
  name: string;
  tokenMasked: string;
  enabled: boolean;
  createdAt: string;
  /** per-key caps（允许表）：空数组 = 不限；allowedModels 支持 `*` glob。 */
  allowedApps: string[];
  allowedProviders: string[];
  allowedModels: string[];
  /** per-key 配额窗口：none|day|week|month；limitTokens/limitCostUsd 为 null = 该维度不限。 */
  limitWindow: string;
  limitTokens: number | null;
  limitCostUsd: number | null;
}

/** 完整密钥：仅在「新增 / 轮换」后一次性返回，含明文 token。 */
export interface GatewayKey {
  id: string;
  name: string;
  token: string;
  enabled: boolean;
  createdAt: string;
}

/** 配额窗口可选值。 */
export const GATEWAY_LIMIT_WINDOWS = ["none", "day", "week", "month"] as const;
export type GatewayLimitWindow = (typeof GATEWAY_LIMIT_WINDOWS)[number];

// ==================== 出站请求脱敏（Fork 扩展，默认关闭） ====================

/** 内置高置信度脱敏模式开关（低误伤）。 */
export interface PiiBuiltinToggles {
  creditCard: boolean;
  usSsn: boolean;
  iban: boolean;
  email: boolean;
  awsAccessKeyId: boolean;
  privateKeyBlock: boolean;
  jwt: boolean;
}

/** 自定义脱敏规则：name 作标签，pattern 为正则（Rust regex 语法）。 */
export interface PiiCustomRule {
  name: string;
  pattern: string;
}

export interface PiiRedactionConfig {
  enabled: boolean;
  builtins: PiiBuiltinToggles;
  customRules: PiiCustomRule[];
  /** 占位符模板，支持 {label}；留空用默认 [REDACTED:{label}]。 */
  placeholder: string;
}

export const EMPTY_PII_BUILTINS: PiiBuiltinToggles = {
  creditCard: false,
  usSsn: false,
  iban: false,
  email: false,
  awsAccessKeyId: false,
  privateKeyBlock: false,
  jwt: false,
};

export const EMPTY_PII_REDACTION: PiiRedactionConfig = {
  enabled: false,
  builtins: { ...EMPTY_PII_BUILTINS },
  customRules: [],
  placeholder: "",
};

// ==================== 代理可靠性（Fork 扩展，默认全关） ====================

export interface ReliabilityConfig {
  /** 上游静默时发 SSE keep-alive 注释。 */
  keepaliveEnabled: boolean;
  /** keep-alive 间隔（秒）；启用但 0 时后端回落 15，上限 300。 */
  keepaliveIntervalSecs: number;
  /** 流式死循环检测并切断。 */
  loopGuard: boolean;
  /** 2xx 非 API 响应（HTML/CF 挑战页）→ 失败转移。 */
  nonApiFailover: boolean;
}

export const EMPTY_RELIABILITY: ReliabilityConfig = {
  keepaliveEnabled: false,
  keepaliveIntervalSecs: 15,
  loopGuard: false,
  nonApiFailover: false,
};
