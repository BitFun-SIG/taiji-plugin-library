# BitFun → OpenBitFun 旧版持久化数据迁移清单

> 状态：静态核对完成（实现前清单；尚未实现或运行验证）
> 品牌更名提交：`8784bbc4258131b4e74e6f9192a23b43c454fa94`
> 最后更新：2026-09-05

## 1. 目的与结论口径

本文梳理品牌名从 BitFun 改为 OpenBitFun 后发生路径或存储命名变化的持久化数据，为后续迁移系统提供输入。每一项均区分：

- **P0 必须迁移**：丢失会直接造成配置、会话、身份、凭据或用户创作内容不可用。
- **P1 应迁移**：属于有价值的历史数据、工作状态或可感知偏好；缺失会造成明显体验或历史损失。
- **P2 可选迁移**：可重建或价值较低，迁移主要用于减少升级扰动。
- **不迁移**：缓存、日志、锁、PID、临时事务目录、可安全重建的索引。
- **待核对**：尚未完成 owner、格式或生命周期核对，不能据此直接实现迁移。

最重要的架构结论是：**迁移系统不能只做旧目录到新目录的递归复制**。至少 `config/app.json`、`data/workspace_data.json`、cron、项目权限、Remote Connect 和 Subscription Auth 存在品牌字段、版本/schema 拒绝、旧格式升级或密钥域变化，必须由各 store 的迁移器读取、校验、转换并原子写入。

## 2. 证据边界

- 已做：当前 checkout 的静态代码检查、提交元数据/路径级检查、本机旧目录的名称、数量和大小检查。
- 未做：未读取用户配置、会话、Token、Cookie、密钥或终端记录的实际内容。
- 未做：未启动应用或开发服务器，未执行 UI/WebView 验证，未执行远程工作区、远控、Peer Device、Detached Dispatch 运行验证。
- 未做：未运行迁移，未删除或移动任何旧数据。

表格中的“已核对”仅表示已找到当前代码 owner、路径或生命周期证据，不代表迁移实现已验证。

## 3. 根目录总览

| 状态 | 数据域 | 旧位置 | 新位置 | 优先级 | 总体建议 |
|---|---|---|---|---|---|
| 已核对 | 产品用户目录 | Windows `%APPDATA%/bitfun`；macOS `~/Library/Application Support/bitfun`；Linux `$XDG_CONFIG_HOME/bitfun`（通常 `~/.config/bitfun`） | 各平台同一 config root 下的 `openbitfun` | 分项判断 | 不可整目录复制；按 `config`、`data`、`skills` 等 store 迁移，排除日志、缓存和临时文件 |
| 已核对 | 产品 Home | `~/.bitfun` | `~/.openbitfun` | 分项判断 | 会话、工作区、记忆、身份、远控、dispatch 等分别迁移；排除锁、trace 和索引 |
| 已核对 | Tauri 应用数据 | `%APPDATA%/com.bitfun.desktop` | `%APPDATA%/com.openbitfun.desktop` | P1 | 窗口状态等按文件迁移，不复制异常恢复标志 |
| 已核对 | WebView2 用户数据 | `%LOCALAPPDATA%/com.bitfun.desktop/EBWebView` | `%LOCALAPPDATA%/com.openbitfun.desktop/EBWebView` | P1 | 不全是缓存；保留 Local Storage、IndexedDB；若承诺登录态则成组迁移 Cookies/Profile，排除浏览器缓存与运行锁 |
| 已核对 | Windows SSH 数据根 | `%LOCALAPPDATA%/BitFun/ssh` | `%LOCALAPPDATA%/OpenBitFun/ssh` | P0 | 连接、远程工作区、host key 信任和密码 vault 成组迁移；旧 connection ID 和旧单对象 workspace 格式需转换 |
| 已核对 | Git worktree / dispatch 外部实体 | 由旧注册表记录 | 由新注册表记录 | P0/P1 | 不得仅改 JSON 中的路径或直接搬目录；优先保留旧实体原位并让新注册表继续引用，或使用 Git-aware 专用迁移 |
| 已核对 | Desktop 开发 profile | `%APPDATA%/com.bitfun.desktop.dev`、`%LOCALAPPDATA%/com.bitfun.desktop.dev/EBWebView` | 对应 `com.openbitfun.desktop.dev` | P2（开发者） | 与正式 profile 完全隔离；只为确实使用过 dev build 的开发者按第 7 节规则迁移，不应混入普通用户升级 |
| 已核对 | Windows 安装与系统集成 | `%LOCALAPPDATA%/BitFun`、BitFun 注册表项/快捷方式/PATH | OpenBitFun 对应项 | P0（升级流程），非用户数据 | 旧安装器当前不会被新名称的检测器发现；需显式接管旧安装记录、安装新包并在成功后移除旧集成，详见第 8 节 |
| 已核对 | Android/iOS/HarmonyOS 应用沙箱 | 旧 package/bundle ID 的私有沙箱与安全存储域 | 新 package/bundle ID 的独立沙箱与安全存储域 | P0 | 包标识变化会让系统把新版视为另一个应用；数据库、身份和凭据不能靠文件名兼容读取，详见第 9 节 |
| 已核对 | Hosted MiniApp/Skin Market | `/srv/bitfun-*-market`、`/etc/bitfun-*-market` | `/srv/openbitfun-*-market`、`/etc/openbitfun-*-market` | P0（服务迁移） | 数据库、artifact、backup、secret env 和服务单元成组迁移；持久 JSON 与 Skin archive 还需结构化转换，详见 5.10 |
| 已核对 | 普通浏览器 origin storage | Market、Mobile Web、Design Lab 各自 origin 下的旧品牌 Local Storage/IndexedDB | 同 origin 下的新 key/DB | 分项判断 | Desktop 迁移器不得扫描外部浏览器 profile；同 origin 页面在各 store hydrate 前双读旧 key 并写新 key，跨 origin 则需站点级 bridge/export-import，详见第 7 节 |

当前路径契约主要见：

- `src/crates/assembly/core/src/infrastructure/app_paths/path_manager.rs`
- Desktop 的 Tauri identifier 配置

`dirs::config_dir()` 决定产品用户目录，所以不能把 Windows 路径硬编码成跨平台迁移规则。`~/.bitfun`/`~/.openbitfun` 则由用户 home 决定；`BITFUN_USER_ROOT`、`BITFUN_HOME` 等旧覆盖变量还可能让真实源位置偏离上述默认值，必须在创建任何新 store 之前按 5.9 处理。

## 4. `%APPDATA%/bitfun` 深入清单

### 4.1 配置与备份

| 状态 | 相对路径 | 作用 | 优先级 | 迁移方式与注意事项 |
|---|---|---|---|---|
| 已核对 | `config/app.json` | 全局产品配置；包含模型、运行时、语音等设置，部分字段可能直接包含 API key | P0 | 使用配置 owner 做 schema-aware 转换；更新产品标识并迁移 retired/renamed 字段，不能原样复制 |
| 已核对 | `config/hooks.json` | 用户级 native hook 配置 | P0 | 迁移并按当前 Codex hook contract 校验；保持启用/禁用和执行策略，不能只迁 hook 脚本不迁配置 |
| 已核对 | `config/external-sources.json` | 外部工具、MCP、subagent 等来源的选择、审批、冲突决策和已确认生态 | P0 | 完整迁移并校验 schema；这是安全/信任决策，不是发现缓存。原生候选 ID 的 `bitfun.desktop:*` / `bitfun.cli:*` 需改成 `openbitfun.desktop:*` / `openbitfun.cli:*`，并由当前 owner 重新计算包含候选 ID 的 native conflict key/fingerprint；不能只做 JSON 字符串替换 |
| 已核对 | `config/announcement-state.json` | 公告已读/展示状态 | P2 | 可迁移以避免升级后重复提示；失败不阻塞主迁移 |
| 已核对 | `config/installer-state.json` | 最近安装路径等安装器状态 | P2 | 可选迁移；路径失效时保留记录并明确降级 |
| 已核对 | `config/backups/`、`data/backups/` | 配置或工作区索引的旧备份 | P1（档案） | 保留为 legacy archive；若导入新 active backup 命名空间，必须先转换为当前格式 |
| 已核对 | 根部 `config.toml` | CLI 配置 | P0 | 迁移正式文件；不迁移同名锁文件。`ui.theme_id` 的 6 个内置值需逐项从 `bitfun-{cyber,dark,ink-night,light,midnight,tokyo-night}` 映射为 `openbitfun-*`；当前读取器不认识旧 ID，原样保留会静默退回基础主题。用户自定义/未知 ID 不做前缀猜改 |
| 已核对 | 根部 `agents/` | 用户级自定义 Agent 定义 | P0 | 完整迁移，目标已有同名定义时保留双方并报告冲突 |
| 已核对 | 根部 `update.lock` | CLI self-update 最长 1 小时的 install 互斥锁 | 不迁移 | 只对旧 updater 进程有效；升级前停止旧 updater，由新 CLI 在新根重新建锁 |
| 已核对 | 根部 `last-update-check` | 文件 mtime 控制自动检查频率，内容记录执行检查时的 CLI 版本 | 不迁移/P2 | 不复制；让新 CLI 按自己的版本和启动时间重新建立检查节奏，避免旧 mtime 延迟或扰乱新版本检查 |
| 已核对 | 根部 `update-last-error` | 后台更新失败的一次性诊断；下一次交互启动读取、打印后删除 | 不迁移 | 旧错误针对旧 binary/updater，不应在 OpenBitFun 首启中重放；源文件留在旧根供显式诊断 |

`app.json` 当前加载会校验 OpenBitFun 产品标识、配置版本和 schema；`workspace_data.json` 也校验产品标识。因此这两个文件是“读取旧结构—转换—写新结构”，不是“复制后改一个目录名”。

### 4.2 核心持久化数据

| 状态 | 相对路径 | 作用 | 优先级 | 迁移方式与注意事项 |
|---|---|---|---|---|
| 已核对 | `data/workspace_data.json` | 工作区注册、选择和相关持久化信息 | P0 | 结构化转换，更新产品标识；对已移动/不存在路径保留记录并标记不可用，不静默删除 |
| 已核对 | `data/memories/memories.sqlite` | 结构化记忆数据库 | P0 | 在新 store 打开前用 SQLite backup API 迁移，或在旧进程完全退出后 checkpoint；不能只复制主 DB 而忽略 WAL |
| 已核对 | `data/agent-runtime/coordination.sqlite*` | Agent/subagent/background task 的 durable coordination 状态 | P0/P1 | 同样使用 SQLite 一致性迁移；`-shm` 不是用户数据，WAL 必须被正确合入 |
| 已核对 | `data/agent-runtime/ownership/` | 运行时所有权/互斥状态 | 不迁移 | 新进程重新建立，迁移会制造陈旧 owner |
| 已核对 | `data/agent-runtime/ipc-v<protocol>/` | Shared Runtime discovery JSON、instance lock；Unix 还含 UDS socket，Windows endpoint 是 named pipe | 不迁移/敏感运行态 | record 含 PID、owner、endpoint 和连接 token。品牌 hash domain 与 pipe/socket 名都已变化；停旧 Runtime 后由新进程重建。不得把旧 token 或 discovery record 复制进新根，也不得把它误归入 `coordination.sqlite` 的 durable 状态 |
| 已核对 | `data/permissions/tool-permissions.sqlite` | 按项目保存的工具授权和有界审计记录 | P0 | SQLite 一致性迁移；旧数组式项目权限需由专用迁移器转换，不能只复制旧配置 |
| 已核对 | `data/cron/jobs.json` | 用户定时任务 | P0 | 旧 v1 当前代码明确要求显式迁移；转换到当前 schema 后原子写入 |
| 已核对 | `data/cron/backups/` | 定时任务恢复备份 | P1（档案） | 保留旧备份；如进入当前备份集合需先做格式转换 |
| 已核对 | `data/terminals/` | 持久化终端 transcript，包括命令、输出、cwd、退出码 | P1 | 迁移 `index.json` 和正式日志分段；排除 `index.json.tmp`，按敏感数据处理 |
| 已核对 | `data/token_usage/` | 模型使用量、Token 和费用统计历史 | P1 | 迁移 `model_stats.json` 与 `records/*.json` |
| 已核对 | `data/usage-data/` | 用户生成的 Insights JSON/HTML 报告 | P1/P2 | 建议迁移；虽可能从原始数据重建，但属于用户显式生成的历史产物 |
| 已核对 | `data/ssh_acp_capabilities.json` | 远程 ACP 能力探测快照 | 不迁移/P2 | 可重新探测；不应让旧快照覆盖当前协商结果 |

### 4.3 用户创作、扩展和工作状态

