# Fork 维护说明

本项目 fork 自 [farion1231/cc-switch](https://github.com/farion1231/cc-switch)，在原项目基础上新增了以下功能。

---

## 新增功能

### OpenAI 兼容接口模型获取

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

### 更新地址

`src-tauri/tauri.conf.json` 中的更新端点和签名公钥已替换为本仓库的配置。

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
