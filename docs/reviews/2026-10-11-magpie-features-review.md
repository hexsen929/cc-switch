# 最近新增功能审查

审查日期：2026-10-11（Asia/Shanghai）。代码基准：`feat/magpie-inspired`，HEAD `52cc31db`。

范围：从 `63c3639b` 开始的多策略路由、会话粘性、请求/响应中间件、订阅账号故障转移、额度路由、意图路由、LAN 网关、原生 Codex 登录导入、脱敏与可靠性功能。检查了前后端配置接线及与现有协议转换路径的交互；这不是整个仓库的全面安全审计。

模块划分、配置持久化、默认关闭和纯函数测试已有基础。主要缺口是功能组合与协议边界：单个模块通过测试，但进入实际代理链路后，模型、凭证、SSE 事件、账号身份和用量记录的语义没有完全对齐。

以下共 18 项：4 项 P1，14 项 P2。标注“最小复现”的结论有独立运行证据；“静态调用链”表示已核对源码触发路径，但未连接真实供应商验证。所有问题均已在本次修改中处理，并为关键触发路径补了回归测试。位置行号对应审查基准 `52cc31db`，修复后可能有偏移。

## P1

### 1. 查询参数中的 LAN 网关密钥会发送给上游

位置：[gateway_auth.rs:312](/Users/mac/Downloads/cc-switch-fork/src-tauri/src/proxy/gateway_auth.rs:312)。验证：静态调用链。

使用支持的 `?key=ccs-...` 鉴权方式时，守卫只注入 `AuthedKey`，未消费查询参数。Gemini handler 的 `path_and_query` 原样进入 forwarder，GeminiAdapter 再将其拼进上游 URL。结果是本地网关密钥暴露给所选上游，也可能被其当作 Google API key 而导致鉴权失败。

建议：鉴权成功后清除承载网关凭证的参数/头，并由转发层独立注入供应商凭证；保留 `alt=sse` 等正常参数。补一个捕获上游完整 URI 的集成测试。

### 2. 关闭请求日志会使网关配额停止累计

位置：[fork_proxy.rs:821](/Users/mac/Downloads/cc-switch-fork/src-tauri/src/database/dao/fork_proxy.rs:821)。验证：静态调用链。

网关 token/cost 配额完全依赖 `SUM(proxy_request_logs)`。但 `response_processor` 的流式收集器和非流式记录入口在 `enable_logging=false` 时直接返回。关闭日志后，新密钥的统计一直为零；已有密钥也不会累计新增用量，日/周/月限额因此失效。

建议：将配额账本与可关闭、可清理的请求日志分离，或确保受配额约束的请求始终记账。测试应包含关闭日志后的多次请求及实际 429。

### 3. SSE 心跳会插进尚未完成的事件，破坏 JSON

位置：[response_processor.rs:819](/Users/mac/Downloads/cc-switch-fork/src-tauri/src/proxy/response_processor.rs:819)。验证：最小复现。

上游网络块不等于 SSE 事件。现实现每次等待块超时就插入 `: keep-alive\n\n`，没有检查是否处于事件中间。发送 `data: {"delta":"hel`、暂停超过心跳间隔，再发送 `lo"}\n\n`，下游收到被心跳截断的 JSON，解析失败。

建议：只在完整 SSE 事件边界插入心跳；缓冲或跟踪尚未完成的事件，并覆盖任意字节分块、UTF-8 分片、CRLF 等情况。

### 4. 原生 Responses 工具结果漏脱敏

位置：[pii_redaction.rs:147](/Users/mac/Downloads/cc-switch-fork/src-tauri/src/proxy/pii_redaction.rs:147)。验证：最小复现。

文本字段白名单漏掉 `function_call_output.output`。开启邮箱脱敏后，`{"input":[{"type":"function_call_output","output":"alice@example.com"}]}` 的命中数仍为 0，邮箱原样上传。文件读取和命令结果正是编码助手传递敏感内容的常见来源；同一结果转换到 Chat/Anthropic 的 `content` 时又会被脱敏。