| 状态 | 相对路径 | 作用 | 优先级 | 迁移方式与注意事项 |
|---|---|---|---|---|
| 已核对 | `data/frontend-workbench/state.json`、`state.previous.json` | 当前、上一版和 pending 前端修订状态 | P0/P1 | 必须迁移并保持引用闭包 |
| 已核对 | `data/frontend-workbench/drafts/` | 用户尚未 apply 的前端草稿 | P0 | 完整迁移 |
| 已核对 | `data/frontend-workbench/revisions/` | bundled 及用户/Agent 创建的前端修订 | P1 | 至少迁移 state 引用的 revision 和所有 `creative-*`；纯 bundled revision 可由安装包重新物化 |
| 已核对 | `data/frontend-workbench/.copy-*` | 复制/提交事务暂存 | 不迁移 | 陈旧事务目录 |
| 已核对 | `data/miniapps/<app>/` | MiniApp 源码、元数据、用户 storage、自定义、版本历史及编译产物 | P0/P1 | 正式 app 目录整体迁移最安全；`compiled.html` 可重建，但一并迁移可避免首启不可用 |
| 已核对 | `data/miniapps/.drafts/` | 进程内沙箱草稿 | 不迁移 | 当前启动会隔离并清理跨进程遗留 draft |
| 已核对 | `data/miniapps/.market-install-*` 等 | 市场安装、更新、回滚临时事务 | 不迁移 | 不能把未完成事务当成正式 MiniApp |
| 已核对 | 用户级/项目级 `hook-imports` | 外部 hook 导入索引、用户 enablement、经验证的托管 snapshot | P0/P1 | index 与 bundle 成组迁移，并重新校验 schema、路径及信任语义 |
| 已核对 | `data/rules/` | 用户级规则 | P0 | 完整迁移；用户内容不能被新版本默认内容覆盖 |
| 已核对 | `data/plugins/` | 用户安装的产品插件包 | P0 | 包、manifest、信任/启用相关 store 成组迁移；旧 `bitfun.plugin.json` 必须由 owner 校验后写成 `openbitfun.plugin.json`，详见 4.3.1 |
| 已核对 | `skills/` | 用户安装或产品附带的 skills | 分项判断 | 用户/第三方 skill 必须迁移；当前版本可重新生成且未被用户修改的 bundled system skill 可重建。配置中引用 skill 的稳定 key 也必须转换，详见 4.3.1 |

#### 4.3.1 插件与 skill 的持久 identity

插件和 skill 除了目录改址，还存在被配置、信任记录和历史 Turn 引用的品牌 identity：

| 状态 | 旧值 → 新值 | 持久位置/作用 | 迁移方式与注意事项 |
|---|---|---|---|
| 已核对 | 每个 package 的 `bitfun.plugin.json` → `openbitfun.plugin.json` | `data/plugins/<packageId>/` 与工作区 `.bitfun/plugins/<packageId>/` 的托管插件 manifest | P0。当前 discovery 只查新文件名，直接复制目录会得到 `missing_manifest`。用当前 `PluginPackageManifest` parser 校验旧文件，确认 package 目录名、声明文件和 SHA-256 后，在目标 package 中以新文件名原子写入；不要盲目保留两份 active manifest |
| 已核对 | 旧 package canonical path → 新 package canonical path | `data/runtime/plugin-trust.json` 的 trust/activation record 保存 `packageId`、version、adapter、**绝对 canonical `sourcePath`** 和 content hash | P0（已批准/激活插件）。搬目录会改变 identity，原样复制 trust store 会使批准和 activation 失配。迁移器必须重新发现目标 package，仅在旧记录的 package/version/adapter/content hash 与目标验证结果一致时，把 `sourcePath` 重绑定到目标；目标已有 trust decision 时保留目标，不能借迁移扩大信任 |
| 已核对 | `bitfun://managed-plugins/...`、`bitfun://plugins/...` → `openbitfun://...` | adapter 生成的 source URI、tool target URI，并参与部分 runtime `plugin_id` hash；可能出现在 Session/dispatch transcript 的历史 tool/plugin diagnostics | 不能对 Session 文本全局替换。历史记录保留原 identity 供展示/审计；恢复仍需解析旧记录时由 plugin compatibility layer 接受旧 URI，并映射到已验证的新 package identity。新记录只写新 scheme |
| 已核对 | `bitfun.plugin.package.v1` / `*:bitfun.plugin` → `openbitfun.plugin.package.v1` / `*:openbitfun.plugin` | OpenCode adapter 生成的 `PluginManifestRef`，不是 `bitfun.plugin.json` 内部的数值 `schemaVersion: 1` | 历史诊断/Session record 双读旧值；不要把这段字符串误写进 package manifest，也不要改变 package manifest 的数值 schemaVersion |
| 已核对 | `user::bitfun::<name>`、`project::bitfun::<name>`、`user::bitfun-system::<name>` → 对应 `openbitfun` key | `config/app.json` 中全局禁用与 Agent profile 的 enabled/disabled skill 列表，以及工作区 `.bitfun/config/agent_profiles.json`/`mode_skills.json` 的 project override | P0（用户选择）。按完整 key 的 slot 段转换，不能替换 skill 名或第三方 slot。当前 `normalize_skill_keys` 只 trim/dedupe，不兼容旧 key；不转换会让用户的启用/禁用选择静默失效 |
| 已核对 | `source_slot=bitfun\|bitfun-system`、`source_id=bitfun` → OpenBitFun 名称 | `SkillInfo`/Skill tool result 与历史 Session 中的来源说明；实际文件归属由目录和 discovery root 决定 | 历史 Turn 保持可读，不必为展示重写整份 Session。当前运行时重新 discovery 后写新 identity；任何以旧 `source_id/source_slot` 判断“产品 owned、可删除”的操作在兼容窗口内必须先 canonicalize，且仍禁止删除 built-in |

插件目录还会出现在项目级 `.bitfun/plugins/`，因此同一 package 转换逻辑必须同时用于用户级与本地/远程 workspace。远程 workspace 的 manifest 与 trust/path identity 应在远端 filesystem owner 上处理，不能把远端 POSIX 路径改写成控制端路径。

### 4.4 浏览器自动化与可重建数据

| 状态 | 相对路径 | 作用 | 优先级 | 迁移方式与注意事项 |
|---|---|---|---|---|
| 已核对 | `data/browser-control/<browser>/` | 专用 Chromium user-data-dir，可能包含 Cookie、登录态、Local State、Preferences | P1/可选 | 若承诺保留自动化浏览器登录态，应迁移；`Local State`、Profile、Cookies 必须成组，并要求浏览器及旧版应用已退出 |
| 已核对 | Chromium `Cache`、`Code Cache`、`GPUCache`、shader cache、Crashpad、`Singleton*`、`DevToolsActivePort` | 缓存、崩溃或运行锁 | 不迁移 | 新版本重建 |
| 已核对 | `runtimes/` | OpenBitFun 管理的 Node/Python/Office 等运行时 | 不迁移/P2 | 属于可重新供应的产品依赖；若为节省下载而复用，必须验证版本、完整性和内部绝对路径，不能盲拷 |
| 已核对 | `data/models/speech/` | 本地语音模型资源 | P2 | 可重新下载；可做校验后硬链接/复制的优化迁移，但不能让模型资源阻塞 P0 数据迁移 |
| 已核对 | `cache/model-downloads/speech/` | 未完成或可重试的语音模型下载 | 不迁移 | 让新版本重新下载 |
| 已核对 | `data/sdk-host/temp/scripts/` | SDK Host 临时脚本 | 不迁移 | 名称和生命周期均表明是临时执行产物 |
| 已核对 | 根部 `temp/scripts/` | Terminal shell integration 生成脚本 | 不迁移 | 新版本按当前 shell/runtime 重新生成；旧脚本可能嵌入旧可执行文件或绝对路径，不能复用 |
| 已核对 | `cache/`、`temp/` | 可重建缓存与临时文件 | 不迁移 | 不复制 |
| 已核对 | `logs/`、`cli-logs/` | 应用与 CLI 日志 | 不迁移 | 用户已明确无需迁移；如需诊断可把旧目录留作只读历史，不纳入新活动日志目录 |

### 4.5 凭据和安全敏感数据

| 状态 | 相对路径/外部存储 | 作用 | 优先级 | 迁移方式与注意事项 |
|---|---|---|---|---|
| 已核对 | `data/subscription_auth.json` | 订阅账户的 v2 metadata：provider、过期时间、account metadata、secret set/chunk 计数，以及注销/并发保护用的 `provider_revisions` tombstone | P0 | 结构化迁移并保留 provider revision；相同 provider 两侧都有记录时目标 credential 优先，但 revision 取较大值。不能只复制 metadata，因为 secret 在外部 vault；也不能漏掉已注销 provider 的 revision，否则旧 refresh 可能复活凭据 |
| 已核对 | Windows/Linux secret service `openbitfun.bitfun.subscription-auth.v1` | 更名前的订阅 secret 域；v2 entry 名为 `<provider>/v2/<setId>/<field>/<index>`，更旧格式为 provider 单 entry | P0 | 从旧 service 逐 chunk 读取、重组并校验，再以新随机 set ID 写入 `openbitfun.subscription-auth.v1`；全部新 secret 写入成功后才提交新 metadata。旧单 entry 是组合 JSON，需复用提交前已有的 legacy importer；不得在日志中输出 entry 内容 |
| 已核对 | macOS `data/.subscription_auth_vault.key` + `data/subscription_auth_vault.json` | prompt-free 文件凭据 vault | P0 | 用旧 key 解密旧 vault，再通过新 owner 按 provider 写入目标 vault并提交 metadata；不能在目标已有数据时复制两文件覆盖，也不能只迁其中一个 |
| 已核对 | `data/subscription_auth.cleanup.json` | 未完成的 vault secret 清理 journal | P0（安全状态） | 记录的是旧 vault service 中待清理 entry，不能原样复制后让新 service 执行。源保持只读；迁移 active credential 后把旧域的待清理项列为显式后续清理，且不能删除仍被旧 metadata 引用的 entry |
| 已核对 | `data/subscription_auth.bak` | Windows metadata replace 的回滚文件，可能仍含旧明文 credential | 条件恢复/高敏感 | 先按旧 owner 的事务规则判断 canonical 缺失时是否恢复，再开始跨目录迁移；canonical 已存在时不作为第二份账户合并。遗留 backup 必须按 secret 文件保护，不能普通归档或输出内容 |
| 已核对 | `data/subscription_auth.lock`、`subscription_auth-<provider>-refresh.lock`、`*.tmp` | metadata transaction/refresh lease 和写入暂存 | 不迁移 | 新目录重新创建锁；旧锁 inode/PID 生命周期无效。临时文件只保留在旧根供诊断，不导入 active store |
| 已核对 | WebSearch `credentials.key` + `credentials.json` | Web 搜索凭据 | P0 | key 与密文成组迁移，校验后原子提交 |
| 已核对 | `.mcp_oauth_vault.key` + `mcp_oauth_vault.json` | MCP OAuth 凭据 | P0 | key 与 vault 成组迁移 |
| 已核对 | `review-platform-tokens.json` | GitLab/GitCode token 明文 JSON store | P0 | 按高敏感明文数据迁移，限制文件权限并原子写入；GitHub token 由 `gh` 管理，不在此文件 |
| 已核对 | `config/app.json` 内 API key | 模型/语音等服务凭据 | P0 | 随结构化配置迁移，迁移日志不得输出值 |
| 已核对 | Windows/Linux keyring service `openbitfun.bitfun.miniapp-market.v1`，entry `github-oauth` | 更名前 MiniApp Market GitHub OAuth 的 access/refresh token 及各自过期时间 | P0/高敏感 | 读取完整旧 payload、反序列化并校验后写入新 service `openbitfun.miniapp-market.v1` 的同名 entry。目标已有完整凭据时保留目标，不逐字段拼接；任何日志不得输出 token |
| 已核对 | macOS `data/.market_credentials_vault.key` + `data/market_credentials_vault.json` | 同一 MiniApp Market OAuth payload 的文件凭据 vault | P0/高敏感 | 旧文件实际位于旧产品 config root 的 `data/`。key 与密文 vault 必须成组读取，再通过新 owner 写入目标；不能只复制其中一个，也不能覆盖已有目标 vault |

### 4.6 未识别文件、历史目录与手工副本

迁移器必须对旧根做一次“已知项减法”盘点：不能因为某个路径没有当前代码 owner 就静默删除，也不能把未知文件直接放进新活动配置目录。本机旧目录的只读名称检查发现了下表中的实例；没有读取文件内容。

