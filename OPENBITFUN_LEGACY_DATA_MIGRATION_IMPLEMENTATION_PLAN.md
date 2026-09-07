# OpenBitFun 旧版 BitFun 数据迁移实现方案

> 状态：V1 实现设计
> 日期：2026-09-05
> 上游盘点：[OPENBITFUN_LEGACY_DATA_MIGRATION_INVENTORY.md](OPENBITFUN_LEGACY_DATA_MIGRATION_INVENTORY.md)

## 1. 结论

BitFun 到 OpenBitFun 的迁移不应实现为“把旧用户目录递归复制到新目录”，而应实现为一个**版本化、按数据领域解析、可恢复的离线导入系统**。

V1 采用以下产品与技术形态：

- 提供一个与 OpenBitFun 一同安装、独立签名和打包的 **OpenBitFun Data Migrator**。
- 主应用第一次启动时，在日志、全局配置、会话数据库等常规存储打开之前，只做轻量的旧数据发现和迁移状态判断。
- 检测到旧 BitFun 数据且尚未作出选择时，主应用以 onboarding 模式启动 Data Migrator，然后在打开常规产品存储前退出；“立即迁移”“稍后提醒”“不再提醒”由迁移器提供。
- 用户选择立即迁移后，Data Migrator 留在当前进程中继续预检和迁移；选择稍后或不再提醒时，迁移器记录决定并重新启动主应用。
- Data Migrator 等待主应用及相关写入进程完全退出，按用户选择扫描、预演、暂存、校验和提交，最后按结果重新启动 OpenBitFun。
- 设置页在“数据与维护”下永久保留“数据迁移”入口，支持首次启动后补迁、查看历史报告和重试失败领域。
- 旧 BitFun 数据始终只读；迁移成功后也不自动删除。
- 迁移以领域为原子提交单位，并通过全局迁移日志实现断点续传。不能宣称多个独立数据库和文件目录之间存在真实的全局事务。
- 目标端已有数据默认优先；冲突必须可见，不静默覆盖。
- 所有迁移操作只在存放旧数据的本机执行。远程控制、Peer Device 和 Detached Dispatch 不得把迁移请求静默转发到另一台机器或回退到控制端本地数据。

这比把迁移逻辑放进安装器或主应用进程更稳妥：安装器只负责安装和可选启动，主应用不在自身持有数据库、日志、WebView 配置和运行时锁时改写这些数据，迁移器也可以独立恢复、升级和记录结果。

## 2. 目标与非目标

### 2.1 V1 目标

V1 只保证高价值用户数据的连续性：

1. 设置与服务凭据。
2. Agents、用户 Skills 与用户 MiniApps。
3. 工作区、会话与 Agent 任务状态。
4. 记忆。
5. 远程连接与设备。

系统还必须满足：

- 能判断是否存在可迁移的旧 BitFun 安装。
- 能给出迁移前扫描结果、预计范围、依赖项、冲突和不可迁移项。
- 能安全处理中途退出、进程崩溃、磁盘不足和局部领域失败。
- 同一来源可重复执行，重复执行不产生重复数据。
- 能从旧数据版本升级到当前 OpenBitFun 数据模型，而不要求旧版应用先启动升级。
- 能清楚说明“已迁移”“已跳过”“有冲突”“需要重新登录/重新配对”“失败且可重试”。

### 2.2 V1 非目标

V1 不做以下事情：

- 不逐字节克隆整个 `%APPDATA%\bitfun` 目录。
- 不迁移缓存、日志、临时文件、PID、锁、Socket、IPC 发现文件或 WebView 缓存。
- 不迁移旧版内置资源；OpenBitFun 应从当前安装包重新生成内置 Agents、系统 Skills 和内置 MiniApps。
- 不复制工作区中的真实项目文件；只迁移工作区登记、连接配置和会话引用。
- 不在迁移完成后删除、移动或改名旧数据。
- 不提供 BitFun 与 OpenBitFun 间的持续双向同步。
- 不在 V1 支持逐个选择 Session。会话树与全局 Agent Runtime 协调数据库存在跨会话引用，拆分迁移会制造不可证明的悬空关系。
- 不保证无法解密、与旧机器硬件绑定或协议已废弃的密钥继续可用；此类项目必须保留非敏感元数据并明确要求用户重新认证。
- 不把未经识别的旧文件“先复制过去再说”。

## 3. 核心原则

### 3.1 按所有者迁移，不按目录迁移

每类数据由其当前产品领域所有者定义：

- 如何识别旧版本；
- 哪些文件和表属于该领域；
- 如何解析和校验；
- 如何转换为当前模型；
- 如何检测重复和冲突；
- 如何提交及验证；
- 哪些数据必须作为一个原子子域处理。

通用迁移引擎只负责编排、锁、暂存、日志、恢复和报告，不猜测领域数据语义。

### 3.2 离线写入

只要迁移范围包含主应用常规会打开的数据，迁移必须在 OpenBitFun 主应用退出后执行。Data Migrator 需要取得目标迁移锁，并确认没有 OpenBitFun、BitFun 或 Agent Runtime 进程仍在写入选中的源和目标存储。

默认只请求优雅退出，不主动强杀进程。若仍有进程占用，迁移器应展示具体进程和“重试 / 取消”操作；强制结束进程只能是明确、二次确认的用户操作。

### 3.3 源只读，目标可恢复