建议：按协议和消息类型识别工具结果等文本槽位，避免仅凭全局键名判断；补原生 Responses 与协议转换后的等价测试。

## P2

### 5. Gemini 模型允许名单检查使用了错误的模型来源

位置：[handlers.rs:2953](/Users/mac/Downloads/cc-switch-fork/src-tauri/src/proxy/handlers.rs:2953)。验证：静态调用链。

`RequestContext::new` 先从 `body.model` 读取模型并检查 `allowed_models`，成功返回后才调用 `with_model_from_uri`。正常 Gemini 请求的模型在 URL 中，body 不含 model，因而按 `unknown` 校验；允许 `gemini-*` 的密钥反而收到 403。客户端提供的 body.model 也与实际 URL 模型脱节。意图路由按模型匹配同样受此顺序影响。

建议：在创建上下文、权限检查和意图路由之前统一解析实际请求模型；Gemini 使用 URI，不信任无关的 body.model。

### 6. 插入心跳后仍保留 Content-Length，导致截断

位置：[response_processor.rs:184](/Users/mac/Downloads/cc-switch-fork/src-tauri/src/proxy/response_processor.rs:184)。验证：最小复现（Hyper HTTP/1 + Axum，内存传输）。

通用流式响应复制了上游 `Content-Length`，心跳却增加了正文长度。上游两条事件原共 20 字节，中途插入心跳后，真实 HTTP 输出只有 `data: {}\n\n: keep-ali`，第二条事件丢失，而服务端仍返回成功。

建议：启用会改变正文的流处理时清理长度等实体头。此问题与第 3 项独立：即使只在正确事件边界插入，也会发生。

### 7. onEvent 流取消后可能遗留上游连接和阻塞线程

位置：[response_middleware.rs:363](/Users/mac/Downloads/cc-switch-fork/src-tauri/src/proxy/response_middleware.rs:363)。验证：最小复现。

reader 等待 `upstream.next()`，worker 等待 `raw_rx.blocking_recv()`；下游取消只有下次 `blocking_send` 时才会被发现。上游保持静默时，取消或外层 idle timeout 无法终止这两个任务。以永远 Pending 的上游验证，丢弃包装后的响应后，原上游仍未被 drop。反复发生会累积连接和阻塞线程。

建议：将下游生命周期与 reader/worker 的取消信号绑定，等待上游时同时监听取消；验证静默上游下的取消、超时和资源释放。

### 8. 原生 Codex 导入错误地合并不同账号身份

位置：[codex_oauth_auth.rs:719](/Users/mac/Downloads/cc-switch-fork/src-tauri/src/proxy/providers/codex_oauth_auth.rs:719)。验证：静态调用链。

`identity_match || workspace_match` 把同用户不同 workspace、同 workspace 不同用户都视为已托管。例如已有 Plus 账号，原生 CLI 切到同用户 Team 后，导入横幅被隐藏，直接调用导入命令也会返回旧账号。现有账号写入逻辑使用 user 与 workspace 联合身份，与这里不一致。

建议：复用现有联合身份判重规则，补 Plus/Team 切换、共享 Team 中不同用户、重复导入的测试。

### 9. 意图路由指定的目标模型会被供应商映射架空

位置：[handler_context.rs:294](/Users/mac/Downloads/cc-switch-fork/src-tauri/src/proxy/handler_context.rs:294)。验证：最小复现。

注入的 model-map 以入站 `request_model` 为 from，但 forwarder 先执行供应商映射，再执行该 map。入站 `claude-sonnet-4-5` 被映射为 `provider-sonnet` 后，意图指定的 `intent-haiku` 永远匹配不到，最终仍发往 `provider-sonnet`。对于模型已写入 URL 的 Gemini 路径，晚改 body.model 同样无法改变实际目标。