| 状态 | 相对路径/模式 | 观察与作用判断 | 优先级 | 迁移方式与注意事项 |
|---|---|---|---|---|
| 已核对目录元数据 | 根部未识别的普通文件（本机存在一个 HTML 和一个 PDF） | 很可能是用户或工具误放到产品根的文件；不是当前 active store | P0（保全） | 保留在旧根，或复制到新根之外的 `legacy-unclassified` 只读档案；在迁移报告中逐项列出相对路径。不得丢弃，也不得自动打开或解释内容 |
| 已核对目录元数据 | `config/app copy.json`、`config/app user copy.json` 等手工副本 | 非当前配置 owner 使用的正式文件，可能含用户调试副本和 API key | P0/P1（档案） | 不导入为 `config/app.json`，不参与“新旧配置谁覆盖谁”的自动裁决；作为敏感 legacy archive 保留并在报告中提示用户手工处理 |
| 已核对目录元数据 | `data/backups/*_workspace_data.json` | 旧 workspace registry 的时间戳备份；当前 active owner 只使用 `data/workspace_data.json` | P1（档案） | 保留原始旧格式。不能直接放入当前恢复集合；只有用户选择恢复某一份时，才通过与 active registry 相同的 schema-aware 转换器导入 |
| 已核对目录元数据 | 根部 `backups/` | 本机实例为空，当前代码未找到该根级目录的活动 owner | 不迁移；非空则档案 | 空目录不创建；若其他安装中非空，按未知文件保留并报告，不与 `config/backups/` 或 `data/backups/` 混合 |
| 已核对目录元数据 | `agents/templates/` | 本机实例为空，当前 checkout 未找到活动 owner | 不迁移；非空则 P0/P1 档案 | 空目录不创建；非空内容先作为用户 Agent 模板候选保全，待格式识别后再由 Agent store 显式导入 |
| 已核对目录元数据 | `data/history/`、`data/snippets/`、`data/templates/`、根部 `workspaces/` | 本机实例均为空，当前 checkout 未找到活动 owner | 不迁移；非空则档案 | 空目录不创建；其他用户机器上若非空，不能据本机样本推断其可删除，应列入 `legacy-unclassified` |
| 已核对目录元数据 | `data/lsp-plugins/` | 本机实例为空；LSP 系统已在 `69bf1c270` 退休 | 不迁移到 active store；非空则档案 | 不恢复为当前产品能力。非空的插件包或用户配置保留在 legacy archive，避免升级过程替用户删除内容 |
| 部分核对 | `data/telemetry/uid` | 本机存在 36-byte 文件；当前 checkout 和更名前父提交均未找到该路径的活动 owner，不能确认是否仍有服务端身份语义 | P2/档案，不导入 active store | 默认原地保留或归档，不把旧 UID 注入当前 telemetry/设备身份。若后续找到历史 owner 或合规保留要求，再增加专用转换；它不能替代 `device_identity.json` |

实现时应维护显式的“已消费路径集合”。迁移结束后，对旧根剩余项只做名称、类型、大小和错误类别级报告；不要通过扩展名猜测并导入，也不要读取未知文件来生成日志摘要。

## 5. `~/.bitfun` 深入清单

### 5.1 顶层数据

| 状态 | 旧相对路径 | 作用 | 优先级 | 迁移方式与注意事项 |
|---|---|---|---|---|
| 已核对 | `projects/` | 本地工作区的运行时数据 | P0/P1 | 按工作区逐项迁移，详见 5.2 |
| 已核对 | `remote_ssh/` | 远程 SSH 工作区在本机的运行时镜像 | P0/P1 | 保持 `<host>/<POSIX path mirror>` 相对结构，不能用 Windows path 语义重新解释 |
| 已核对 | `personal_assistant/` | 助理工作空间及多 workspace 变体 | P0 | 迁移 `workspace`、`workspace-<id>`；同时发现更老的顶层 `workspace` / `workspace-*` |
| 已核对 | `memories/` | 记忆生成、归并和工作文件 | P0 | 整体迁移；它与 `%APPDATA%/bitfun/data/memories/memories.sqlite` 是不同数据域，均需保留 |
| 已核对 | `runtime-events/` | 每 Session 一份 JSONL 的在途流事件，用于崩溃/重启后恢复尚未完整写入 Session 的 Turn | P0 | 迁移能与已迁移 Session 关联的文件；首版不要主动删除无法关联或疑似陈旧文件 |
| 已核对 | `device_identity.json` | Remote Connect/设备身份 | P0 | 必须保留有效的 32 位十六进制 `device_id`；该 ID 可能已被账户登录流程改写为服务端绑定 ID，不能依赖主机名/MAC 重新生成 |
| 已核对 | `dispatch/` | Detached Dispatch 的 target job、controller outbound、Git 交付和崩溃恢复状态 | 分项判断 | 详见 5.4；活动任务和未完成同步必须保留，锁/PID 不迁移 |
| 已核对 | `worktrees/` | 产品管理的 Git worktree 实体根 | P0/P1 | 详见 5.5；默认不搬实体，迁移项目运行时内的注册表并保留旧绝对路径 |
| 已核对 | `relay-deploy/`、`relay-src/`、`docker-config*` | Relay 部署选择、源码/自定义和 Docker client config | 分项判断 | 详见 5.6 |
| 已核对 | `account_session.enc` + `account_session.key` | Remote Connect 登录 token、user ID、账户 master key、relay URL 和可选 device ID | P0 | 直接复制后新代码无法解密。旧 v2 key 是 `SHA256(legacy-machine-key + "\|BitFun::session_store::v2\|" + local-secret)`，更老 v1 是 machine-bound `BitFun::session_store::v1`；迁移器依次尝试旧格式，校验 payload 后以当前 `openbitfun::session_store::v1` 域重新加密。目标已有不同账户/relay 时不覆盖 |
| 已核对 | `account_hint.json` | 登录表单预填的 username + relay URL，不含 password/master key | P1 | 结构化复制；目标非空时保留目标。它不是登录凭据，不能在 account session 解密失败时制造“已登录”状态 |
| 已核对 | `account_sync/<user>.json` | Session 云同步 pull cursor 和每 Session 已上传内容 hash | P1 | 仅在已确认是同一 user + relay 账户时合并：`last_session_since` 取较大值，`uploaded_hashes` 做并集且目标同 ID 值优先。损坏文件留存并报告，不能按当前 loader 的 default 行为静默归零 |
| 已核对 | `account_sync/<user>.settings.json` | 设置同步的 `(version, hash)` 游标 | P1 | 只在同一 user + relay 下合并，并把 version/hash 当成不可拆分的一对；选择较高 version 对应的整对值。`*.tmp` 不迁移 |
| 已核对 | `remote_connect_persistence.json` | Feishu/Telegram/Weixin bot 连接、明文 bot secret、聊天工作区/助理/Session 选择、配对状态、表单值和 verbose 偏好 | P0 | 按高敏感 owner-only JSON store 迁移；旧 bare-string `current_workspace` 通过当前兼容反序列化升级。按 `bot_type` 合并且目标 connection 优先，旧值只填目标空缺；解析失败不得像当前普通 loader 那样降为默认空数据 |
| 已核对 | `bot_connections.json` | `remote_connect_persistence.json` 之前的同结构 bot store | P0（条件） | 仅当旧 canonical 文件不存在且不存在 `remote_connect_persistence.json.bak` 时作为来源；这是更名前 loader 的既有 fail-closed 顺序。成功导入后写当前 canonical store，旧文件保持只读 |
| 已核对 | `remote_connect_persistence.json.bak`、写入临时文件 | Windows replace 事务的回滚状态 | 不自动迁移为 active data | 若 canonical 缺失而 `.bak` 存在，表示可能停在清除/替换中；禁止回退读取 `bot_connections.json` 或擅自恢复旧账户上下文。保留并报告为需人工/owner 恢复的事务异常 |
| 已核对 | `weixin/<botAccountId>_get_updates_buf.txt` | Weixin long-poll 增量游标 | P1（活动 bot） | 对 `remote_connect_persistence.json` 引用的 bot account ID 迁移，避免重复消费或错过恢复边界；目标已有值时由 Weixin owner 比较/验证，不按字符串大小猜测新旧 |
| 已核对 | `weixin/<botAccountId>_context_tokens.json` | peer ID → Weixin context token；没有 token 时在下一条入站消息前不能正常回复/发送 typing | P0/P1（活动 bot） | 作为敏感 token map 迁移，按 peer ID 合并且目标值优先，文件权限收紧；不得记录 peer/token。无对应 bot account 的孤立文件保留为 legacy archive |

`BotChatState` 的 pending action、过期时间、delegated account token/master key、活动远端设备对象等字段带 `serde(skip)`，本来就只存在于进程内，不属于迁移对象。持久化的 `account_remote_context` 标志则必须保留，因为它防止重启后把远端路径/Session 错当成本机上下文。

### 5.2 工作区运行时目录：`projects/` 与 `remote_ssh/`

每个工作区不能只迁 `turns/`。一个完整 Session 可能包含 `metadata.json`、`state.json`、`turns/`、`turn-catalog.json`、`prompt_cache.json`、`token-anchors.json`、`snapshots/`、`artifacts/`、`tool-results/` 和 `evidence-ledger.json`。

| 状态 | 工作区内相对项 | 作用 | 优先级 | 迁移方式与注意事项 |
|---|---|---|---|---|
| 已核对 | `sessions/` | 会话 metadata、turn、状态、缓存重放信息、产物和证据 | P0 | 以完整会话为迁移单元，保留内部 ID 和相对结构；排除会话内 `request-traces/` 与运行锁 |
| 已核对 | `plans/` | 会话/工作流计划 | P1 | 随工作区迁移 |
| 已核对 | `snapshots/` | 工作区或会话快照 | P1 | 随工作区迁移；保持 Session/Turn 引用关系 |
| 已核对 | `config/isolation_status.json` | 工作区隔离状态 | P1 | 谨慎迁移；目标路径或 worktree 失效时显式降级，不能制造“已隔离”的假状态 |
| 已核对 | 项目级持久化 store（如 `hook-imports`） | 项目范围的集成配置和信任状态 | P0/P1 | 按 owning store 迁移和校验 |
| 已核对 | `request-traces/` | 请求诊断 trace | 不迁移 | 诊断数据，不属于会话事实 |
| 已核对 | `.session-write-locks/`、`locks/`、`*.lock` | 进程互斥和写锁 | 不迁移 | 新进程重新建立 |
| 已核对 | `product-search-v1.sqlite`、`search/` | 可重建搜索 sidecar/index | 不迁移 | 让新版本重建，避免旧 schema 或绝对路径污染 |
| 已核对 | 空的旧布局 `checkpoints/`、`diffs/`、`local/` | 历史布局遗留 | 不迁移 | 若实际检测到非空且代码 owner 不明，应保留在 legacy archive 并报告，不直接丢弃 |
| 部分核对 | 旧 `memory/` | 旧工作区级记忆布局 | 待定 | 不主动删除；先做版本识别，再导入当前记忆 owner 或留作 legacy archive |

### 5.3 任意工作区内的 `.bitfun/`

不能扫描整块磁盘来寻找工作区。应从 `workspace_data.json`、助理工作空间、远程工作区注册记录和 dispatch/worktree 注册表枚举已知工作区，再检查其根部的旧产品目录。

| 状态 | `.bitfun/` 内相对项 | 作用 | 优先级 | 迁移方式与注意事项 |
|---|---|---|---|---|
| 已核对 | `config/agent_profiles.json`、`tool_permissions.json`、`mode_skills.json`、`agent_subagents.json`、`hooks.json` | 项目级 Agent、权限、skill、subagent、hook 配置 | P0 | 迁移到工作区 `.openbitfun/config`，必要时执行各配置 schema 转换 |
| 已核对 | `agents/`、`rules/`、`plugins/`、可能的 `skills/` | 用户编写或安装的项目扩展内容 | P0 | 保持文件和目录结构；不得被新版本默认内容覆盖 |
| 已核对 | `search/flashgrep-index` | 工作区搜索索引 | 不迁移 | 可重建 |
| 已核对 | `computer_use_debug/` | Computer Use screenshot/OCR 调试图像；默认助理工作空间下实际形成嵌套的 `workspace/.bitfun/computer_use_debug/` | 不迁移/敏感诊断 | 新版本不需要这些 JPEG 恢复任何运行状态；文件名还可能带 OCR 查询片段。源目录保持只读或由用户另行归档，迁移报告只记录目录级数量/大小，不输出文件名和图像内容 |
| 已核对 | `bin/`、`tmp/` | 下载/运行时工具、临时文件 | 不迁移 | 新版本重建或按需重新下载 |

### 5.4 Detached Dispatch

同一个 `~/.bitfun/dispatch` 根下有两种角色的数据：

- target 侧由 CLI `DispatchStore` 持有真正的 job、事件流、permission/message/turn mailbox 和目标 Git checkout；
- controller 侧 `outbound/` 只是提交到其他进程后的 durable observer 索引、恢复 journal 和传输产物。

#### 5.4.1 Target 侧