- 不修改旧数据库，不在旧目录创建 marker，不对旧 SQLite 执行写入式 checkpoint。
- 读取 SQLite 时使用 SQLite 备份/一致性快照能力，把主数据库及 WAL 中的有效内容合并到暂存数据库；不能只复制 `.sqlite` 文件。
- 在提交前对目标端将被修改的数据创建可恢复备份或回滚记录。
- 暂存目录与目标数据放在同一卷，确保文件替换可以使用原子 rename。
- 每个领域提交后立即做读取验证，再写入提交标记。

### 3.4 可重复、可恢复、可解释

一次迁移由稳定的来源指纹、所选范围、转换器版本和计划哈希标识。每个源实体应有来源 ID/哈希到目标 ID 的映射。同一计划重试时：

- 已成功且校验通过的领域直接跳过；
- 暂存未提交的领域可以重新生成；
- 提交状态不确定的领域先按结果校验，不盲目再写一次；
- 失败不抹掉已成功领域；
- 最终报告与界面状态一致。

## 4. 数据根目录与发现

### 4.1 Windows 示例

旧版 Windows 数据根目录的典型位置是：

`%APPDATA%\bitfun`

例如当前用户可能是：

`C:\Users\wsp\AppData\Roaming\bitfun`

实现中不得硬编码用户名或这个绝对路径。源路径应由平台目录解析器和旧产品标识共同解析；目标路径必须通过 OpenBitFun 的 AppPaths/PathManager 获取，不能在各迁移适配器内自行拼接。

还需要按领域发现用户主目录下的数据，例如：

`%USERPROFILE%\.bitfun\memories`

### 4.2 来源发现条件

轻量检测器只读取少量稳定标记，输出：

- 是否存在旧 BitFun 根目录；
- 是否存在至少一个受支持领域的数据；
- 可识别的旧产品版本/模式版本；
- 路径可读性；
- 粗略数据量；
- 是否疑似仍有 BitFun 进程运行；
- 是否已经对同一来源完成过迁移。

目录存在本身不等于可迁移。空目录、只有缓存/日志的目录以及不受支持的未来格式不能触发“可迁移”结论。

如果发现多个来源目录，首次引导默认选择当前用户最可信的来源，并允许用户在迁移器内查看和改选；不得自动合并多个来源。

### 4.3 “第一次打开”的定义

“第一次打开”由 OpenBitFun 自己的数据目录中的 onboarding 状态判断，而不是通过“目标目录是否为空”判断。至少记录：

- 来源指纹；
- 检测时间；
- 用户选择：立即、稍后、不再提醒；
- 最近一次提示版本；
- 对应迁移运行 ID。

“不再提醒”只关闭自动引导，不移除设置页常驻入口，也不把旧数据标记为已迁移。

## 5. V1 迁移分组

用户界面只展示五个高层组；组内的依赖和原子子域由系统管理，不能提供会破坏一致性的任意文件勾选。

### 5.1 设置与服务凭据

包含：

- 用户设置、功能开关、主题/外观、语言、模型与服务配置；
- 可移植的提供商账户配置和服务端点；
- 由凭据引用关联的 API Key、Token 或密码；
- 与当前版本仍有明确语义映射的实验选项。

规则：

- 只迁移当前设置所有者列入映射表的 key，并通过版本化转换器写入当前 schema。
- 未知或已废弃 key 记录为“未支持”，不能原样注入当前配置。
- 目标端已显式设置的值优先；只有目标端仍是缺省值或不存在时，才接受源值。
- 凭据不通过日志、命令行参数、迁移报告或普通 JSON 暂存。
- 凭据应通过旧凭据所有者/安全存储读取，并写入当前 Vault/系统凭据库。无法解密时迁移非敏感配置并标记“需要重新登录或重新填写凭据”。
- 机器绑定或用户 SID 绑定的秘密不能通过简单文件复制绕过安全边界。

### 5.2 Agents、Skills 与 MiniApps

#### Agents

来源：

`%APPDATA%\bitfun\agents`

迁移所有正式的用户 Agent 定义及其由 Agent 所有者声明的附件。当前安装包提供的内置 Agent 不从旧目录覆盖。

处理规则：

- 先解析 manifest/schema，再复制允许的资源；
- 保留稳定来源 ID，并在冲突时建立来源 ID 到新目标 ID 的映射；
- 引用 Skill、MiniApp 或模型配置的字段必须在相关领域映射完成后重写；
- 无法解析的 Agent 保留在隔离报告中，不注册到产品运行时。

#### Skills

来源：

`%APPDATA%\bitfun\skills`

包含：

- `skills` 下除根级 `.system` 目录外的用户 Skill；
- Skill manifest 声明的脚本、模板和资源。

排除：

`%APPDATA%\bitfun\skills\.system`

`.system` 由 OpenBitFun 当前版本重新生成。实现应以 `OPENBITFUN_SYSTEM_SKILL_DIR` 所代表的稳定产品事实识别系统目录，不在多个位置重复硬编码字符串。

迁移期间不执行 Skill 脚本、不加载插件、不运行 hook。路径校验必须拒绝越界路径、junction/reparse point 和循环链接。

#### MiniApps

来源：

`%APPDATA%\bitfun\data\miniapps`

包含：

- 用户安装或创建的非内置 MiniApp；
- MiniApp 所有者明确声明可迁移的用户存储和自定义数据。

排除：

- 当前或旧版内置 MiniApp 的安装源；
- 可由安装包重新生成的 bundle；
- 下载缓存、构建缓存和临时运行状态。

内置身份应结合内置 ID 清单、`BUILTIN_APPS` 与 `.builtin-manifest.json` 判断，不能只根据目录名猜测。若内置 MiniApp 曾产生用户数据，应由 MiniApp 数据适配器把“内置代码”和“用户数据”分开：代码不迁移，兼容的用户数据可以迁移。