建议：把意图目标模型作为独立、明确优先级的路由结果，在构造协议请求和 URL 之前应用；测试应捕获最终 URL/body，而不只测试 map 函数。

### 10. “超长上下文”会命中普通短请求

位置：[intent_routing.rs:209](/Users/mac/Downloads/cc-switch-fork/src-tauri/src/proxy/intent_routing.rs:209)。验证：最小复现。

条件是 `est_input_tokens > 0`，`hello` 即命中。UI 语义意图模式没有长度阈值选项；该规则会把几乎所有普通请求送到目标供应商，并遮挡后续规则。现有测试也把短文本命中当成了正确结果。

建议：定义有意义的默认阈值或可配置阈值，覆盖阈值两侧，并修正固化当前错误的测试。

### 11. “带图片”会把音频、PDF 和工具声明当成图片

位置：[intent_routing.rs:383](/Users/mac/Downloads/cc-switch-fork/src-tauri/src/proxy/intent_routing.rs:383)。验证：音频/PDF 已最小复现，工具声明经静态核对。

见到任意 `inlineData` 就判真，未检查 MIME；任意对象的 type 含 image 也判真。Gemini 的 audio/wav、application/pdf 都会命中图片规则，无图片的 image_generation 工具声明也符合当前条件，导致错误选路。

建议：限定消息内容中的媒体块，并按明确的图片类型和 `image/*` MIME 判断。

### 12. 循环检测会中断合法 Responses 工具参数

位置：[reliability.rs:116](/Users/mac/Downloads/cc-switch-fork/src-tauri/src/proxy/reliability.rs:116)。验证：最小复现。

任意字符串 delta 都被当成文本，包括 `response.function_call_arguments.delta`。工具生成合法零值数组时，`0,` 重复 30 次就触发循环切断，完整参数无法交给客户端。

建议：按 SSE 事件类型区分文本与工具参数；至少排除函数参数，并测试合法重复数据结构。

### 13. Gemini 原生流的循环检测实际不生效

位置：[reliability.rs:106](/Users/mac/Downloads/cc-switch-fork/src-tauri/src/proxy/reliability.rs:106)。验证：最小复现与 handler 接线核对。

界面允许为 Gemini 开启 loop guard，但抽取函数未处理 `candidates[].content.parts[].text`。原生 Gemini 响应没有转换成现有三种增量形状，重复文本抽取结果一直为空，检测不会触发。

建议：补充 Gemini 文本提取及多 part 处理，或明确限制开关支持的协议。

### 14. Copilot 账号故障转移未同步切换 endpoint 和模型能力

位置：[forwarder.rs:2421](/Users/mac/Downloads/cc-switch-fork/src-tauri/src/proxy/forwarder.rs:2421)。验证：静态调用链。

这里将绑定账号 A 改选为 B 时，已按 A 决定动态 endpoint、模型能力和协议，实际只换成 B 的 token。账号分属不同企业端点或模型权限时，会把 B 的 token 发给 A 的 endpoint，或继续使用 A 的模型能力判断，故障转移仍失败。

建议：每次尝试先选择有效账号，再统一用于 endpoint、能力、协议、认证和健康记账。需要覆盖不同账号端点的模拟集成测试。

### 15. 会话粘性绑定的是首个候选，不是实际应答者

位置：[provider_router.rs:584](/Users/mac/Downloads/cc-switch-fork/src-tauri/src/proxy/provider_router.rs:584)。验证：静态调用链及全仓库调用点核对。

请求发送前即记住 P1，成功路径未按真正返回结果的供应商更新。P1 5xx、P2 成功且 P1 尚未熔断时，下轮仍先请求 P1；P1 恢复后对话跳回 P1。与 UI“固定上一轮应答的供应商”不一致，会降低缓存命中及对话状态连续性。

建议：在成功选择响应供应商后更新粘性；失败尝试不应覆盖实际成功绑定。测试 P1 失败/P2 成功后的下一轮。