| 状态 | 相对路径 | 作用 | 优先级 | 迁移方式与注意事项 |
|---|---|---|---|---|
| 已核对 | `jobs/<jobId>/job.json` | 提交请求、protocol version、intent hash、创建时间、标题；也是初始化完成的 commit marker | P0 | 与该 job 的 state/events/mailbox 成组导入；检查旧 `productId`/`dataNamespace` 和协议版本，不能仅复制 |
| 已核对 | `jobs/<jobId>/state` | queued/running/terminal、turn、取消和错误状态 | P0 | 与 job 原子迁移；旧 `running` 不能凭 PID 直接恢复，应由 owner 的 crash recovery 判定 |
| 已核对 | `events.ndjson` + `events.meta.json` | 有界追加事件流、cursor base、截断/省略事实 | P0 | 成组迁移，保持 byte cursor 语义；不能重排或只复制事件文件 |
| 已核对 | `permissions/{pending,answers,resolved}/` | 远程可回答权限 mailbox | P0 | 完整迁移；pending 权限是远程解除阻塞所必需的数据 |
| 已核对 | `messages/{pending,consumed}/`、`turns/{pending,consumed}/` | 在途 steering 消息和后续 Turn 的幂等 mailbox | P0 | 完整迁移并保留 message/turn ID |
| 已核对 | `job.pid`、`preparing`、`.lock`、`.events.lock`、`.spawn.lock`、`.worker.lock` | 旧进程 PID、spawn/worker/写入互斥和中间标志 | 不迁移 | 升级后进程身份和锁均失效；让 current owner 重建/恢复 |
| 已核对 | `.retention-gc`、`.retention-gc.lock`、`workspaces/.*.lock`、`repos/.*.lock` | GC 时间戳和操作锁 | 不迁移 | 新进程重新调度 GC 并建立锁 |
| 已核对 | `workspaces/<jobId>/provision.json` | job 与 repoKey、目标 checkout、bundle 状态之间的 durable binding | P0（活动/待同步 job） | 必须和 job、repo、worktree 一致处理；其中绝对路径不能在复制后单独替换 |
| 部分核对 | `workspaces/<jobId>/` 其他 bundle/upload/sync 文件 | 传输断点和同步中的产物 | P0/P1（仅未完成操作） | 仅对活动 preparation/upload/sync 保留；已完成 job 的纯 staging 可由 owner 清理 |
| 已核对 | `repos/<repoKey>/` | target 共享 bare repository；虽称 cache，但可能仍承载活动 job 或尚未拉回 controller 的提交 | P0/P1 | 活动/近期 job 引用时不能丢；不要脱离 `worktrees` 单独搬移 |
| 已核对 | `worktrees/<repoKey>/<project>-<job>` | target 每 job checkout，可能含尚未同步回 controller 的用户工作结果 | P0/P1 | 不应视作缓存。优先保留旧根原位并兼容引用；若必须迁移，使用 Git-aware 迁移并修复 bare repo/worktree 双向 metadata |

target terminal job、worktree 和事件目前按 30 天保留，共享 bare repo 也有独立的 30 天 last-used 保留策略。品牌迁移不能把“会被 retention 清理”误解成“升级时可直接丢弃”：近期 terminal job 仍需要被观察和同步结果。

#### 5.4.2 Controller `outbound/`

| 状态 | 相对路径 | 作用 | 优先级 | 迁移方式与注意事项 |
|---|---|---|---|---|
| 已核对 | `outbound/<jobId>.json` | durable observer 索引；保存 target/session、cursor、状态、源工作区，以及 baseline worktree ID/path/claim | P0 | 结构化迁移；绝对路径和 worktree claim 必须与 5.5 的决定一致，不能整目录覆盖 |
| 已核对 | `outbound/.preparations/<jobId>.json` | 提交确认前的 owner-only crash journal 和 setup audit | P0（未完成提交） | 保留未过期/仍有关联的记录；先恢复或核对 baseline claim，再进入正常 reconciliation |
| 已核对 | `outbound/.preparations/<jobId>.run` 等锁 | preparation 运行锁 | 不迁移 | 新进程重新建立 |
| 已核对 | `outbound/.transcripts/<jobId>.json` | renderer observer transcript 与 cursor/completeness 配对缓存 | P1 | 建议迁移。它可从 target 重放，但 target 事件可能已截断；丢失后不一定能恢复完整可见历史 |
| 已核对 | `outbound/.bundles/` | controller 上传给 target 的 base Git bundle staging | P1（未完成上传） | 只为未完成 preparation/upload 保留；能从仍存在的 baseline 安全重建时可丢弃 |
| 已核对 | `outbound/.results/` | 从 target 拉回、尚未成功 fetch/fast-forward 的 result bundle 及摘要 | P0（未完成同步） | 在 baseline 已确认接收前必须保留；成功接收后属于可删除传输产物 |

#### 5.4.3 Dispatch 分支名

品牌提交把默认 `app.worktrees.branchPrefix` 从 `bitfun/` 改为 `openbitfun/`，因此新建 dispatch baseline 的默认分支也从 `bitfun/dispatch/<jobId>` 变成 `openbitfun/dispatch/<jobId>`。这里的分支名不是展示文案：它同时存在于实际 Git ref、managed worktree 注册表、controller `outbound/<jobId>.json`、preparation journal，以及 target 的 provision/job 状态中。

迁移时应区分“未来默认值”和“既有实体”：

1. 只有 `config/app.json` 中的 `app.worktrees.branchPrefix` **精确等于旧默认值** `bitfun/` 时，才迁为 `openbitfun/`；用户自定义前缀必须原样保留。
2. 已存在或仍活动的 `bitfun/dispatch/*` 分支及其所有引用默认保持原名。新代码应按记录中的真实分支操作，而不能根据当前默认值重新推导。
3. 不能只改 JSON 中的 `branch`。那会让记录与实际 ref 分离，并可能使 baseline guard、result fetch/fast-forward 或 retention claim 失效。
4. 如果产品明确要求重命名既有分支，必须作为 Git-aware 原子事务：确认 checkout/ref/dirty 状态，重命名真实分支并同步 worktree registry、outbound、preparation、target provision/job 等全部引用；任一步失败都保留旧分支与可恢复 journal。

### 5.5 普通 managed worktree

普通 worktree 的实体默认位于 `~/.bitfun/worktrees/<repository-id>/<project>-<short-id>`，但注册表 `worktrees.json` 位于对应项目的运行时 `config` 目录中。注册表保存 `projectWorkspacePath`、每个 worktree 的绝对 `path`、branch/base/lifecycle、幂等 receipt，以及 `dispatch:<jobId>` retention claim。

Git 的 common dir `worktrees/*` admin metadata 和 linked checkout 内的 `.git` 文件也包含路径关系。因此建议：

1. **迁移项目 runtime 中的 `worktrees.json`，但默认不搬 `~/.bitfun/worktrees` 实体。**
2. 新注册表继续引用旧实体的绝对路径；新建 worktree 使用新的 `~/.openbitfun/worktrees` 根。
3. 保留并验证 dispatch claim，避免升级后自动清掉仍作为 dispatch baseline 的 worktree。
4. 若产品决定实体也必须改路径，应走单独 Git-aware 事务：确认 clean/dirty/branch/claim 状态，使用 Git worktree 操作或 repair 更新两侧 metadata，再更新工作区索引、注册表和所有 outbound record。普通文件复制后字符串替换不可接受。
5. 用户在 `app.worktrees.root_path` 中显式配置的自定义绝对根不应改写；只有等价于旧产品默认值的配置才可按迁移策略处理。

### 5.6 Relay 自部署与远端 Docker 数据

这一组数据位于执行 Relay 部署的 SSH 目标机，不会因为本机 Desktop 目录迁移而自动迁走。所有运行过旧 CLI/Relay 自部署的主机都要独立发现和处理。

| 状态 | 旧位置/对象 | 作用 | 优先级 | 迁移方式与注意事项 |
|---|---|---|---|---|
| 已核对 | `~/.bitfun/relay-deploy/relay.port` | Relay 监听端口选择 | P1 | 迁移到新目录，避免升级后回到默认端口导致健康检查或端口冲突 |
| 已核对 | `~/.bitfun/relay-deploy/relay.mirror-mode` | 镜像路线选择 | P2 | 可迁移用户选择；不存在则重新 auto 探测 |
| 已核对 | `relay-deploy/*.sh`、`*.pid`、`*.driver.pid`、`*.preparing`、`*.log` | 生成脚本、运行 PID/准备标志和部署日志 | 不迁移 | 升级不得接管旧 PID；检测到仍在运行的部署任务时应停止迁移并要求其结束/取消，而不是复制状态 |
| 已核对 | `relay-deploy/import-*.json` | 一次性账户导入文件，正常路径会立即删除 | 不迁移 | 若遗留，按高敏感失败产物告警并留在旧目录，不复制到新活动目录 |
| 已核对 | `~/.bitfun/docker-config`、`docker-config-<uid>` | Relay 自部署专用 Docker client config，可能含 registry auth 和用户 Docker 设置 | P0/P1 | 迁移 `config.json` 及未知用户文件，保留 0700/所有权；不要输出 auth。多个 uid 目录分别处理 |
| 部分核对 | `~/.bitfun/relay-src` | source-mode Relay 部署 checkout、`.env` 及可能的用户修改 | P1 | 不能整体当缓存。`.env`、Caddy/部署自定义和 Git dirty 文件必须保留；纯可验证 clean checkout 可重新 clone |
| 已核对 | Docker container `bitfun-relay` → `openbitfun-relay` | 正在运行的旧 Relay 服务 | P0（运行迁移） | 新部署前受控停止/暂存旧容器，验证新容器健康后再移除；否则旧容器会占用端口，新部署也看不到它 |
| 已核对 | Docker volume `relay-server_relay-db` | Relay 账户、设备、会话/路由等数据库所在 volume | P0 | volume 名未变，但容器内 DB 文件从 `/app/data/bitfun_relay.db` 改为 `openbitfun_relay.db`；必须在旧容器停止/SQLite 一致性确认后迁移 DB，不能让新容器创建空库 |
| 已核对 | Docker volume `relay-server_room-web` | Remote room Web 资产和 OpenBitFun Pages 数据 | P0/P1 | volume 名和 mount path 未变，应原地保留；不得作为镜像缓存删除 |
| 已核对 | 旧 Relay image 与 brand label | 可重新拉取的容器镜像/标记 | 不迁移 | 新 Relay 验证健康后再由维护流程清理旧镜像 |

若用户通过 Compose source mode 部署，volume 的实际前缀由 Compose project 决定，不能假定总是 `relay-server_*`；迁移器应从旧容器 inspect 的 mount 列表发现真实 volume。

### 5.7 CLI daemon 与远端安装状态

| 状态 | 旧位置/对象 | 作用 | 优先级 | 迁移方式与注意事项 |
|---|---|---|---|---|
| 已核对 | `~/.bitfun/cli_daemon.pid` | CLI account-device daemon 的进程 PID | 不迁移 | PID 只对旧进程有效；受控停止旧 daemon 后由新 daemon 重建 |
| 已核对 | Linux `$XDG_CONFIG_HOME/systemd/user/bitfun-cli-daemon.service` | 旧 CLI daemon 自启动单元 | P0（系统集成） | stop/disable 旧 unit，安装并启动 `openbitfun-cli-daemon.service`；不能简单复制，因为 ExecStart 和 unit 名均变化 |
| 已核对 | macOS `~/Library/LaunchAgents/com.bitfun.cli.daemon.plist` | 旧 CLI daemon LaunchAgent | P0（系统集成） | bootstrap/bootout 有序切换到 `com.openbitfun.cli.daemon`，并更新可执行文件路径 |
| 已核对 | `~/.bitfun/dispatch/install/` | SSH 自动安装 CLI 的 archive、脚本和 staging | 不迁移 | 安装传输产物可由当前签名 release 重新供应 |
| 已核对 | 旧 `bitfun` CLI 可执行文件/软链接 | 旧 daemon、交互命令和自升级入口 | P0（升级入口），不属于用户数据 | 当前 Linux self-update 的 `install_archive` 明确要求当前文件名为 `openbitfun`，并从 archive 取同名 binary；以 `bitfun` 文件名运行会被拒绝。品牌切换必须由外部 bootstrap/package manager 或专门的旧版 bridge 安装新 binary，再切换 service/PATH；验证新 CLI 与 daemon 后再决定保留 `bitfun` compatibility shim 或清理。Windows/macOS 当前 self-update 本来不支持，分别走 installer/package manager |

旧 daemon 与新 daemon 不能长期并行：同机进程共享 `device_id`，Remote Connect 按该 ID 路由，后连接者会抢占路由。升级顺序必须是“停旧服务 → 迁移身份/账户 session → 安装新服务 → 验证在线”。

### 5.8 持久 metadata 与跨版本 wire 保留键