### 5.3 工作区、会话与 Agent 任务状态

这是一个用户可选组，但在内部至少包含“工作区/会话存储”和“Agent Runtime 协调数据库”两个有依赖的原子子域。

包含：

- 工作区注册信息、最近使用记录和会话关联；
- 完整 Session 树、消息、附件引用、运行记录及所有者声明的索引；
- `runtime-events` 等恢复旧会话所必需的持久事件；
- `%APPDATA%\bitfun\data\agent-runtime\coordination.sqlite`。

`coordination.sqlite` 是会话数据的一部分，而不是可选缓存。旧 Session 中的 Subagent、任务关系、投递/协调状态依赖它。V1 中只要用户选择“工作区、会话与 Agent 任务状态”，迁移计划就必须自动包含该数据库，界面可以解释依赖但不能取消勾选。

排除 Agent Runtime 的短期进程状态：

- `ownership`；
- `ipc-v*`；
- PID、lock、socket、端口发现文件；
- 运行实例租约、心跳和可重新生成的 discovery 状态；
- SQLite `-shm` 文件；
- 其他只在旧进程存活期间有效的数据。

SQLite `-wal` 不是独立目标文件，但如果存在，必须作为一致性读取输入，由 SQLite 备份流程合并进暂存数据库。源端保持只读，最终目标数据库不携带旧 `-wal`/`-shm`。

会话迁移还必须满足：

- V1 不允许逐个选择 Session。
- 先建立工作区、Session、Turn、Subagent/Task 等实体的完整 ID 映射，再写交叉引用。
- 无法解析的新旧状态不能通过删除记录“修复”；应保留源记录、降级该会话并给出明确状态。
- 工作区本地路径只作为引用迁移，不复制目录内容。路径不存在时仍保留记录并标记“需要重新定位”。
- 远程工作区路径按远端 POSIX 路径处理，不能用 Windows `std::path` 语义重写。
- 若协调数据库的 schema 不受支持或一致性校验失败，整个会话组不得标记成功。

### 5.4 记忆

来源至少包括：

`%APPDATA%\bitfun\data\memories\memories.sqlite`

`%USERPROFILE%\.bitfun\memories`

包含：

- 当前 Memory 所有者能够解析的结构化记忆；
- 与记忆关联、且 schema 明确支持的索引和附件；
- 文件型记忆目录中非临时、非生成物的数据。

规则：

- 数据库通过一致性备份读取，不复制活动中的 SQLite 文件组合。
- 迁移器不得在迁移阶段触发 embedding、总结、压缩或其他模型调用。
- 可重新生成的全文/向量索引默认不迁移；导入事实数据后由当前版本按需重建。
- 重复记忆优先使用稳定 ID 和规范化内容哈希去重。
- 同 ID 不同内容时保留目标记录，并为源记录分配新 ID 或进入显式冲突列表，不能静默覆盖。
- 文件型记忆只允许在声明的根目录内读取，拒绝路径穿越和链接越界。

### 5.5 远程连接与设备

包含两类需要分别原子处理的子域。

#### Remote Connect、设备、账户和机器人

可迁移项包括：

- 设备显示名称、用户可识别的连接资料；
- 已授权设备/控制端的非敏感元数据；
- Remote Connect、IM Bot 等仍受支持的配置；
- 可由当前安全存储接收的认证资料。

默认排除：

- 在线状态、心跳、relay lease、临时会话 token；
- 进程和 Socket 发现信息；
- 已过期的一次性配对码；
- 当前协议不再接受的缓存。

设备 ID 和加密身份不能一概复制。领域适配器必须把数据分成：

1. 可移植资料：直接转换；
2. 可安全迁移的秘密：通过 Vault/系统安全存储转写；
3. 机器绑定身份：生成当前身份并提示重新配对；
4. 不受支持记录：保留可读报告，不注册为有效连接。

不得为了“看起来迁移成功”而复用可能导致两个设备共享同一活跃身份的密钥。

#### SSH 连接与远程工作区凭据

作为独立原子子域处理：

- SSH 主机、跳板链、端口和用户名配置；
- 远程工作区登记；
- `known_hosts`/主机指纹；
- 密码、私钥口令等 Vault 引用。

目标端已有连接资料优先。`known_hosts` 冲突必须展示指纹差异，不能自动接受新的主机身份。私钥文件只迁移引用；只有产品已有明确“导入密钥”流程且用户选择后，才复制密钥，并保留严格文件权限。

## 6. 用户流程

### 6.1 首次启动引导

首次启动引导由 Data Migrator 的 onboarding 模式承载，避免为了显示引导而先初始化主应用 WebView、配置、日志、插件或会话服务。启动顺序应是：

1. 解析当前平台的 OpenBitFun 数据根目录。
2. 读取最小迁移 onboarding 状态。
3. 运行只读的 legacy probe。
4. 如果没有受支持的旧数据，继续正常启动。
5. 如果用户已选择不再提醒，继续正常启动。
6. 如果存在可迁移数据，生成 onboarding 请求，以最小平台进程启动能力启动 Data Migrator，然后直接结束本次主应用启动。
7. Data Migrator 展示首次启动引导；用户选择“立即迁移”时在同一迁移器进程中继续扫描、确认和迁移。
8. 用户选择“稍后提醒”或“不再提醒”时，迁移器原子写入独立 onboarding 状态，再携带一次性已处理 run ID 重启主应用。
9. 重启后的主应用校验 onboarding 状态/run ID，通过检测后才创建日志和打开正常产品存储，避免重复拉起迁移器。

