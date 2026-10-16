# Fork 维护说明

本项目 fork 自 [farion1231/cc-switch](https://github.com/farion1231/cc-switch)，在原项目基础上新增了以下功能。

---

## 新增功能

### 1. OpenAI 兼容接口模型获取

允许用户在配置 Provider 时，通过填写 API 端点和密钥，直接获取该接口支持的模型列表，并在下拉框中选择。

**涉及文件：**

| 文件 | 改动说明 |
|------|----------|
| `src-tauri/src/services/provider/models.rs` | **新增文件**，实现 `fetch_openai_models` 函数，向目标接口发起请求并返回模型列表 |
| `src-tauri/src/services/provider/mod.rs` | 新增 `mod models;` 声明和 `pub use models::{fetch_openai_models, FetchOpenAiModelsResponse};` 导出 |
| `src-tauri/src/commands/provider.rs` | 新增 `fetch_provider_models_openai` Tauri 命令及 `FetchOpenAiModelsResponse` 导入 |
| `src-tauri/src/lib.rs` | 在 `invoke_handler` 中注册 `commands::fetch_provider_models_openai` |
| `src/lib/api/providers.ts` | 在 `providersApi` 对象末尾新增 `fetchOpenAiModels` 方法 |
| `src/components/ui/model-suggest.tsx` | **新增文件**，前端模型选择 UI 组件 |
| `src/components/providers/forms/ProviderForm.tsx` | 集成模型获取按钮和模型下拉选择 |

### 2. Claude Provider 运行时指令文件

每个 Claude Provider 可以独立维护并启用一个系统指令文件和一个追加指令文件。保存或切换 Provider 时，CC Switch 会将当前选择投影到 Claude 配置目录下的固定运行时文件：

- `--system-prompt-file` -> `cc-switch/system-prompt.md`
- `--append-system-prompt-file` -> `cc-switch/append-prompt.md`

Shell 集成只在文件存在且非空、且用户未手动提供同类参数时加载这些文件。安装、升级和卸载均由 Provider 表单中的“Claude Shell 集成”卡片管理；不会修改 `settings.json` 或普通 `CLAUDE.md` 提示词预设。

### 3. 更新地址

`src-tauri/tauri.conf.json` 中的更新端点和签名公钥已替换为本仓库的配置。

### Codex 配置兼容

当前模板及写入链路不再输出 Codex 已忽略的 `disable_response_storage`、
`preferred_auth_method`、`features.remote_connections`、
`features.code_mode_tool_description_first` 和
`features.guardianv2.thread_context`。清理限定于这些 TOML 路径及内嵌
`profiles`，保留其它设置、路由和 `auth.json`；不会改成语义不同的
`forced_login_method`。

启动时先在本机 fork 数据库中保存旧供应商/通用片段快照，再原子迁移；
当前配置文件通过既有文件锁、备份及冲突检测写入。以后保存、切换、MCP
同步和压缩设置变更也使用相同清理逻辑，避免旧快照重新带回这些字段。

GitHub CI 额外使用固定官方 Codex `0.162.1` 严格加载实际模板和预设，
详见 [配置兼容契约](docs/codex-config-contract.md)。该检查不代表所有旧版本
Codex 都具有相同配置语义，也不测试模型请求或认证。桌面应用通过
`session-flags` 注入的内部参数不属于磁盘配置，须由桌面应用更新修正。

### 4. Magpie 功能适配

参考 [Magpie](https://github.com/yetone/magpie/tree/674b6ca2881f1289c4ccdec158762978a463320c)
的交互与协议处理规则，在现有 Rust / React 架构中接入以下功能：

- **子代理会话树**：Claude、Codex、OpenCode 子会话按所属项目归入父会话，支持展开、搜索、阅读和 Markdown 导出。子会话只读；恢复和删除在前后端均被阻止，继续操作应回到主会话。缺失或不明确的父关系保留为顶层记录，不跨项目猜测。
- **请求诊断**：在用量页打开代理请求详情，可查看各次尝试的供应商、状态和实际测得的阶段耗时。正文预览独立控制、默认关闭，开启后只在内存保留最近 40 个请求；请求和响应各限 128 KiB。预览展示最后一次上游尝试，发生客户端协议转换前的正文；不采集鉴权请求头，不能可靠脱敏的损坏或截断 JSON 使用占位内容。
- **Codex Responses 消息阶段**：仅为本程序转换生成的 Responses 推断 `commentary` 与 `final_answer`。工具前说明与最终回答分开，失败、截断、拒答及未知结束原因不伪造最终回答；官方原生透传保持原样。
- **输入超限兼容**：HTTP 413 返回 `request_too_large`；明确的模型输入上限拒绝可沿已有故障转移队列继续，不改变官方登录单账号和 Stack 边界，也不将普通 400、限速或输出参数错误当成可转移的上下文问题。
- **统一搜索 MCP**：在「设置 → 网络」启用并配置搜索源，再选择客户端「保存并添加到 MCP」。支持 Tavily、Brave、Exa、Firecrawl、You.com 和 SearXNG，多密钥轮询或按序尝试、限流退避和来源排序；Tavily 可查询额度。`web_fetch` 仅访问公网文本网页。服务复用现有网关，启用后自动启动，关闭搜索不停止其他模型路由。局域网调用受网关密钥权限和独立每日工具调用上限约束；工具调用不会虚构成大模型 token 消耗。

搜索功能默认关闭，不取代客户端原有联网工具；搜索 API 密钥保存在本机设置中，前端普通设置响应和网关工具响应不返回原文密钥。模型代理诊断也不会自动开启正文记录。

参考项目为 MIT 许可；上述功能按本项目的供应商模式、会话适配器、MCP 同步和代理生命周期实现，未引入 Magpie 的 Go / Bun 运行时或插件系统。编译与回归测试由 GitHub Actions 执行。

---

## 构建与发布

### 自动构建（推荐）

推送带 `v` 前缀的 tag 即可触发 GitHub Actions 自动在 macOS / Windows / Linux 上构建并发布：

```bash
git add .
git commit -m "你的提交信息"
git tag v3.x.x
git push origin main
git push origin v3.x.x
```

### 签名密钥

私钥存储在 `~/.tauri/cc-switch.key`（本机），对应的 GitHub Secrets：

| Secret 名称 | 说明 |
|-------------|------|
| `TAURI_SIGNING_PRIVATE_KEY` | 私钥文件完整内容 |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | 私钥密码 |

**注意：私钥丢失后无法对更新包签名，用户将无法自动更新。请妥善备份。**

---

## 同步上游更新

当上游 [farion1231/cc-switch](https://github.com/farion1231/cc-switch) 有新代码时，使用以下流程合并，同时保留本 fork 的改动。

### 步骤

```bash
# 1. 拉取上游最新代码
git fetch upstream

# 2. 将本 fork 的改动 rebase 到上游最新代码之上
git rebase upstream/main

# 3. 如果出现冲突，逐个解决后继续
git add <冲突文件>
git rebase --continue

# 4. 推送到本仓库
git push origin main --force
```

### 最容易发生冲突的文件

以下文件同时被上游和本 fork 修改过，rebase 时需要特别注意：

| 文件 | 注意事项 |
|------|----------|
| `src-tauri/src/commands/provider.rs` | 保留新增的 `fetch_provider_models_openai` 函数和 `FetchOpenAiModelsResponse` 导入 |
| `src-tauri/src/services/provider/mod.rs` | 保留 `mod models;` 和 `pub use models::...` 两行 |
| `src-tauri/src/lib.rs` | 保留 `commands::fetch_provider_models_openai` 注册行 |
| `src/lib/api/providers.ts` | 保留 `providersApi` 末尾的 `fetchOpenAiModels` 方法 |
| `src-tauri/tauri.conf.json` | 保留本仓库的 `pubkey` 和 `endpoints` 地址，不要被上游覆盖 |

### 冲突解决原则

- 上游新增的功能：**保留**
- 本 fork 新增的函数/注册/导出：**保留**
- `tauri.conf.json` 中的 `pubkey` 和 `endpoints`：**始终使用本仓库的值**

### 如果 rebase 出错想放弃

```bash
git rebase --abort
```

---

## 远程仓库配置

```
origin   -> https://github.com/hexsen929/cc-switch.git  （本 fork）
upstream -> https://github.com/farion1231/cc-switch.git （原项目）
```