品牌提交还改名了若干双下划线保留键。它们不一定各自形成独立文件，但会进入持久 Turn/缓存，或跨 Desktop、CLI、Runtime、Peer 的事件 envelope。当前 reader 都只读取新名称，因此必须在兼容层双读，而不能指望目录复制自动修复。

| 状态 | 旧键 → 新键 | 出现位置与作用 | 迁移/兼容结论 |
|---|---|---|---|
| 已核对 | `__bitfunOptimisticDispatchJobId` → `__openbitfunOptimisticDispatchJobId` | `DialogTurn.userMessage.metadata` 中的 optimistic-turn adoption 标记；会随 Session Turn、dispatch transcript cache 等完整 Turn 持久化 | P0（含仍 pending 的 Turn）。读取与 strip 阶段同时接受旧/新键，写回时规范化为新键。当前只读新键，旧 optimistic Turn 可能无法被 target 的正式 Turn 接管，造成可见重复 Turn |
| 已核对 | `__bitfunRuntimeStreamId`、`__bitfunRuntimeEventCursor` → 对应 `__openbitfun*` | Runtime 投递给 Desktop/Web/Peer 的事件 payload 游标，用于检测跨进程 stream 与事件 gap | wire 兼容 P0。接收端在支持旧 peer/runtime 的窗口内双读并同时剥离两组键；发送端应按协商能力选择名称或在过渡期双写。否则旧键会被当作无游标事件，gap fencing/replay 语义失效 |
| 已核对 | `__bitfunSourceDeviceId`、`__bitfunSourcePayload` → 对应 `__openbitfun*` | Desktop 给 peer-originated event 加的来源设备和非对象 payload wrapper；通常只存在于 Remote Connect/Peer 事件 envelope | wire 兼容 P0，不作为独立磁盘 store 搬迁。混合版本双读/双剥离；否则 peer 事件可能被误判为本地未标记事件，或把 wrapper 本身交给业务消费者，造成跨设备事件串线 |
| 已核对 | Runtime identity hash domain `bitfun-agent-runtime-instance-v2` → `openbitfun-agent-runtime-instance-v2` | Shared Runtime 根据产品、channel、用户数据根、protocol、workspace 计算的实例 hash；hash 命名 discovery/lock，并派生 named pipe/UDS 名 | 不迁移。它是活进程 ownership/discovery 边界，不是 Session identity；新版本应创建新实例。升级器需先停旧 Shared Runtime，不能为了“复用”而伪造新 hash 指向旧 endpoint |
| 已核对 | CLI stdio Host identity `bitfun-cli-server-host` → `openbitfun-cli-server-host` | `openbitfun server` 注入的进程内 `AppServerHostPolicy` identity | 当前无独立落盘对象，不迁移。它不是 `device_identity.json`，也不应据此改 Session/workspace ID；若外围客户端另有持久 host allowlist，那属于该客户端自己的跨版本兼容数据，需同时接受旧/新 identity 后再规范化 |
| 已核对 | `bitfun://runtime/...`、`bitfun://current-session/...` → `openbitfun://...` | tool path、tool result、compression transcript、session reference 等会把逻辑路径写入 Session Turn 和相关持久记录 | P0（历史记录仍需恢复/解析时）。不能对 Session JSON 全局替换；path parser/resolver 在兼容期双读旧 scheme，完成 workspace/session 边界和 traversal 校验后只在运行时 canonicalize。新记录只写新 scheme；当前 `tool-contracts` parser 仅声明新前缀 |
| 已核对 | `bitfun://payloads/...`、`bitfun://artifacts/...`、`bitfun://logs/...` → `openbitfun://...` | adapter/runtime diagnostics 中的 payload、artifact 和 log URI，可能随 tool/plugin 结果持久化 | 保留原始历史 URI用于展示与审计；需要 dereference 的 owner 双读并映射到已验证对象，不按字符串前缀直接拼本地路径。新记录只写新 scheme |
| 已核对 | `bitfun://pair?...` → `openbitfun://pair?...` | Remote Connect 二维码/深链，可能被用户保存、转发，或在混合版本手机与 Desktop 间传递 | wire/deep-link 兼容 P0，不属于磁盘目录搬迁。新移动端在兼容窗口内接受旧链接并按字段校验；不能只接受新 scheme 后要求用户重新生成仍有效的旧链接 |

`~/.bitfun/runtime-events/*.jsonl` 不需要为上述游标键做全局文本替换：其落盘 record 的稳定字段是 `streamId`、`cursor`、`event`。该目录仍按 5.1 作为 Session 恢复数据迁移，保留原始 JSONL 顺序与字段即可。保留键兼容应放在事件 envelope owner，而不是数据目录迁移器中。

### 5.9 持久环境变量与启动配置

进程环境变量本身不是目录中的文件，但用户或部署者会把它们持久化在 Windows `HKCU\Environment`/系统环境、PowerShell profile、`.bashrc`/`.zshrc`/`.profile`、systemd unit/drop-in、LaunchAgent plist、Task Scheduler/服务配置、Docker Compose `.env` 或部署 env 文件中。当前运行时大多只读 `OPENBITFUN_*`，因此它们属于升级发现与配置迁移范围。

| 状态 | 旧变量 → 新变量 | 作用与优先级 | 迁移方式与注意事项 |
|---|---|---|---|
| 已核对 | `BITFUN_HOME` → `OPENBITFUN_HOME`；`BITFUN_USER_ROOT` → `OPENBITFUN_USER_ROOT`；`BITFUN_DEVICE_IDENTITY_PATH` → `OPENBITFUN_DEVICE_IDENTITY_PATH` | 决定产品 Home、用户数据根和设备身份文件的真实位置；P0 | migration bootstrap 必须在初始化新 `PathManager`、CLI config、Remote Connect store 前读取。新变量存在时始终优先；仅在目标变量缺失时把有效旧路径作为源定位/精确迁移候选，不能先使用新默认根再寻找旧数据 |
| 已核对 | `BITFUN_BUN_COMMAND`、`BITFUN_OPENCODE_BUN_HOST_ENTRY` → 对应 `OPENBITFUN_*` | Plugin Host/Server 的 Bun 命令和 host entry 覆盖；P1 | 仅在已识别的产品启动配置 owner 中改 key 并保留原值；目标变量优先。路径或命令需重新验证可执行性，不全局替换 shell/profile 文本 |
| 已核对 | `BITFUN_MINIAPP_MARKET_API_URL`、`BITFUN_APPEARANCE_MARKET_API_URL` → 对应 `OPENBITFUN_*` | 自定义 Market endpoint；P1 | 精确改名并验证 URL；目标值优先。不能把默认生产 URL 推断成用户显式配置，也不能因旧 endpoint 离线而删除它 |
| 已核对 | `BITFUN_CLI_DISABLE_AUTO_UPDATE`、`BITFUN_LOG_DIR` → 对应 `OPENBITFUN_*` | CLI 自动更新策略和自定义日志目录；P1/P2 | 在产品管理的 service/env 配置中迁移；布尔开关按存在语义保留，自定义路径保持原意。日志本身仍不迁移，且旧路径不能在未校验时被递归搬运 |
| 已核对 | `BITFUN_MIRROR` 及同组 mirror/Docker/release route 变量 → `OPENBITFUN_*` | Relay/self-host 的用户或 operator 部署选择；条件 P0/P1 | 若位于持久 `.env`、systemd/service 配置或产品管理的 deploy state，按 owner 的明确字段逐项迁移；自动探测结果和单次脚本变量不迁移。signing secret、release credential 等由 operator/secret manager 改名，Desktop 迁移器不得读取 |
| 已核对 | `BITFUN_MODELS_DEV_PATH/URL`、`BITFUN_OPEN_DEVTOOLS`、`BITFUN_BROWSER_VIDEO_DECODER_MODE`、`BITFUN_PERF_TRACE`、`BITFUN_WEBDRIVER_*` → 对应 `OPENBITFUN_*` | 开发、诊断、WebDriver 覆盖 | 默认不迁移；开发者若仍需要应在自己的启动配置中显式改名。迁移器不应把诊断行为带入普通生产首启 |
| 已核对 | `BITFUN_CLI_BACKGROUND_UPDATE` 以及 test child/PID/lock/ready/E2E 变量 → 对应 `OPENBITFUN_*` | 子进程协议、互斥或测试瞬时状态 | 不迁移。由新进程重新生成；把旧值带入新进程可能误判后台模式、锁或测试环境 |
| 已核对 | updater endpoint、release/signing key、Android signing、package/release channel 等 build/CI-only 变量 | 构建与发布基础设施 | 不属于用户数据迁移。由 CI/secret owner 独立改名和轮转，客户端不得发现、复制或记录这些值 |

迁移器只能自动修改自己拥有且有结构定义的注册表值、service/drop-in、plist 或 `.env`。对任意 shell/profile/用户文件只报告“发现旧变量名及文件位置”，不得做全局文本替换；尤其不能在报告中记录变量值。

### 5.10 Hosted MiniApp/Skin Market 服务数据

这组是自部署/生产运维数据，不在 Desktop 用户根中。品牌提交把两个服务的系统路径、container/service 名和 artifact contract 一并改名；旧实例与新实例不能同时写同一数据库或 artifact tree。

| 状态 | 旧位置/对象 → 新位置/对象 | 作用与优先级 | 迁移方式与注意事项 |
|---|---|---|---|
| 已核对 | `/srv/bitfun-miniapp-market/data/market.sqlite` → `/srv/openbitfun-miniapp-market/data/market.sqlite` | MiniApp Market 账户、投稿、release 等权威数据库；P0 | 停止旧服务，用 SQLite backup 生成 offline copy，运行结构迁移并做 `integrity_check`，再切换新服务。不能复制 live DB 或让新服务先创建空库 |
| 已核对 | `/srv/bitfun-miniapp-market/artifacts` → `/srv/openbitfun-miniapp-market/artifacts` | 用户上传的 MiniApp package、截图等 artifact；P0 | 与 DB 引用闭包一起复制到 offline staging，校验引用、hash、size 和权限后原子切换；不能把 artifact 当构建缓存 |
| 已核对 | `/srv/bitfun-skin-market/data/market.sqlite` + `artifacts/` → `/srv/openbitfun-skin-market/...` | Skin 投稿/release DB、appearance package 和 preview；P0 | DB 与 artifact tree 必须成组离线迁移。只移动目录仍会留下旧 archive/schema/hash，需执行下述专用转换 |
| 已核对 | 两个旧根的 `backups/` → 新根 `backups/` | 数据库与 artifact 的 daily/weekly 恢复资产；P0/P1 | 保留原备份为不可变 legacy recovery set；不要让新 backup rotation 在验证前裁剪旧集合。进入新恢复集合前应演练完整 restore 和 schema migration |
| 已核对 | `/etc/bitfun-miniapp-market/market.env`、`/etc/bitfun-skin-market/market.env` → 对应 OpenBitFun 路径 | OAuth、session/CSRF secret 和服务配置；P0/高敏感 | 由 operator 在受控权限下复制并按字段改名，保持 `root:root`/`0600` 等权限；不得把内容写入迁移报告、普通 backup 或 Desktop 迁移器 |
| 已核对 | 旧 container、systemd service/timer、backup/restore executable 名 → OpenBitFun 对象 | 服务启动、备份调度和恢复入口；P0（系统集成） | 停旧 unit/container，安装新对象并指向已验证的新路径；验证 DB/artifact/backup 后再启用 timer。不得让新旧 timer 同时操作同一数据 |
| 已核对 | MiniApp DB JSON 中 `minBitfunVersion`/`minOpenbitfunVersion` → `minOpenBitFunVersion` | `submissions.metadata_json`、`releases.metadata_json` 的最低客户端版本 contract；P0 | 使用 `deploy/openbitfun-host/migrate-market-data-v1.py miniapp` 在 offline DB 上转换并规范为 `1.0.0`；脚本会拒绝传入声明为 live 的 DB，解析冲突时应中止而非猜测 |
| 已核对 | Skin `.bitfun-appearance`、`appearance.json` schema `bitfun.appearance` 和旧 DB 引用 → OpenBitFun contract | package 内容寻址、manifest、review bundle 与 DB 引用；P0 | 使用同一工具的 `skin` 模式重写 archive、改 schema、重算 package SHA-256/size/manifest/review-bundle hash 并同步 DB；旧 archive 移入 artifact tree 下 `migrated-bitfun-v1`。不能只改扩展名或只更新 DB |
| 已核对 | `/srv/bitfun-*-market/app` checkout | 部署源码，不是权威用户数据；clean checkout 不迁移/P2，dirty 修改可能是 operator 自定义且应作为 P1 档案 | 新路径按已记录 revision 重新部署；发现 dirty/untracked 文件时保全并报告，由 operator 审核后移植，不能用新 checkout 覆盖 |

## 6. Windows 本机 SSH 数据

Desktop 和 Server 当前都以 `dirs::data_local_dir()/OpenBitFun/ssh` 初始化 `SSHConnectionManager`；品牌提交把中间目录从 `BitFun` 改为 `OpenBitFun`。

