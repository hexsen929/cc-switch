import { invoke } from "@tauri-apps/api/core";
import type {
  ProxyStatus,
  ProxyServerInfo,
  ProxyTakeoverStatus,
  GlobalProxyConfig,
  AppProxyConfig,
  ClaudeModelRoutingSettings,
  ClaudeModelRoutePolicy,
  ProviderRoutingStrategy,
  ProxyStack,
  ProxyStackNotice,
  CodexDaemonRestartOutcome,
  AppModeView,
  StartupAttachFailure,
  RequestRewriteConfig,
  ResponseMiddlewareConfig,
  IntentRoutingConfig,
  GatewayKey,
  GatewayKeyView,
} from "@/types/proxy";
import { PROVIDER_ROUTING_STRATEGIES } from "@/types/proxy";

function normalizeRoutingStrategy(value: string): ProviderRoutingStrategy {
  return (PROVIDER_ROUTING_STRATEGIES as string[]).includes(value)
    ? (value as ProviderRoutingStrategy)
    : "order";
}

export const proxyApi = {
  // ========== 代理服务器控制 API ==========

  // 启动代理服务器
  async startProxyServer(): Promise<ProxyServerInfo> {
    return invoke("start_proxy_server");
  },

  // 停止代理服务器（不恢复已接管配置）
  async stopProxyServer(): Promise<void> {
    return invoke("stop_proxy_server");
  },

  // 停止代理服务器并恢复配置
  async stopProxyWithRestore(): Promise<void> {
    return invoke("stop_proxy_with_restore");
  },

  // 获取代理服务器状态
  async getProxyStatus(): Promise<ProxyStatus> {
    return invoke("get_proxy_status");
  },

  // ========== 接管状态 API ==========

  // 获取各应用接管状态
  async getProxyTakeoverStatus(): Promise<ProxyTakeoverStatus> {
    return invoke("get_proxy_takeover_status");
  },

  // 为指定应用进入/退出代理模式。stack 为真时进入的是 Stack 模式（和路由模式二选一）；
  // route 是确认框里选的路由目标（Stack 模式下是默认那家），不传沿用上次的路由
  async setProxyTakeoverForApp(
    appType: string,
    enabled: boolean,
    stack = false,
    route?: string | null,
  ): Promise<void> {
    return invoke("set_proxy_takeover_for_app", {
      appType,
      enabled,
      stack,
      route: route ?? null,
    });
  },

  // 指定路由目标（聚合模式下是默认那家）：直连时只记下来、下次进入路由 / 聚合模式时用它，
  // 已经在路由 / 聚合模式时当场生效
  async setProxyRoute(appType: string, providerId: string): Promise<void> {
    return invoke("set_proxy_route", { appType, providerId });
  },

  // 应用页模式行：生效的模式、路由目标（直连时是上次路由的那家）、直连那家
  async getAppMode(appType: string): Promise<AppModeView> {
    return invoke("get_app_mode", { appType });
  },

  // 启动时没能接上代理、已退回直连的应用（取一次就清空）
  async takeStartupAttachFailures(): Promise<StartupAttachFailure[]> {
    return invoke("take_startup_attach_failures");
  },

  // 设置里在路由和 Stack 之间换时：处于另一种模式（stack 为真是 Stack 模式）的 Claude Code、
  // Codex 先退回直连。返回退回直连的应用
  async exitProxyAppsInMode(stack: boolean): Promise<string[]> {
    return invoke("exit_proxy_apps_in_mode", { stack });
  },

  // 直连供应商：路由模式下退出路由时写回的那家（和路由到的那家互相独立）
  async getDirectProvider(appType: string): Promise<string | null> {
    return invoke("get_direct_provider", { appType });
  },

  // ========== Stack 模型 API ==========

  // Stack 模型名单：每一家和它发布的模型 id，以及提示
  async getProxyStack(appType: string): Promise<ProxyStack> {
    return invoke("get_proxy_stack", { appType });
  },

  // 把一家加入或移出 Stack 模型（enabled 是目标值）。成功时返回客户端看不到或看不全 Stack 模型
  // 的提示；失败时抛出 ProxyStackWriteError
  async setProxyStackMember(
    appType: string,
    providerId: string,
    enabled: boolean,
  ): Promise<ProxyStackNotice | null> {
    return invoke("set_proxy_stack_member", { appType, providerId, enabled });
  },

  // Codex 聚合的模型被别的模型目录挡住时，改用 CC Switch 生成的目录（去掉指向别的文件的
  // model_catalog_json）。返回之后还剩的提示
  async adoptCodexStackCatalog(): Promise<ProxyStackNotice | null> {
    return invoke("adopt_codex_stack_catalog");
  },

  // 重启 Codex 的托管守护进程（codex 命令行连的那个），让它重读模型目录。会中断正在运行的
  // 任务，只在用户确认之后调
  async restartCodexAppServerDaemon(): Promise<CodexDaemonRestartOutcome> {
    return invoke("restart_codex_app_server_daemon");
  },

  // ========== v3+ 全局/应用级配置 API ==========

  // 获取全局代理配置
  async getGlobalProxyConfig(): Promise<GlobalProxyConfig> {
    return invoke("get_global_proxy_config");
  },

  // 更新全局代理配置
  async updateGlobalProxyConfig(config: GlobalProxyConfig): Promise<void> {
    return invoke("update_global_proxy_config", { config });
  },

  // 获取指定应用的代理配置
  async getProxyConfigForApp(appType: string): Promise<AppProxyConfig> {
    return invoke("get_proxy_config_for_app", { appType });
  },

  // 更新指定应用的代理配置
  async updateProxyConfigForApp(config: AppProxyConfig): Promise<void> {
    return invoke("update_proxy_config_for_app", { config });
  },

  // ========== Claude 模型路由（Fork 扩展） ==========

  async getClaudeModelRoutingSettings(): Promise<ClaudeModelRoutingSettings> {
    return invoke("get_claude_model_routing_settings");
  },

  async setClaudeModelRoutingSettings(
    settings: ClaudeModelRoutingSettings,
  ): Promise<void> {
    return invoke("set_claude_model_routing_settings", { settings });
  },

  async listClaudeModelRoutePolicies(): Promise<ClaudeModelRoutePolicy[]> {
    return invoke("list_claude_model_route_policies");
  },

  async upsertClaudeModelRoutePolicy(
    policy: ClaudeModelRoutePolicy,
  ): Promise<void> {
    return invoke("upsert_claude_model_route_policy", { policy });
  },

  // Provider 级路由策略（Fork 扩展）：order | rotate | usage
  // 作用于「自动故障转移」开启、候选 Provider ≥ 2 时的主路由与降级顺序
  async getProviderRoutingStrategy(
    appType: string,
  ): Promise<ProviderRoutingStrategy> {
    const value = await invoke<string>("get_provider_routing_strategy", {
      appType,
    });
    return normalizeRoutingStrategy(value);
  },

  async setProviderRoutingStrategy(
    appType: string,
    strategy: ProviderRoutingStrategy,
  ): Promise<void> {
    return invoke("set_provider_routing_strategy", { appType, strategy });
  },

  // 会话粘性（Fork 扩展）：开启后同一对话尽量固定在上一轮应答的 Provider 上，
  // 避免 rotate / usage 策略逐请求换家破坏上游 prompt 缓存
  async getProviderStickyEnabled(appType: string): Promise<boolean> {
    return invoke<boolean>("get_provider_sticky_enabled", { appType });
  },

  async setProviderStickyEnabled(
    appType: string,
    enabled: boolean,
  ): Promise<void> {
    return invoke("set_provider_sticky_enabled", { appType, enabled });
  },

  // 订阅内多账号故障转移（Fork 扩展）：开启后托管 OAuth 账号（Copilot/Codex/xAI）
  // 收到 401/403/429 会进入短暂冷却，同一订阅池内后续请求自动改选其它健康账号；
  // 默认关闭，关闭时账号解析行为与改造前完全一致
  async getSubscriptionAccountFailoverEnabled(
    appType: string,
  ): Promise<boolean> {
    return invoke<boolean>("get_subscription_account_failover_enabled", {
      appType,
    });
  },

  async setSubscriptionAccountFailoverEnabled(
    appType: string,
    enabled: boolean,
  ): Promise<void> {
    return invoke("set_subscription_account_failover_enabled", {
      appType,
      enabled,
    });
  },

  // 可编程请求中间件（Fork 扩展）：用户 onRequest JS 脚本，沙箱执行、默认关闭、失败放行
  async getRequestMiddlewareEnabled(appType: string): Promise<boolean> {
    return invoke<boolean>("get_request_middleware_enabled", { appType });
  },

  async setRequestMiddlewareEnabled(
    appType: string,
    enabled: boolean,
  ): Promise<void> {
    return invoke("set_request_middleware_enabled", { appType, enabled });
  },

  async getRequestMiddlewareScript(appType: string): Promise<string> {
    return invoke<string>("get_request_middleware_script", { appType });
  },

  async setRequestMiddlewareScript(
    appType: string,
    script: string,
  ): Promise<void> {
    return invoke("set_request_middleware_script", { appType, script });
  },

  // 声明式请求改写预设（Fork 扩展）：无代码「现成中间件」——改模型名 / 覆盖参数 / 注入 system
  async getRequestRewriteConfig(
    appType: string,
  ): Promise<RequestRewriteConfig> {
    return invoke<RequestRewriteConfig>("get_request_rewrite_config", {
      appType,
    });
  },

  async setRequestRewriteConfig(
    appType: string,
    config: RequestRewriteConfig,
  ): Promise<void> {
    return invoke("set_request_rewrite_config", { appType, config });
  },

  // 响应侧可编程中间件（Fork 扩展）：onResponse 非流式 / onEvent 流式
  async getResponseMiddlewareConfig(
    appType: string,
  ): Promise<ResponseMiddlewareConfig> {
    return invoke<ResponseMiddlewareConfig>("get_response_middleware_config", {
      appType,
    });
  },

  async setResponseMiddlewareConfig(
    appType: string,
    config: ResponseMiddlewareConfig,
  ): Promise<void> {
    return invoke("set_response_middleware_config", { appType, config });
  },

  // 意图路由（Fork 扩展）：按请求意图把目标 provider 软置顶为失败转移链 P1，可选改写上游模型
  async getIntentRoutingConfig(appType: string): Promise<IntentRoutingConfig> {
    return invoke<IntentRoutingConfig>("get_intent_routing_config", {
      appType,
    });
  },

  async setIntentRoutingConfig(
    appType: string,
    config: IntentRoutingConfig,
  ): Promise<void> {
    return invoke("set_intent_routing_config", { appType, config });
  },

  // ========== 局域网网关（Fork 扩展，默认关闭 + 强制 per-key 鉴权） ==========

  async getLanShareEnabled(): Promise<boolean> {
    return invoke<boolean>("get_lan_share_enabled");
  },

  async setLanShareEnabled(enabled: boolean): Promise<void> {
    return invoke("set_lan_share_enabled", { enabled });
  },

  async listGatewayKeys(): Promise<GatewayKeyView[]> {
    return invoke<GatewayKeyView[]>("list_gateway_keys");
  },

  /** 新增密钥：customToken 为空时后端生成 `ccs-<uuid>`；返回含明文 token 的完整记录。 */
  async addGatewayKey(
    name: string,
    customToken?: string | null,
  ): Promise<GatewayKey> {
    return invoke<GatewayKey>("add_gateway_key", {
      name,
      customToken: customToken ?? null,
    });
  },

  /** 轮换 token：返回新的完整记录；id 不存在返回 null。 */
  async rotateGatewayKey(id: string): Promise<GatewayKey | null> {
    return invoke<GatewayKey | null>("rotate_gateway_key", { id });
  },

  async setGatewayKeyEnabled(id: string, enabled: boolean): Promise<void> {
    return invoke("set_gateway_key_enabled", { id, enabled });
  },

  async renameGatewayKey(id: string, name: string): Promise<void> {
    return invoke("rename_gateway_key", { id, name });
  },

  async removeGatewayKey(id: string): Promise<void> {
    return invoke("remove_gateway_key", { id });
  },

  // ========== 计费默认配置 API ==========

  // 获取计费模式来源
  async getPricingModelSource(appType: string): Promise<string> {
    return invoke("get_pricing_model_source", { appType });
  },

  // 设置计费模式来源
  async setPricingModelSource(appType: string, value: string): Promise<void> {
    return invoke("set_pricing_model_source", { appType, value });
  },
};
