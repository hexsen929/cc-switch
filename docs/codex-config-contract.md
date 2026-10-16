# Codex 配置兼容验证

GitHub CI 的 **Official Codex configuration contract (0.162.1)** 固定使用官方 npm 包 `@openai/codex@0.162.1`。验证范围是该版本的实际配置加载契约，不代表任意旧版或桌面 alpha 版均受支持。

`tests/contracts/codexConfigFixtures.test.ts` 直接调用应用内的所有 Codex 供应商预设、自定义模板和 `generateThirdPartyConfig`，导出未经删键、替换或过滤的 TOML。新增预设自动进入验证；API Key 和托管 OAuth 两种生成方式都会覆盖。原生官方登录的空配置也保留在验证集内。

`scripts/validate-codex-config.py` 对每个配置创建独立临时 HOME 和 CODEX_HOME，仅写入虚构的 auth.json，然后运行官方 CLI：

```sh
codex --strict-config app-server --listen stdio://
```

脚本仅发送 `initialize`、`initialized` 和 `config/read`。它检查：

- 官方严格加载器没有拒绝未知配置键，也没有发出配置警告。
- `config/read(includeLayers=true)` 的 user layer 与导出的原始 TOML 一致。
- model、provider、reasoning 和供应商字段出现在真实有效配置中，model/provider 的来源确实是临时配置。
- 顶层、供应商及 features 下的三个故意未知键均被官方加载器拒绝，避免验证路径绕过严格检查时误报通过。

任务还通过同一固定 CLI 的 `app-server generate-json-schema` 保存协议 schema，供审阅 `config/read` 的协议类型。它不是从另一个最新版本下载的配置 schema，也不把简单的 TOML 解析或 `codex features list` 当作配置验收。

验证不需要真实账号或 API Key，不发送模型请求，不启动线程、回合或 MCP 工具。脚本不继承开发者的凭据环境变量；外部 HTTP 代理被固定到不可用的本机 fixture 地址。模型访问、认证交换、套餐权限和上游协议可用性仍需对应集成测试，不能由配置通过推断。

GitHub artifact `codex-config-contract-0.162.1` 保存导出配置、固定版本生成的协议 schema 和每个预设的结果。依赖安装、fixture 导出及完整验证均在 GitHub 云端完成，本地只保留源码。

排查明确指定的配置文件时，也可使用已安装的 CLI 做隔离加载，无需项目依赖：

```sh
python3 scripts/validate-codex-config.py \
  --codex /absolute/path/to/codex \
  --config-file /explicit/path/to/config.toml \
  --allow-version
```

`--allow-version` 只适用于单文件排查，不能用于 CI fixture 套件；所选 CLI 仍须支持 `--strict-config` 和 `config/read`。此模式只输出成功或经过净化的未知键路径，隐藏原始错误、配置值和用户路径。它不读取配置旁边的真实 auth.json，也不更改原文件。