| 状态 | 旧目录内相对路径 | 作用 | 优先级 | 迁移方式与注意事项 |
|---|---|---|---|---|
| 已核对 | `ssh_connections.json` | 已保存连接；包括 host/port/user、认证类型、私钥/证书路径、ProxyJump、container、超时重试等，不含密码明文 | P0 | 结构化导入并合并；新增字段有 default。旧 connection ID 可能是 `ssh-<user>@<host>:<port>`，当前稳定格式是无 port 的 `ssh-<user>@<host>` |
| 已核对 | `remote_workspace.json` | 用 `connectionId + remotePath` 标识的远程工作区恢复记录 | P0 | 旧版可能是单对象，当前只接受数组；转数组并与 connection ID 同步改名，保持 `remotePath` 的 POSIX 语义 |
| 已核对 | `known_hosts` | 产品内保存的 SSH host key/fingerprint 信任记录 | P0 | 完整迁移并按 `host:port` 合并；冲突必须显式报告，不能以新或旧值静默覆盖 |
| 已核对 | `.ssh_password_vault.key` + `ssh_password_vault.json` | 随机 AES-256-GCM key 和按 connection ID 索引的密码密文 | P0 | 两文件成组迁移。若 connection ID 改名，需要同步重键 vault entry；密文可原样保留，但必须验证 key 长度和至少一次受控解密 |
| 已核对 | 私钥/证书路径指向的外部文件 | 用户自己的 SSH key/certificate | 不属于品牌目录迁移 | 不复制外部 key；保留路径。若路径恰好位于旧 BitFun 目录内，作为异常情况提示用户或由专用规则迁移并更新引用 |

connection ID 是四个 store 的关联键：`ssh_connections.json`、`remote_workspace.json`、密码 vault 和可能引用该连接的 workspace/dispatch 记录必须在同一个迁移事务中转换。任一凭据不可用时仍应保留 profile 和 workspace，明确提示重新输入凭据，不能把记录裁掉。

## 7. Tauri 与 WebView 持久化

WebView 数据不能整体视为运行缓存。品牌更名同时改变 Tauri identifier 和若干前端 storage key；因此即使复制旧 WebView profile，仍可能需要在前端存储层做旧 key → 新 key 的兼容读取或一次性改名。

| 状态 | 数据 | 作用 | 优先级 | 迁移方式与注意事项 |
|---|---|---|---|---|
| 已核对 | `%APPDATA%/com.bitfun.desktop/.window-state.json` | 窗口大小、位置、最大化等状态 | P2 | 可迁移；恢复前校验显示器和几何，避免旧屏幕布局把窗口放到不可见区域 |
| 已核对 | `webview-recovery.json` | WebView 异常恢复/重建标志 | 不迁移 | 旧版本的异常状态不应触发新版本恢复流程 |
| 已核对 | WebView Local Storage | 语言、布局宽度、模型选择、输入历史、待发送消息、dispatch 观察状态、终端 profile、更新跳过等 | P1，部分 P0 | 复制 profile 后还需迁移改名的 storage key；按 key 设定冲突策略 |
| 已核对 | IndexedDB `openbitfun-appearance` 及旧品牌对应库 | 主题/外观包 | P1 | 做数据库/对象级迁移或验证 UDF 复制可保留；用户自定义外观不可当缓存丢弃 |
| 已核对 | Network/Cookies、Local State、Profile Preferences | 嵌入页面或自动化相关登录态、浏览器 profile 密钥/偏好 | P1/可选 | 若迁移 Cookie，必须在 WebView 关闭时成组复制 `Local State`、Profile 和 Network/Cookies |
| 已核对 | `sessionStorage` | 当前 WebView 进程会话状态 | 不迁移 | 生命周期仅限当前进程 |
| 已核对 | Code Cache、Cache、GPUCache、shader/component caches、Crashpad、运行锁 | 缓存与运行时状态 | 不迁移 | 新 WebView 重建 |

### 7.1 Local Storage 与 IndexedDB 键级清单

下表来自更名提交中 Web 存储 owner 的选择性比较，不包含 CSS class、DOM attribute、事件名等非持久化品牌字符串。所有 Local Storage 迁移都应在 Zustand store、输入队列和页面状态首次 hydrate 之前完成。

| 状态 | 旧 key/数据库 | 新 key/数据库 | 作用与优先级 | 转换与冲突策略 |
|---|---|---|---|---|
| 已核对 | `bitfun.workspace-session-view.v2` | `openbitfun.workspace-session-view.v2` | Session 列表分组、排序和过滤，P1 | 目标不存在时导入；若 payload 的 `filters.hiddenSources` 含旧枚举 `bitfun`，需改为 `openbitfun`，再走当前 store version migrate。目标已存在时保留目标偏好 |
| 已核对 | `bitfun-input-history` | `openbitfun-input-history` | 按 Session 保存的输入历史，P1 | 先走 store 内部 v1→v2 迁移，再按 Session 合并；保持“最近在前”、去重并执行每 Session 100 条上限。不能因 key 改名绕过现有 version migrate |
| 已核对 | `flowChat.pendingQueue.<sessionId>`；`flowChat.peerPendingQueue.v1.<encoded>` | `openbitfun.flowChat.pendingQueue.v1.<encode(JSON.stringify([surfaceId, sessionId]))>` | 尚未发送/等待 drain 的本地与 Peer composer queue，P0 | 这是本次提交中同时改了 key 格式的高风险项。旧 local key 补 `surfaceId=local`，旧 peer key保留元组；按 queue item ID/本地 Turn ID 合并且保留完整 composer payload。必须先写新 key 并验证，再删除新 profile 内的旧 key；源 profile仍只读 |
| 已核对 | `bitfun-dispatch-jobs-v1` | `openbitfun-dispatch-jobs-v1` | controller 的 dispatch observer Zustand 快照，P0/P1 | 与 `~/.bitfun/dispatch/outbound` 对账后导入；按 job ID 合并，backend/outbound 事实优先于陈旧 renderer projection，不得让旧快照复活已 terminal/已删除的 job |
| 已核对 | `bitfun-dispatch-dismissals-v1` | `openbitfun-dispatch-dismissals-v1` | 独立的 job/session dismissal ledger，防止陈旧 renderer 重新投影，P0 | 与目标 ledger 做集合并集并执行当前 2048 条边界；不能仅在 target key 不存在时复制，否则会丢 deletion authority |
| 已核对 | `bitfun-context-storage` | `openbitfun-context-storage` | composer 选择的非 image、非 PR 上下文，P1 | 校验 Zustand envelope；目标为空时导入。两侧均有数据时按 context ID 合并、目标顺序优先，并继续排除当前 owner 明确不持久化的 image/PR context |
| 已核对 | `bitfun-i18n-state` | `openbitfun-i18n-state` | 当前语言、fallback 和自动检测偏好，P1 | 只迁当前 locale contract 仍接受的字段和值；目标已设置时保留目标。`bitfun-language` 不是当前活动 owner |
| 已核对 | `bitfun-manual-terminal-profiles:<workspacePath>` | `openbitfun-manual-terminal-profiles:<workspacePath>` | 每工作区的手工终端 profile，P0/P1 | 从已知 workspace registry 枚举 key，不遍历猜测工作区；按 profile ID 合并并保持命令、cwd/env 等敏感字段，不输出内容。工作区路径若搬迁需同步重键 |
| 已核对 | `bitfun:review-platform:last-remote:<workspacePath>` | `openbitfun:review-platform:last-remote:<workspacePath>` | 每工作区最近选择的 Review remote，P1 | 目标缺失时复制；校验 remote ID 仍存在。工作区路径变更时同步重键，不用控制端 path 语义改写远程路径 |
| 已核对 | `bitfun:settings:acp-agents:hidden-remote-connections:v1` | `openbitfun:settings:acp-agents:hidden-remote-connections:v1` | 用户隐藏的远程 ACP connection ID 集合，P1 | 与目标集合并集；若第 6 节迁移 connection ID，必须同步改值。无效 ID 保留在报告中，不静默当作用户取消隐藏 |
| 已核对 | `bitfun.sessionUsage.export.redactPaths` | `openbitfun.sessionUsage.export.redactPaths` | Session Usage 导出时是否隐藏路径，P1（隐私偏好） | 只接受布尔语义；目标已存在时保留目标。迁移失败使用当前更保守的默认值，不把失败解释为用户明确关闭脱敏 |
| 已核对 | `bitfun.reasoning-presets.recent.v1` | `openbitfun.reasoning-presets.recent.v1` | 每模型最近选择的 reasoning preset，P2 | 按 model ID 合并，目标值优先；仅保留当前模型声明仍支持的 preset，未知值可留在 legacy report |
| 已核对 | `bitfun:remote-connect:disclaimer-agreed:v1` | `openbitfun:remote-connect:disclaimer-agreed:v1` | 远控免责声明已确认状态，P2 | 仅将有效 `true` 导入到空目标；它不是账户授权或设备身份，迁移失败不得影响 Remote Connect 数据本身 |
| 已核对 | `bitfun:update:lastDailyPromptDate`、`bitfun:update:lastPromptedLatestVersion`、`bitfun:update:skippedVersion` | 对应 `openbitfun:update:*` | 更新提示/跳过版本偏好，P2 | 三项作为一个小 store 迁移；校验日期/版本字符串，目标值优先，失败不阻塞主迁移 |
| 已核对 | `bitfun:leftPanel*`、`bitfun:rightPanel*`、`bitfun:bottomTerminalPanelLastHeight` | 对应 `openbitfun:*` | 布局尺寸/折叠偏好，P2 | 当前实际读取的是 `rightPanelLastWidth` 和 `bottomTerminalPanelLastHeight`；只迁活动项并校验有限正数。其余旧键不应因常量仍存在就自动导入 |
| 已核对 | `bitfun-appearance` IndexedDB，stores `packages`、`catalog` | `openbitfun-appearance` | 用户导入/编辑的 Appearance package 与 catalog，P1；用户创作项可视为 P0 | 不能只创建同名空库。按 package ID 读取、校验并写入目标 DB；同 ID 目标项优先，保留冲突副本/报告。需要保证 catalog 与 package 引用闭包 |
| 已核对 | `bitfun-market-theme`、`bitfun-market-locale` | `openbitfun-market-theme`、`openbitfun-market-locale` | MiniApp Market 页面主题和语言；P2 | 同 origin 页面先读新 key，目标缺失时校验旧值并写新 key。普通外部浏览器由页面自己迁移，Desktop 迁移器不扫描浏览器 profile |
| 已核对 | `bitfun-skin-market-theme`、`bitfun-skin-market-locale` | `openbitfun-skin-market-theme`、`openbitfun-skin-market-locale` | Skin Market 页面主题和语言；P2 | 与 MiniApp Market 相同；若站点换了 origin，Local Storage 不会自动跨域，需要服务端 bridge 或用户可控 export/import |
| 已核对 | `bitfun.design-lab.locale` | `openbitfun.design-lab.locale` | Design Lab 语言；P2（开发者） | 仅在 Design Lab 自己的 origin/profile 中迁移有效 locale；不导入 Desktop 主应用 profile |
| 已核对 | `bitfun.design-lab.token-drafts.v1` | `openbitfun.design-lab.token-drafts.v1` | Design Token 编辑草稿；开发者场景 P0/P1 | 在 token editor 首次 hydrate 前按草稿 identity 合并，目标草稿优先并保留冲突副本；不能因它属于开发工具而当缓存丢弃 |
| 已核对 | `bitfun-mobile-language`、`bitfun-mobile-theme` | `openbitfun-mobile-language`、`openbitfun-mobile-theme` | Mobile Web 语言和主题；P1/P2 | 在 Mobile Web bootstrap/Provider 读取前迁移当前 locale/theme contract 接受的值；目标值优先 |
| 已核对 | `bitfun.mobile.install_id`、`bitfun.mobile.user_id` | `openbitfun.mobile.install_id`、`openbitfun.mobile.user_id` | Mobile Web 稳定 install identity、已确认 user ID/表单预填和无账户模式自动重连；P0 | 两项按一个 identity 组合迁移，保持 install ID 原值；目标已有不同 install ID 时进入设备冲突流程，不能覆盖或生成新 ID 后假装完成迁移。user ID 属敏感标识，报告不记录值 |
| 已核对 | `bitfun.mobile.user_id_failure_count`、`bitfun.mobile.user_id_lock_until` | 对应 `openbitfun.mobile.*` | Mobile Web 本地配对保护失败计数与 60 秒 lockout；P0（仍生效的安全状态） | 成组读取并校验数值；仅保留尚未过期的 lockout，过期后由 owner 清除两项。不能只迁 failure count 而丢 lock deadline，也不能把迁移失败解释为成功验证 |
| 已核对 | `bitfun.mobile.last_selected_model_id` | `openbitfun.mobile.last_selected_model_id` | Mobile Web 最近模型选择；P1 | 目标缺失时导入；加载 catalog 后通过现有 normalize/resolve 逻辑验证，失效模型降级为 `auto`，但在 legacy report 中保留旧值摘要而不重写为另一个模型 |
| 已核对 | IndexedDB `bitfun-mobile-remote-cache`，stores `session-state`、`transcripts` | `openbitfun-mobile-remote-cache` | 按 account/device 保存的远端 Session/工作区列表与 transcript 离线投影；P1 | 权威数据仍在目标 Desktop/Relay，但缓存含离线可见历史。按 `accountId + deviceId` scope、Session ID 合并，保留较新的记录并遵守当前每设备数量上限；不能复制底层 IndexedDB 文件到已有目标库 |
| 已核对 | `bitfun-flow-chat-state`、`bitfun-flow-chat-global`、`bitfun-session-ids`、`bitfun-session-*` | 提交中改成 `openbitfun-*`，但当前 constructor 仍会主动清除此组 | 已退休的旧 renderer Session cache，不迁移 | 权威会话在 Rust Session store。不要把这组键导入新 profile；否则下一次启动也会被 `clearOldStorage()` 清掉 |
| 已核对 | `bitfun-*` 常量组中的 `language`、左右 panel、recent workspaces、user preferences、model configs、chat history、diff warning | 提交中有对应 `openbitfun-*` 常量 | 当前 checkout 除 `manual-terminal-profiles` 外没有活动 reader，默认不迁移 | 这些仅定义于 `shared/constants/app.ts`，不能据名称把过时 browser cache 当权威数据。若未来 owner 恢复，需单独补 schema owner 和迁移测试 |
| 已核对 | `bitfun.deepReview.skipCostConfirmation` | `openbitfun.deepReview.skipCostConfirmation` | 当前只在测试 fixture 中出现，生产代码无 reader | 不迁移。不能根据测试字符串推断存在活动偏好 store |
| 已核对 | `bitfun.flowChat.mockBackgroundCommands` | `openbitfun.flowChat.mockBackgroundCommands` | 仅开发环境 mock 开关 | 不迁移 |
| 已核对 | `wing_coder_current_model_config`、`editor-config` | 名称未变 | 当前模型选择与编辑器偏好，P1/P2 | 若采用选择性导出/导入而非复制完整 Local Storage，仍应分别保留；前者的 ID 必须在已迁移模型配置中存在，后者只合并当前接受字段 |