这段检测必须位于 `src/apps/desktop/src/lib.rs` 中 `LogConfig::new` 和 `initialize_global_config` 之前。检测器自身只能使用独立、极小且不会打开产品数据库的日志/诊断方式；启动迁移器也只能依赖最小平台进程能力，并在 Windows 明确隐藏控制台窗口。

引导页应解释：

- 旧数据不会被删除；
- 迁移期间 OpenBitFun 需要关闭；
- 可以选择迁移范围；
- 稍后可以从设置再次进入；
- 迁移器会在结束后重新打开 OpenBitFun。

首次提示不应直接开始迁移，也不应把“关闭窗口”解释为用户同意。关闭 onboarding 窗口默认等价于“稍后提醒”，并重新打开主应用。

### 6.2 设置中的常驻入口

在 `src/web-ui/src/app/scenes/settings/settingsRegistry.ts` 对应的“数据与维护”分类新增“数据迁移”。

入口至少展示：

- 是否检测到旧 BitFun 数据；
- 上次扫描时间和来源；
- 最近一次迁移结果；
- 已成功/失败/跳过的领域；
- “扫描旧数据”“开始迁移”“查看报告”“重试失败项”；
- 已选择不再提醒时恢复首次引导提醒的选项。

从设置开始迁移时，如果存在正在运行的 Agent、终端任务、文件写入或未保存操作，主应用应先展示关闭影响。迁移请求确认后，仍走与首次启动相同的独立迁移器流程，Web UI 不直接读写用户数据。

### 6.3 Data Migrator 向导

建议步骤：

1. 欢迎与来源选择。
2. 只读扫描。
3. 范围选择和依赖说明。
4. 冲突、不可迁移项、磁盘空间和进程预检。
5. 用户最终确认。
6. 关闭/等待相关应用。
7. 迁移进度。
8. 结果与报告。
9. 重新启动 OpenBitFun。

范围选择中，“工作区、会话与 Agent 任务状态”是一个整体选项；`coordination.sqlite` 作为强制依赖显示在详情中。

进度必须以领域、阶段和计数表达，不能用无法验证的伪百分比。取消只允许发生在安全边界：

- 扫描和计划阶段可以立即取消；
- 暂存阶段可在当前可中断步骤后取消；
- 单个领域进入提交临界区后，必须完成提交或回滚到已知状态再响应取消。

## 7. 独立迁移器架构

### 7.1 为什么是独立应用

迁移器应是单独的可执行程序和应用包，而不是主应用中的隐藏窗口，原因包括：

- 可以在主应用所有数据库、文件 watcher、日志和运行时完全关闭后工作；
- 不共享主应用 WebView profile，避免迁移浏览器状态时占用自己；
- 能独立获取目标数据锁；
- 崩溃恢复和升级边界清晰；
- 可由安装器、首次启动或设置入口统一启动；
- 可单独进行代码签名、包完整性校验和进程身份校验。

安装器可以提供“安装完成后迁移旧 BitFun 数据”的入口，但只负责启动同一个 Data Migrator，不能复制一套迁移实现。

### 7.2 进程交接协议

主应用与迁移器之间使用版本化请求文件，不把密钥或大段配置放在命令行。协议支持两种入口：首次启动的 `onboarding` 模式由迁移器收集选择；设置页的 `execute` 模式携带用户已确认的初始范围，但迁移器仍重新扫描并做最终确认。请求至少包含：

- 请求协议版本；
- 随机 run ID 和一次性 nonce；
- 调用来源：首次启动、设置或安装器；
- 用户选择的来源引用与领域；
- 主应用进程 ID；
- 安装通道/产品身份；
- 请求创建时间和过期时间。

请求文件保存在当前用户专属、ACL 受限的迁移目录。迁移器必须校验：

- 请求属于当前 OS 用户；
- 请求未过期且 nonce 未消费；
- 来源和目标由可信路径解析器重新解析，而不是直接信任任意绝对路径；
- 发起程序和待重启程序属于受信任的 OpenBitFun 安装；
- 协议版本受支持。

交接流程：

1. 主应用落盘并 fsync 请求。
2. 主应用使用共享进程管理器启动迁移器；Windows 子进程不得闪现控制台窗口。
3. 迁移器读取请求并开始等待。
4. 主应用停止接收新任务，优雅关闭 Agent Runtime、数据库、watcher 和窗口。
5. 迁移器确认相关进程退出并获取独占迁移锁。
6. 执行迁移。
7. 写入最终结果和 onboarding 状态。
8. 通过可信安装解析器定位主程序并重新启动。
9. 新主应用读取 run ID，展示一次性迁移结果摘要。

不能接受请求文件传入的任意 executable path 并执行，避免形成命令执行入口。

### 7.3 运行拓扑

建议新增一个最小交付 profile：

`src/apps/data-migrator`

它只链接：

- 稳定迁移 contracts；
- 通用迁移编排引擎；
- V1 五类数据的离线适配器；
- 平台路径、安全存储、文件系统和进程服务；
- 迁移器自身的轻量 UI。

它不应链接完整 `product-full`、启动 Agent Runtime、加载用户插件或初始化正常产品会话服务。

如果迁移器采用 Tauri/WebView UI，应使用独立 bundle ID、独立 WebView data directory 和最小命令白名单。UI 仍只调用 typed adapter，不直接使用 Tauri 文件系统 API。

## 8. 分层与代码所有权

以下是建议的物理落点；实现时可以根据现有 crate 粒度调整文件名，但不能改变所有权方向。