### 16. Token 刷新鉴权失败不会触发订阅账号故障转移

位置：[forwarder.rs:1101](/Users/mac/Downloads/cc-switch-fork/src-tauri/src/proxy/forwarder.rs:1101)。验证：静态调用链。

账号内重试仅接受推理请求返回的 `UpstreamError 401/403`。若绑定账号的 refresh token 已失效，获取 token 阶段直接返回 `AuthError`，既不冷却该账号，也不尝试健康备用账号。Codex 列表里的 reauth_required 又只依据身份字段，不能保证排除这种失效账号，因此后续请求可能反复失败。

建议：对明确归属于某账号、明确不可用的 token 刷新鉴权错误执行冷却和备用账号重试，同时保留对配置错误等其它 AuthError 的区分。

### 17. Claude 的 system 预设注入不适配已转换的上游协议

位置：[request_rewrite.rs:138](/Users/mac/Downloads/cc-switch-fork/src-tauri/src/proxy/request_rewrite.rs:138)。验证：静态调用链。

预设在所有协议转换之后运行，却仅依据入站 `AppType::Claude` 注入顶层 system。Claude→OpenAI Chat/Responses/Gemini 时，此时 body 已不再是 Anthropic 格式；新增 system 不会修改真实的 messages/instructions/systemInstruction，严格上游还可能拒绝该字段。

建议：在协议转换前应用语义层 system 注入，或按实际出站协议改写对应字段；不要仅按客户端 AppType 判断。

### 18. 中文紧邻敏感值时，内置正则会漏脱敏

位置：[pii_redaction.rs:90](/Users/mac/Downloads/cc-switch-fork/src-tauri/src/proxy/pii_redaction.rs:90)。验证：最小复现。

内置模式使用 Unicode `\b`；汉字和数字都属于 word 字符，相接处没有该边界。开启信用卡脱敏，`卡号4111111111111111，请处理` 的命中数仍是 0。邮箱、AWS key 等使用相同边界的模式也应检查中文邻接情况。

建议：根据敏感值的实际字符集设计边界，增加中文、日文和标点邻接样例，同时避免匹配更长标识符中的子串。

## 已完成验证与限制

- `cargo test --manifest-path src-tauri/Cargo.toml --lib -- --test-threads=4`：3,815 通过，0 失败，9 忽略。
- `pnpm test:unit`：216 个文件、2,517 项通过。
- `pnpm typecheck`、`cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check`、`git diff --check`：通过。
- Release workflow 摘要验证涉及的 4 个 Vitest 文件：27 项通过。
- PII/意图路由复现：[源码](/private/tmp/cc-switch-redaction-intent-review.rs)、[输出](/private/tmp/cc-switch-redaction-intent-review.out)。直接引用真实模块，少量外部类型以 stub 提供。
- SSE 心跳复现：[事件分块](/private/tmp/cc-switch-keepalive-review.rs)、[HTTP 长度截断](/private/tmp/cc-switch-keepalive-content-length-review.rs)。使用从当前源码提取的原流函数；日志/usage 依赖以 stub 隔离，HTTP 长度验证使用真实 Hyper/Axum 内存连接。
- 循环检测复现：[源码](/private/tmp/cc-switch-reliability-review/src/main.rs)。直接引用真实 reliability 模块。
- onEvent 取消复现：[源码](/private/tmp/ccs-response-review/main.rs)。直接引用真实 response_middleware/sse 模块，ProxyResponse 与 AppError 以最小 stub 提供。

本次没有连接真实付费供应商、导入真实账号、执行真实 LAN 攻击验证或跑桌面 UI 手动流程；这些边界以本地模拟上游、数据库和纯函数回归覆盖。修复已覆盖最终 URI/凭证清理、Gemini URI 模型权限、账号/模型/endpoint 一致性、工具结果脱敏、SSE 分块心跳与 Content-Length、静默取消资源释放、配额账本和日志关闭、实际成功供应商粘性等组合路径。