`sessionStorage` 中改名的 `bitfun:flowchat:*`、startup reload、unexpected-exit/config-recovery notice、thread-goal dismiss、`pendingProjectDescription` 和 `pendingOpenSettings` 都不迁移。它们要么只用于当前 renderer 生命周期，要么是一次性导航/防重复提示状态；跨版本恢复反而可能跳过新进程应执行的动作。

### 7.2 WebView profile 的迁移边界

- **目标 UDF 尚不存在**：在 Desktop/WebView 完全退出后，将旧 UDF 中的持久子树复制到同卷 staging，再排除缓存、Crashpad、运行锁和 session-only 数据；校验后原子成为新 UDF。随后在任何页面 store hydrate 前执行 7.1 的 key/schema 迁移。
- **目标 UDF 已存在**：不能把旧 `Local Storage/leveldb`、IndexedDB 或 Cookies 的底层文件覆盖/拼接到目标目录。应以独立 WebView2 environment 或受支持的 store 导出器只读打开旧 UDF，再通过各 owner 合并到目标；若无法安全读取，保留旧 UDF并报告，不创建“成功但数据为空”的标记。
- **Cookie/登录态是单独承诺**：只有产品明确承诺保留嵌入页登录态时，才迁移 `Local State`、对应 Profile、Network/Cookies 和相关密钥材料；同一 Windows 用户之外不可假设密文可解。浏览器自动化 profile 仍按 4.4 独立处理，不能和 Desktop WebView UDF混用。

### 7.3 Desktop 开发 identifier

`src/apps/desktop/tauri.dev.conf.json` 也从 `com.bitfun.desktop.dev` 改为 `com.openbitfun.desktop.dev`。它会形成与正式版、以及新 dev build 都不同的 Tauri app-data/WebView profile：

| 状态 | 旧位置 | 作用 | 优先级 | 迁移方式与注意事项 |
|---|---|---|---|---|
| 已核对 | `%APPDATA%/com.bitfun.desktop.dev` | dev build 的窗口状态和 Tauri app data | P2（开发者） | 只迁有价值的窗口偏好；排除 `webview-recovery.json`。不得导入正式 `com.openbitfun.desktop` profile |
| 已核对 | `%LOCALAPPDATA%/com.bitfun.desktop.dev/EBWebView` | dev build 的 Local Storage、IndexedDB、Cookie 与缓存 | 分项判断（开发者） | 按 7.1/7.2 同样做 store 级迁移；通常不承诺保留 dev 登录态，不能把 dev Cookie 合并进正式 profile |

生产升级器默认只处理正式 identifier。dev profile 应由显式开发者迁移开关处理，以免普通用户机器上的历史测试 profile 污染生产数据。

### 7.4 Cookie 名称变更

Cookie 是否迁移取决于服务端 session 的生命周期，不能看到 `Cookies` 数据库里有旧名称就统一改名：

| 状态 | 旧名 → 新名 | 生命周期/作用 | 迁移结论 |
|---|---|---|---|
| 已核对 | `bitfun_html_preview` → `openbitfun_html_preview` | Desktop 每次 `html_preview_create` 生成随机 token、随机 localhost 端口和进程内 preview server；Cookie 无 `Max-Age`/`Expires`，release 或进程退出后 server 状态消失 | 不迁移。旧 token 对新进程没有授权意义；即使 UDF 中残留也应排除/忽略，不得复制为新名 |
| 已核对 | `bitfun_market_session` + `bitfun_market_csrf` → `openbitfun_market_*`；`bitfun_skin_session` + `bitfun_skin_csrf` → `openbitfun_skin_*` | MiniApp/Skin Market 的 7 天 Web session；session token 在服务端 DB 中校验，CSRF token 必须与其 hash 配对，Cookie 分别 scoped 到 `/miniapp`、`/skin` | P1（承诺保留市场登录态时）。优先让服务端在有限兼容期接受完整旧 session+CSRF 对，验证仍有效后重新签发/设置新名称并 expire 旧名称；不要直接编辑 Chromium Cookies DB，也不能只改 session cookie 而漏 CSRF。若不承诺登录态则明确要求重新登录 |
| 已核对 | `bitfun_page_access` → `openbitfun_page_access` | Relay Pages 的 8 小时浏览器 grant；token 只映射到 Relay 进程内 `PageAccessManager`，服务重启即失效 | 不迁移。升级重启后旧 grant 本来不可恢复；用户重新完成 Page 登录/授权即可。不能把它与 Relay DB 或账户 session 混为一谈 |

市场 Cookie 可能存在于 Desktop WebView，也可能存在于普通外部浏览器；迁移系统只能处理产品管理的 UDF。外部浏览器应依赖服务端旧名兼容和安全重签，不能扫描或修改用户浏览器 profile。

### 7.5 Installer WebView identifier

安装器也从 `com.bitfun.installer` 改为 `com.openbitfun.installer`，会形成新的 Tauri app-data/WebView profile。对更名前后安装器源码的定向扫描没有发现 Local Storage、sessionStorage 或 IndexedDB owner；安装步骤状态保存在 React 进程内，最终通过 command 写入产品配置和 installer state。

| 状态 | 旧位置 | 作用 | 优先级 | 迁移方式与注意事项 |
|---|---|---|---|---|
| 已核对 | Windows `%APPDATA%/com.bitfun.installer`、`%LOCALAPPDATA%/com.bitfun.installer/EBWebView` | Installer 自己的 Tauri/WebView profile；未发现应用级持久 store | 不迁移 | 默认视为安装器运行缓存/profile，由新安装器重建；不要把它并入 Desktop 的 `com.openbitfun.desktop` UDF |
| 已核对 | Installer command 写入的 `%APPDATA%/bitfun/config/installer-state.json`、语言/主题/模型等产品配置 | 安装结果和用户选择，owner 在产品用户目录而非 Installer WebView | 按对应产品 store 分项迁移 | `installer-state.json` 按 4.1/第 8 节处理；配置字段随 `app.json`/CLI config 的 schema-aware 迁移，不能因为 Installer profile 不迁而漏掉这些输出 |

## 8. Windows 安装目录、注册表与快捷方式

这一节属于**升级系统集成**而非用户数据复制，但品牌更名后若不处理，会得到两个并存安装、两个卸载项或失效快捷方式。当前新安装器只读取 OpenBitFun 名称的注册表键，并且总是把用户选择的父目录规范化到 `OpenBitFun` 子目录；它不会识别或原地接管旧 `BitFun` 安装。