| 层 | 建议落点 | 责任 |
|---|---|---|
| 稳定合同与产品领域 | `src/crates/contracts/product-domains/src/legacy_migration/` | DTO、领域 ID、状态机、报告、兼容版本、远程姿态 |
| 通用服务 | `src/crates/services/services-integrations/src/legacy_migration/` 或最邻近的专用 service crate | 发现、锁、暂存、原子文件操作、SQLite 一致性快照、日志、恢复 |
| Product Assembly | `src/crates/assembly` 下的迁移装配模块 | 选择当前产品支持的领域适配器和 migrator delivery profile |
| 领域所有者 | 设置、Session、Memory、Remote、MiniApp 等现有 owner 附近 | 旧格式 reader、转换器、去重/冲突和当前格式 writer |
| Desktop | `src/apps/desktop` | 启动前 probe、引导、进程交接、重启后的结果提示 |
| Data Migrator | `src/apps/data-migrator` | 独立 UI、调用编排器、展示进度和报告 |
| Web UI | 设置注册表及数据迁移 feature | 展示状态和发起本机交接，不读写旧文件 |
| Installer | `OpenBitFun-Installer` | 安装和可选启动迁移器，不拥有迁移逻辑 |

当前相关事实应复用而不是复制：

- 系统 Skill 目录：`src/crates/execution/agent-runtime/src/skills/roots.rs` 中的 `OPENBITFUN_SYSTEM_SKILL_DIR`。
- 内置 MiniApp 身份：`src/crates/contracts/product-domains/src/miniapp/builtin.rs` 中的 `BUILTIN_APPS` 及 `.builtin-manifest.json`。
- Agent Runtime 协调数据库路径：`src/crates/assembly/core/src/infrastructure/app_paths/path_manager.rs` 中的 `data/agent-runtime/coordination.sqlite`。
- 设置分类注册：`src/web-ui/src/app/scenes/settings/settingsRegistry.ts`。
- 远程命令姿态：`src/crates/contracts/product-domains/src/remote_surface/table.rs`。

### 8.1 稳定合同

核心合同建议包含：

- `LegacySourceDescriptor`：产品、版本、平台、解析后的根、来源指纹；
- `MigrationDomainId`：五个用户组及内部原子子域；
- `ScanFinding`：数量、大小、版本、风险、依赖和不可迁移原因；
- `MigrationSelection`：用户选择和系统自动补入的依赖；
- `MigrationPlan`：有序步骤、预计写入、冲突策略和计划哈希；
- `MigrationProgressEvent`：领域、阶段、已处理/总数、当前安全取消点；
- `MigrationConflict`：稳定代码、来源摘要、目标摘要和处理结果；
- `MigrationRunReport`：最终状态、领域结果、警告、重认证要求；
- `MigratorProtocolVersion`：主应用、安装器与迁移器的交接能力协商。

持久化 shape 只能追加带默认值的字段。旧迁移器和新主应用之间必须按能力协商，不能以安装包版本相同为前提。

### 8.2 领域适配器

每个领域实现统一的离线生命周期：

1. `probe`：确认是否存在及格式版本；
2. `scan`：只读枚举和基本校验；
3. `plan`：转换、依赖、冲突和空间估算；
4. `stage`：写入隔离暂存区；
5. `validate_stage`：按当前 owner 读取暂存结果；
6. `commit`：事务或原子替换；
7. `validate_commit`：从目标读取验证；
8. `rollback`：仅处理该领域尚未确认成功的目标修改；
9. `summarize`：输出去敏报告。

禁止用“复制未知目录”的默认适配器兜底。

### 8.3 桌面命令

主应用侧命令保持窄接口并使用结构化 `request`，例如：

- `get_legacy_migration_status`；
- `scan_legacy_migration`；
- `prepare_legacy_migration`；
- `get_legacy_migration_report`；
- `set_legacy_migration_prompt_preference`。

真正的 `execute` 只存在于 Data Migrator 本地进程，不暴露给普通 Web UI 或远程调用面。

这些命令必须登记到 Product Operation Registry：

- remote workspace：本机产品数据操作，不路由到远程工作区；
- remote control：显式 unsupported；
- Peer Device：`ControllerLocal`，不能默认代理到 peer；
- Detached Dispatch：不支持。

## 9. 迁移日志与报告

迁移元数据建议位于目标数据根的专用目录，例如逻辑上的：

`data/migrations/bitfun-to-openbitfun/`

具体路径仍由统一路径管理器提供。目录至少包含：

- `onboarding.json`：提示偏好和最近来源；
- `runs/<run-id>/request.json`：去敏后的请求；
- `runs/<run-id>/plan.json`：不可变计划及哈希；
- `runs/<run-id>/journal.jsonl`：追加式状态事件；
- `runs/<run-id>/report.json`：机器可读最终报告；
- `runs/<run-id>/report.html` 或 UI 可渲染摘要；
- `runs/<run-id>/stage/`：未提交暂存；
- `runs/<run-id>/backup/`：本次修改涉及的目标备份；
- `lock`：本机独占迁移锁。

日志状态至少包括：

- `discovered`；
- `scanned`；
- `planned`；
- `waiting_for_processes`；
- `staging`；
- `validating_stage`；
- `committing`；
- `validating_commit`；
- `completed`；
- `completed_with_warnings`；
- `cancelled`；
- `failed_recoverable`；
- `failed_manual_action_required`。

每个领域独立记录 `not_started / staged / committed / verified / failed / skipped`。Journal 使用追加和 fsync；汇总文件通过临时文件加原子 rename 更新。

报告中不能包含凭据、完整消息正文、完整记忆正文或私钥内容。文件路径默认缩短到逻辑根下的相对路径；日志遵循仓库规则，只使用英文且不含 emoji。