| 状态 | 旧对象 | 新对象 | 作用/优先级 | 迁移方式与注意事项 |
|---|---|---|---|---|
| 已核对 | 默认 `%LOCALAPPDATA%/BitFun`；自定义父目录下的 `BitFun/` | 对应 `OpenBitFun/` | 已安装二进制与资源；P0 升级流程、非用户数据 | 不把旧安装目录当用户数据搬迁。先从旧注册表确认真实位置和签名/版本，正常安装并验证新 payload，再调用受信任的旧卸载路径或按 manifest 清理旧 payload。发现未知文件时保全并报告，不能递归删除整个旧目录 |
| 已核对 | `bitfun-desktop.exe` | `openbitfun-desktop.exe` | 主程序入口；P0 | 所有快捷方式、PATH、Run 值和卸载记录必须成组更新；不能只改文件名，因为签名、资源和 sidecar 也随发行包变化 |
| 已核对 | `HKCU/HKLM\Software\Microsoft\Windows\CurrentVersion\Uninstall\BitFun` | `...\Uninstall\OpenBitFun` | Add/Remove Programs、安装位置、旧卸载命令；P0 | 旧键只用于发现和调用旧卸载器，不能复制后字符串替换其中命令。新安装成功后写新键，再移除确认属于旧产品的键；兼顾 per-user/per-machine hive |
| 已核对 | `HKCU/HKLM\Software\BitFun Team\BitFun` | `...\OpenBitFun Team\OpenBitFun` | Tauri NSIS `MANUPRODUCTKEY` 安装位置；P0 | 读取旧默认值作为发现证据；不要把旧 `BitFun` 末段直接交给当前 `prepare_install_target`，否则会形成 `BitFun\OpenBitFun` 嵌套目录 |
| 已核对 | Desktop/Start Menu 的 `BitFun.lnk`、可能的 `Programs\BitFun\` | `OpenBitFun.lnk`、当前 Start Menu 布局 | 用户入口；P1 | 新快捷方式验证目标存在后再删旧快捷方式；不能原样复制 `.lnk`，其中目标仍指向旧 exe |
| 已核对 | `HKCU\...\Run` 的 `BitFun` 值、用户 PATH 中的旧安装目录 | `OpenBitFun` 值、新安装目录 | 自动启动和 CLI/程序发现；P0/P1 | 若用户原先启用则等价迁移，避免双启动；PATH 精确按规范化目录元素替换，不做子串替换。目标验证前保留旧入口 |
| 已核对 | `HKCU\Software\Classes\Directory\{Background\}shell\BitFun` | 当前产品不再注册该菜单 | 退休的资源管理器右键菜单；不迁移 | 新版验证成功后清理确认为旧产品的键；当前卸载代码只清理 `OpenBitFun` 名称，升级路径还需显式覆盖旧键 |
| 已核对 | `%APPDATA%/bitfun/config/installer-state.json` | `%APPDATA%/openbitfun/config/installer-state.json` | 最近安装路径；P2 | 与 4.1 同一 store。只能作为候选提示，必须再与旧注册表和实际签名 payload 交叉验证；它不能证明安装仍有效 |

安装器在安装末期还会写新根的首次启动语言、主题和模型配置。因此品牌迁移必须先运行，或让安装器通过同一个 schema-aware merge owner 写入；否则安装器会先制造“目标非空”，使后续迁移误判为一次普通全新安装。

## 9. Android、iOS 与 HarmonyOS 应用数据

这一组不在 Desktop 的两个根目录内，但同一个更名提交同时修改了移动端 package/bundle ID 和 store 名。若这些客户端已向用户发布，它们属于独立的 P0 升级迁移项目。

### 9.1 跨沙箱边界

| 平台 | 旧应用标识 | 新应用标识 | 直接影响 | 迁移边界 |
|---|---|---|---|---|
| Android | `com.bitfun.mobile`（debug 为 `.debug`） | `com.openbitfun.mobile`（debug 为 `.debug`） | 产生新的 `/data/user/<n>/<package>` 私有目录、UID 和 AndroidKeyStore 访问域；不是同一 package 的覆盖升级 | 首选保持已发布 application ID 不变，只改代码 namespace/品牌。若新 ID 已不可逆，需先发布由旧签名应用执行的显式 exporter/受签名权限保护的桥接，再由新应用导入；新应用不能直接复制旧 SharedPreferences/DB/Keystore |
| iOS | `com.bitfun.mobile.ios` | `com.openbitfun.mobile.ios` | 新 data container、UserDefaults domain；默认 Keychain access group 也会随签名 app identifier 改变 | 首选保持已发布 bundle ID。否则需在旧版仍可更新时建立 App Group/Keychain access group 或加密导出协议，并验证 provisioning entitlement；仅保持相同 Team ID 不足以证明新版可读旧 Keychain |
| HarmonyOS | `com.bitfun.app` | `com.openbitfun.app` | 新应用沙箱、Preferences/RDB/HUKS 域；distributed KV 的隔离还取决于 bundleName + storeId | 首选保持已发布 bundleName。否则由旧应用先解密并导出，再通过平台支持的受控通道导入；要同时覆盖手机和手表，不能只迁一端的 distributed KV |

提交后的当前 runtime owner 未保留下表旧名称的兼容读取；而 package/bundle ID 已经先把旧数据隔离在另一个应用域。因此“新应用启动时检查旧文件名”本身并不能完成移动端迁移。

### 9.2 Android/iOS 共享数据域

| 状态 | 旧 store | 新 store | 作用/优先级 | 转换与注意事项 |
|---|---|---|---|---|
| 已核对 | `bitfun-mobile.db` | `openbitfun-mobile.db` | SQLDelight：本地 General Chat Session/message、composer draft，以及按设备保存的远端 Session 列表、transcript 和 replay cursor；用户聊天/草稿 P0，远端投影 P1 | 由旧应用导出逻辑在数据库一致性点读取，再让新 schema owner transaction 导入全部关联表；不能复制正在打开的 SQLite 文件，也不能只迁 Session 不迁 message/draft/cursor |
| 已核对 | Android `bitfun_install/install_id`；iOS UserDefaults `bitfun.mobile.install_id` | 对应 `openbitfun_*` / `openbitfun.mobile.*` | 移动客户端向 Desktop/Relay 呈现的稳定设备身份；P0 | 保持原值，避免升级被当作全新设备并留下孤立路由。目标已有不同 ID 时不能覆盖，应进入账户/设备冲突流程 |
| 已核对 | Android `bitfun_app_settings`；iOS `bitfun.mobile.language` | 对应 OpenBitFun 名称 | 主题（Android）与语言偏好；P1/P2 | 只导入当前枚举/locale contract 接受的值；目标已由用户设置时保留目标 |
| 已核对 | Android `bitfun_secure_cloud_account` + AndroidKeyStore alias `bitfun.mobile.cloud_account`；iOS Keychain service `com.bitfun.mobile.account` | 对应 OpenBitFun secure domain | `cloud_account_session`：relay URL、username、token、user ID、account master key、目标设备选择；P0/高敏感 | 必须由旧安全域解密读取、校验记录后重新加密写入新域；不能复制 Android preferences 密文或只改 alias/service。全部写入成功后才建立已登录状态 |
| 已核对 | Android `bitfun_secure_general-chat` + alias `bitfun.mobile.general-chat`；iOS service `com.bitfun.mobile.generalchat` | 对应 OpenBitFun secure domain | General Chat base URL、model、API key、selected model ID；P0/P1/高敏感 | 四个逻辑字段成组迁移，API key 不得出现在迁移日志；目标存在时按完整 provider 配置裁决，不能混出“旧 URL + 新 key” |
| 已核对 | Android `bitfun_secure_pairing_protection` + alias `bitfun.mobile.pairing_protection`；iOS service `com.bitfun.mobile.pairing` | 对应 OpenBitFun secure domain | `user_id_protection` 连续失败次数和 lockout 截止时间；P0（安全状态） | 保留仍未过期的冷却状态；过期记录由 owner 清除。迁移失败不能被解释为一次成功配对，也不能用它替代 install identity |

### 9.3 HarmonyOS store 级清单

| 状态 | 旧 store/alias | 新 store/alias | 作用/优先级 | 转换与注意事项 |
|---|---|---|---|---|
| 已核对 | Preferences `bitfun_remote_identity` | `openbitfun_remote_identity` | install/device identity；P0 | 保持有效身份；目标冲突不覆盖 |
| 已核对 | Preferences `bitfun_cloud_account` + HUKS alias `bitfun_cloud_account_session` | 对应 `openbitfun_*` | 云账户 session 密文与加密 key；P0/高敏感 | 在旧应用域中解密并验证，随后用新 HUKS key 重新加密；复制 preferences 密文后改 alias 无法解密 |
| 已核对 | RDB `bitfun_general_chat.db` | `openbitfun_general_chat.db` | General Chat 本地 Session、消息和草稿；P0 | 按 RDB schema/transaction 导出导入，保留 Session-message-draft 引用闭包 |
| 已核对 | Preferences `bitfun_general_chat_config` + HUKS alias `bitfun_general_chat_api_key` | 对应 `openbitfun_*` | General Chat endpoint/model/选择和 API key；P0/P1/高敏感 | 配置与 secret 成组迁移并重新加密；不能分别采用不同侧的 provider 字段 |
| 已核对 | RDB `bitfun_remote_sessions.db`、`bitfun_remote_chat.db` | 对应 `openbitfun_*` | 远端 Session 列表、消息、cursor/离线投影；P1 | 按 device/session 主键合并，目标事实优先；解析失败保留旧记录而不是重置空库 |
| 已核对 | Preferences `bitfun_app_locale` | `openbitfun_app_locale` | locale；P1/P2 | 校验当前 locale contract 后导入 |
| 已核对 | distributed KV `bitfun_harmony_handoff_s1` | `openbitfun_harmony_handoff_s1` | 手机与手表间的账户/控制 handoff；P0（活动交接） | store ID 和 bundleName 都参与隔离。旧端、新端及手表必须按同一版本化协议双读/转存；只改 store ID 会让两端看到空 store |
| 已核对 | KV key `bitfun.account.provision.request`、`bitfun.account.provision.response` | `openbitfun.account.provision.request`、`openbitfun.account.provision.response` | phone/watch provisioning 的活动 request/response；P0（活动交接） | 与 handoff store ID 和 bundleName 作为一个协议版本迁移。兼容窗口内两端双读旧/新 key，并按 request identity 配对；不能只复制 response、只改 store ID，或让旧 response 应答不相关的新 request |

移动端凭据导出必须是用户可感知、短时、一次性的加密迁移流程，并且在新应用确认导入前不删除旧应用数据。若旧包已无法更新、平台又没有共享容器/备份恢复能力，应明确报告“无法自动迁移并需重新登录”，不能伪造迁移成功。

## 10. 迁移系统必须遵守的合并与失败策略

1. **旧源只读且不删除。** 首次成功也不要立即删除旧目录；保留可回退窗口，由单独的显式清理流程处理。
2. **在各 store 首次打开前执行。** SQLite、密钥 vault、WebView UDF 等不能在新旧进程仍持有写锁时复制。
3. **新目录为空时原子提交。** 先写同卷 staging，校验成功后 rename/replace；失败不留下“看似已迁移”的半成品。
4. **新目录已有数据时按记录合并。** 不允许整目录覆盖。默认保留新数据；ID 冲突应保留双方或由 owning store 明确裁决，并写入可见报告。
5. **凭据必须先读旧、再写新、再提交 metadata。** 解密失败不能静默等价为“用户未登录”，也不能先删除旧 secret。
6. **SQLite 使用 backup/transaction。** 不单独复制 `.sqlite` 主文件；WAL 中可能还有未 checkpoint 的提交。
7. **迁移可重入。** 维护 migration manifest，记录源/目标、store 版本、hash 或稳定摘要、每项状态和错误；重复启动只重试未完成/可重试项。
8. **解析失败保留并显式降级。** 不能通过删除、重置或静默创建空配置来“恢复”。
9. **远程路径保持 POSIX 语义。** `remote_ssh` 和远端 dispatch/workspace 记录不能用控制端 Windows path join/split 重新解释。
10. **敏感内容不进入日志。** 报告文件只记录 store、路径、错误类别和摘要，不输出 Token、Cookie、密钥、配置值、会话文本或终端内容。

## 11. 静态核对结论与实现阶段待办

本清单已完成对已知产品根、workspace runtime、Session/Dispatch、Remote Connect、SSH、WebView/browser storage、CLI updater、Hosted Market、安装系统、Android/iOS/HarmonyOS，以及品牌提交中相关 storage key/service name/路径名的定向静态核对。没有展开 4,748 文件的完整 diff；结论来自当前 owner 与 `8784bbc^` 对应文件的选择性比较。

“静态核对完成”不等于迁移系统已经可发布。实现阶段仍需：

1. 建立最早期 migration bootstrap，在 `PathManager`、CLI config、SQLite、vault、WebView 和各页面 store 首次打开前完成源发现；旧根覆盖环境变量必须在这里处理。
2. 建立显式 store registry。每项声明 source、target、owner、schema/version、敏感级别、冲突策略、可重入 marker 和验证器；不提供“递归复制整个旧根”的后门。
3. 先实现 P0 owner：全局配置、workspace registry、Session/runtime-events、设备/账户身份、权限/cron、Remote Connect、SSH、移动端身份/凭据、活动 Dispatch/worktree、Market DB/artifact。
4. 为 URI、metadata、wire key、pairing link 和混合版本 peer 增加兼容期双读/能力协商；持久历史保留原始值，新写入只用 OpenBitFun canonical 名称。
5. 对旧 payload 做 fixture 驱动的迁移测试：legacy deserialize、目标为空、目标已有数据、冲突、损坏/WAL、重复执行、失败回滚、old-payload round trip 和敏感日志无泄漏。仅用当前版本新写数据的测试不算升级覆盖。
6. 提供默认 dry-run 和机器可读/用户可读报告，只输出路径、store、数量、摘要和错误类别；实际迁移继续保持旧源只读。清理旧数据必须是迁移验证后的独立显式动作。
7. 分别验证 Windows/macOS/Linux，本地 workspace、Remote workspace、Remote control、Peer Device、Detached Dispatch，以及 Hosted Market offline cutover。当前文档没有任何运行证据，不能用本地静态检查替代这些场景。

对于 `data/telemetry/uid`、旧 workspace `memory/` 和任意机器上出现的未知非空目录，当前结论仍是“保全并报告，不导入 active store”。这不是遗漏项，而是没有可靠 owner/schema 时的明确安全边界。

## 12. 主要证据文件索引

| 数据域 | 主要当前 owner/证据 |
|---|---|
| 产品路径与 identity | `src/crates/assembly/core/src/infrastructure/app_paths/path_manager.rs`；`src/crates/contracts/core-types/src/product_identity.rs`；`src/apps/cli/src/config.rs` |
| CLI updater 状态 | `src/apps/cli/src/self_update.rs` |
| Session/workspace runtime | `src/crates/assembly/core/src/agentic/persistence/manager.rs`；`src/crates/assembly/core/src/infrastructure/app_paths/path_manager.rs` |
| tool path URI | `src/crates/execution/tool-contracts/src/framework.rs`；`src/crates/assembly/core/src/agentic/tools/tool_context_runtime.rs` |
| Remote Connect identity/session/sync | `src/crates/services/services-integrations/src/remote_connect/device.rs`；`session_store.rs`；`sync_state.rs`；`src/crates/services/services-integrations/src/remote_connect.rs` |
| MiniApp Market 客户端凭据 | `src/crates/services/services-integrations/src/miniapp_market/credentials.rs` |
| Web UI storage | `src/web-ui/src/shared/constants/app.ts` 及第 7.1 节各 key 的直接 reader；`src/web-ui/src/infrastructure/appearance/storage/AppearanceStorage.ts` |
| Mobile Web storage | `src/mobile-web/src/pages/PairingPage.tsx`；`src/mobile-web/src/pages/ChatPage.tsx`；`src/mobile-web/src/services/RemoteCache.ts`；theme/i18n providers |
| Hosted Market | `deploy/openbitfun-host/migrate-market-data-v1.py`；`deploy/miniapp-market/`；`deploy/skin-market/` |
| Installer identifier | `OpenBitFun-Installer/src-tauri/tauri.conf.json`；更名前路径 `BitFun-Installer/src-tauri/tauri.conf.json` |
| HarmonyOS handoff/provision | `src/apps/mobile/harmonyos/entry/src/main/ets/services/WatchHandoffStore.ets`；`WatchProvisionProtocol.ets` |
| 品牌前后对照 | 当前 checkout 与提交父版本 `8784bbc^` 的上述对应文件；未使用完整 diff 作为结论来源 |