## 10. 执行状态机

### 10.1 Discover

- 解析受支持源；
- 读取最小版本标记；
- 计算不含敏感内容的来源指纹；
- 拒绝跨用户、权限异常或来源与目标相同的配置。

### 10.2 Scan

- 所有领域只读扫描；
- 识别 schema 版本、实体数量、逻辑大小和风险；
- 检查源数据库完整性；
- 不生成目标记录，不执行用户代码。

### 10.3 Plan

- 展开用户选择的强制依赖；
- 为每个领域选择明确的 `source_version -> current_version` 转换路径；
- 计算目标冲突、ID 映射策略、空间需求和执行顺序；
- 生成不可变计划哈希；
- 向用户展示后再次确认。

若用户选择会话组，计划必须包含 `coordination.sqlite`；缺失或不兼容时在确认前显示阻断原因。

### 10.4 Acquire

- 等待 BitFun、OpenBitFun 及相关 Agent Runtime 退出；
- 获取源读取保护和目标独占迁移锁；
- 再次确认来源指纹未变化；
- 检查可用空间，至少覆盖暂存、备份和安全余量；
- 记录恢复点。

### 10.5 Stage

- 按拓扑顺序执行领域 reader 和转换器；
- 先建立跨实体 ID 映射，再写引用；
- SQLite 使用一致性备份到 stage；
- 文件内容复制到 stage 后校验大小和哈希；
- 用户扩展内容保持惰性，不加载、不执行。

建议依赖顺序：

1. 设置的非敏感结构和凭据引用；
2. Skills、MiniApps、Agents；
3. 工作区、会话、runtime-events 和协调数据库；
4. 记忆；
5. Remote Connect/设备和 SSH 子域；
6. 最终交叉引用修复。

实际提交顺序由 plan 固化，恢复时不得重新推测。

### 10.6 Validate Stage

- 用当前版本 owner 的 reader 打开暂存结果；
- 执行 schema、外键、引用闭包和数据库完整性检查；
- 确认没有源路径越界、未声明文件或秘密泄漏到普通暂存；
- 会话组必须证明 Session/Subagent/Task 与协调数据库的引用闭包。

### 10.7 Commit

每个原子子域单独提交：

- 数据库使用 owner 事务或受支持的 import API；
- 独占目录可使用同卷原子 rename；
- 与目标已有数据合并时必须走 owner writer，不能替换整个目标目录；
- 写入提交意图、执行提交、读取验证、写入已验证标记；
- 进入提交临界区后不能留下“可能写了一半但无记录”的状态。

全局使用 saga：某领域提交后，后续领域失败时保留已经验证的数据，报告“部分完成”，并允许只重试失败领域。除非 owner 能证明补偿操作无损，否则不自动回滚已成功领域。

### 10.8 Finalize

- 清理未需要保留的临时 stage；
- 保留报告、必要恢复信息和用户选择；
- 释放锁；
- 重新启动可信 OpenBitFun 主程序；
- 主应用展示一次性摘要；
- 旧 BitFun 数据保持不变。

## 11. 合并、冲突与 ID 策略

通用默认规则：

| 情况 | 默认处理 |
|---|---|
| 目标不存在，来源有效 | 导入 |
| 稳定 ID 与内容哈希均相同 | 视为重复，跳过 |
| 同名但 ID 不同 | 两者保留；必要时给来源显示名加“来自 BitFun” |
| 同 ID、内容不同且可安全重映射 | 目标保留，来源生成新 ID，重写全部来源引用 |
| 同 ID、内容不同且不可安全重映射 | 不写入该实体，记录显式冲突 |
| 目标已有显式设置/凭据 | 目标优先 |
| 来源格式未知或校验失败 | 保留源，跳过目标写入，报告原因 |

领域补充规则：

- Settings：按 key 级别合并，只导入白名单及有明确转换器的项。
- Agents/Skills/MiniApps：按稳定扩展 ID 和内容版本判断；不允许旧内置内容覆盖当前内置内容。
- Sessions：ID 重映射必须覆盖工作区、Session、Turn、附件、runtime event、Subagent/Task 和协调库引用，不能只改顶层 Session ID。
- Memory：内容哈希用于辅助去重，但不能把内容相似当作完全相同。
- Remote：不能让两个设备共享一个应当唯一的活跃设备身份；安全优先于无提示连续登录。
- SSH：host key 冲突要求用户确认，不能自动覆盖。

V1 不提供逐条通用冲突编辑器。可自动安全处理的按规则处理；需要用户判断的项目在计划或报告中分领域展示，并提供“跳过该项/该领域”的安全选择。

## 12. 安全与隐私

迁移器处理的是不可信的本地输入，必须按导入器而不是备份复制工具进行防护：

- 所有文件操作限定在解析后的允许根目录内。
- Windows 检查 reparse point/junction；其他平台检查 symlink，默认不跟随越界链接。
- manifest 中的绝对路径和 `..` 越界一律拒绝。
- 对文件数量、单文件大小、总大小、JSON 深度和数据库页数设置合理的资源上限，并把超限报告给用户。
- SQLite 先做只读完整性和 schema 检查；不执行来源提供的扩展、触发器脚本或任意 SQL 文本。
- 不执行 Agent、Skill、MiniApp、hook 或插件代码。
- 凭据只在最短生命周期的受保护内存和系统安全存储通道中出现。
- 日志、遥测和崩溃报告不能包含秘密或用户正文。
- 迁移请求和报告使用当前用户 ACL；不允许跨 OS 用户迁移。
- 迁移器和主程序互相验证产品身份、签名/安装位置和协议版本。
- 默认不要求管理员权限，也不通过提权绕过 ACL。
- 备份和暂存的保留策略必须可见；包含敏感材料的临时文件在失败后也要按策略安全清理。
- 导入的可执行内容在首次实际启用时仍走当前产品的信任、权限和安全审查。

## 13. 远程场景边界

### 13.1 Remote Workspace

迁移器运行在保存本机旧 BitFun 产品数据的设备上。迁移工作区登记和远端 POSIX 路径，但不连接 SSH 主机复制项目内容，也不把本地路径传给远端执行。

如果某条工作区记录需要当前版本不具备的远程能力，应保留记录并显示 unsupported/needs-repair，不能静默改成本地工作区。

### 13.2 Remote Control

手机网页或 IM Bot 不能远程启动会关闭桌面应用的迁移流程。对应命令必须返回明确的“请在保存旧数据的桌面设备上操作”，不能空响应或在控制端本地执行。

迁移完成后，普通的结果通知可以通过已有远程事件机制展示，但不能把报告中的敏感路径或内容推送给远程控制端。

### 13.3 Peer Device Mode

迁移命令为 `ControllerLocal`。每台设备只迁移自己的旧数据，不能由控制器把本机来源路径代理给 peer，也不能默认在 peer 上执行。

若未来需要迁移 peer 上的数据，必须在 peer 设备本地发起并完成独立的用户确认和进程交接。

### 13.4 Detached Dispatch

V1 不支持在 Detached Dispatch 作业中迁移。迁移需要本机交互、关闭产品进程和独占用户数据锁，不属于无交互 durable job。

未来如提供 headless CLI，只能作为单独能力设计，具备明确源、目标、锁、报告和确认语义，不能复用调度作业做隐式迁移。

## 14. 打包、升级与重启

- Data Migrator 与 OpenBitFun 使用同一发布通道，但拥有独立应用身份和可执行文件。
- 安装器验证迁移器存在且签名有效；缺失时不能退化为安装器自行复制文件。
- 主应用和迁移器通过协议能力协商。新主应用遇到旧迁移器时应要求修复安装或更新，不能发送其不理解的请求。
- updater 在迁移期间不得替换主应用或迁移器；迁移锁和更新锁要有确定的获取顺序。
- 迁移器运行时不自更新。更新应在迁移开始前完成，或迁移结束后由主应用处理。
- 重启路径从受信任安装注册信息解析，并继承正确安装通道；不执行请求中的任意路径。
- 重启失败不影响已提交数据。结果页应提供“打开 OpenBitFun”和安装修复说明。
- 卸载 BitFun 与清理旧数据是迁移以外的显式用户动作。

Windows 是首个必须完整支持的平台。macOS/Linux 如果存在历史 BitFun 版本，应使用同一领域协议和平台路径解析器，不能复制 Windows 绝对路径逻辑。

## 15. 测试与验证方案

### 15.1 固定样本

为每个受支持旧版本建立去敏、最小但关系完整的 fixture：

- 空安装；
- 只有一个领域；
- 五个领域齐全；
- 含 Session/Subagent/Task/coordination 交叉引用；
- SQLite 存在未 checkpoint 的 WAL；
- 目标已有数据和 ID 冲突；
- 无效 JSON、未知 schema、损坏 SQLite；
- 路径穿越、junction/symlink、超大文件；
- 凭据可迁移与不可解密；
- 远程设备身份不可移植；
- 中英文及特殊字符路径。

fixture 必须由 owner reader 生成或验证，避免测试只证明迁移器和自造假格式彼此一致。

### 15.2 单元与契约测试

- 版本探测和转换器选择；
- 各领域 include/exclude 规则；
- `.system` Skills 排除；
- 内置 MiniApp 与用户数据拆分；
- 会话选择自动包含 `coordination.sqlite`；
- ID 映射和引用闭包；
- 目标优先、重复跳过、冲突可见；
- 报告去敏；
- 交接协议版本和过期 nonce；
- Product Operation Registry 的本机/远程姿态。

### 15.3 集成测试

- 在隔离目录运行完整 scan → plan → stage → commit → verify；
- 对同一来源连续迁移两次，第二次不产生重复；
- 源目录迁移前后哈希一致；
- 目标 owner 能正常读取迁移结果；
- SQLite WAL 内容进入目标且 `-shm` 不进入目标；
- 会话及旧 Subagent 在导入后可被当前 owner 解析并恢复关系；
- 每个状态转换点注入崩溃，重启后能继续或给出确定状态；
- 某领域失败后，其他已验证领域保持可用，重试只处理失败项；
- 磁盘不足、权限不足和占用冲突不产生半提交。

### 15.4 进程与打包测试

- 主应用生成请求后优雅退出；
- migrator 等待所有 writer 并获取独占锁；
- 迁移结束后从可信安装位置重启；
- 请求路径注入不能启动任意程序；
- 安装、升级、修复安装和卸载组合；
- Windows 下迁移器和子进程不闪现控制台；
- 迁移期间 updater 不与 migrator 互相覆盖；
- 代码签名和包内容闭包。

### 15.5 手工验证

发布前至少覆盖：

- 全新安装首次引导；
- “稍后提醒”“不再提醒”和设置常驻入口；
- 五类范围选择及强制依赖说明；
- 关闭主应用、迁移进度、取消安全点；
- 完成、有警告、可恢复失败和重启失败；
- 屏幕阅读器、键盘操作、本地化和长路径展示。

手工 UI 验证需要按仓库规则单独显式执行；本文档落地本身不启动开发服务器，也不构成运行时/UI 验证。

## 16. 分阶段实施

### Phase 0：格式冻结与样本

- 为五个领域指定 owner。
- 从受支持 BitFun 版本生成去敏 fixture。
- 明确每个旧 schema 的最低支持版本。
- 固化 include/exclude 表、数据依赖和来源指纹算法。
- 对 `coordination.sqlite` 的表关系及 Session 引用做闭包审计。

交付：版本支持矩阵、fixture、领域责任表。

### Phase 1：合同与无 UI 引擎

- 新增 migration contracts、状态机、journal、报告和锁。
- 实现平台路径发现、只读 SQLite snapshot、暂存和领域原子提交工具。
- 建立 crash injection 和 idempotency 测试框架。
- 实现 dry-run CLI/test harness，仅用于开发和自动化验证。

交付：可在隔离目录完成扫描、计划和无损演练的核心。

### Phase 2：五类领域适配器

建议按依赖顺序实现：

1. 设置和凭据；
2. Skills、MiniApps、Agents；
3. 工作区、Session、runtime-events、`coordination.sqlite`；
4. Memory；
5. Remote Connect、设备和 SSH。

每个适配器必须同时交付旧格式 fixture、转换测试、冲突测试、当前 owner 读取验证和去敏报告。

### Phase 3：独立 Data Migrator

- 增加最小 migrator delivery profile 和 UI。
- 实现进程交接、独占锁、恢复、取消和可信重启。
- 建立独立 bundle ID、WebView profile、签名和安装布局。
- 完成 Windows 进程/锁/打包测试。

### Phase 4：产品入口

- 将轻量 probe 放到 Desktop 正常存储初始化之前。
- 增加首次启动引导。
- 在“数据与维护”增加常驻入口和历史报告。
- 将命令加入 Product Operation Registry，并验证 Remote/Peer/Dispatch 显式边界。
- 让安装器仅启动同一个迁移器。

### Phase 5：发布与观测

- 小范围发布，观测仅包含去敏结果码、领域状态、耗时和失败阶段。
- 建立不含用户正文的失败诊断导出。
- 根据真实旧版本补充转换器和 fixture。
- 达到验收门槛后扩大推送，不以自动删除旧数据作为收尾。

## 17. V1 验收标准

V1 只有同时满足以下条件才算完成：

- Windows 能正确发现 `%APPDATA%\bitfun`，且实现不硬编码具体用户名。
- 首次启动检测发生在正常日志和全局配置初始化之前。
- 用户可以立即迁移、稍后提醒或不再提醒。
- 设置“数据与维护”中始终存在迁移入口。
- 迁移在独立、签名的 Data Migrator 中执行，主应用在写入前已退出。
- 五个用户组均有明确 owner、旧版本 reader、转换器、验证器和报告。
- `agents` 全部正式用户内容、`skills` 中非 `.system` 内容以及非内置 MiniApps 按规则迁移。
- 会话组强制包含 `data/agent-runtime/coordination.sqlite`，并验证 Session/Subagent/Task 引用闭包。
- `ownership`、`ipc-v*`、PID/lock/socket/discovery 和 SQLite `-shm` 不进入目标。
- SQLite WAL 中已提交内容不会因只复制主文件而丢失。
- 目标已有数据默认不被覆盖，所有冲突可见。
- 迁移可重试、可恢复且重复执行不重复导入。
- 旧 BitFun 数据在成功、失败和取消后均保持不变。
- 凭据和用户正文不进入日志、命令行、遥测或普通报告。
- 远程调用返回明确 unsupported，Peer 命令为 `ControllerLocal`，无静默本地回退。
- 完成后能可信重启 OpenBitFun；重启失败不回滚已验证数据。
- fixture、领域集成、崩溃恢复、进程交接和打包测试通过。
- 每个受支持旧版本都有至少一个包含会话与 `coordination.sqlite` 的端到端样本。

## 18. 已确定的 V1 决策

以下事项不再留给实现阶段临时决定：

1. 使用独立 Data Migrator，不在运行中的主应用内直接迁移。
2. 安装器只启动迁移器，不拥有第二套迁移逻辑。
3. 旧源数据只读且永不自动删除。
4. 迁移按领域 owner 解析，不递归复制整个用户目录。
5. 用户界面采用五个高层组。
6. `agents` 全部正式用户 Agent 纳入。
7. `skills` 排除 `.system`，其他用户 Skills 纳入。
8. `data/miniapps` 只迁移非内置 MiniApp；内置代码重建，用户数据另行适配。
9. `data/agent-runtime/coordination.sqlite` 是会话组的强制组成部分。
10. V1 不支持逐个 Session 选择。
11. 目标端已有数据默认优先，冲突显式报告。
12. 领域原子提交加全局 saga，不虚构跨领域全局事务。
13. 首次检测位于常规产品存储打开之前。
14. “数据与维护”永久保留迁移入口。
15. 迁移只在数据所在本机执行，远程场景明确降级。
16. 用户扩展内容在迁移期间保持惰性，绝不执行。

## 19. 后续可扩展项

不阻塞 V1，但合同应允许后续增加：

- 受支持旧版本的增量转换器；
- macOS/Linux 历史数据路径；
- 经明确设计的 headless 导入工具；
- 用户导出的离线迁移包；
- 更细的领域内预览和冲突选择；
- 完成后的显式旧数据清理向导；
- 对不可移植设备身份的辅助重新配对流程。

这些扩展仍必须遵循源只读、owner-aware、目标优先、可恢复和远程显式降级的基本原则。
