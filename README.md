# Assemble

Assemble 是一个全新的、自带服务端的项目，用来完全替换 jarvis：服务端与桌面端都在这个仓库里，Rust 负责业务规则，Tauri 2 只做窗口与系统集成。优先级是文件传输与日程规划——文件传输这一半已经写完，正式服务也上线了（<https://as.yxswy.com>）；日程规划第一批（日程、提醒、到点通知）的服务端与桌面端已经写完，给 Codex 与 ZCode 用的对话入口 `assemble-mcp` 也写完了，手机上的提醒还没有做。为什么这样做、放弃了哪些备选，见[决策 0002](docs/decisions/0002-own-server-replaces-jarvis.md)。

参考实现是 `jarvis` 仓库（只读，本仓库不修改它，也不迁移它的数据）。

## 现在能做什么

"证据"一栏区分自动化测试与真实运行，两者不能互相替代（见[协作约定](AGENTS.md)）。

| 能力                                                                                                                       | 做到了哪一步                                                                                                                                                                                                             | 证据                                                                                                     |
| -------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------- |
| 账户与登录：服务器命令行建账户，或在网页上凭邀请码注册；桌面端在系统浏览器里登录，密码不经过应用；登录后可以在网页上改密码 | 服务端与桌面端都已实现                                                                                                                                                                                                   | 自动化测试；服务端在真实浏览器里登录成功；注册与改密码在真实浏览器里走过一遍；桌面端整条登录没有真人走完 |
| 设备与在线状态：登录过的每一台电脑，谁在线                                                                                 | 已实现                                                                                                                                                                                                                   | 自动化测试；真实运行                                                                                     |
| 发送到设备：经服务端中转，只能发给在线设备；可以取消、撤回，过期自动清理                                                   | 服务端与桌面端都已实现                                                                                                                                                                                                   | 自动化测试（含对着真实服务端的整条路）；桌面端没有真人收发过                                             |
| 桌面端接收：自动存到"下载/Assemble"，同名加序号，弹系统通知，点通知定位到文件                                              | 已实现；接收位置可以改                                                                                                                                                                                                   | 同上                                                                                                     |
| 桌面端发送：挑文件 → 面板里选云盘或一台设备 → 带进度传完                                                                   | 已实现；失败可以重发，传输途中可以取消                                                                                                                                                                                   | 同上                                                                                                     |
| 云盘：账户在服务端的文件，平铺一层，有配额，同一份内容只存一遍                                                             | 服务端与桌面端都已实现（上传、取回、删除；改名只有接口）                                                                                                                                                                 | 同上                                                                                                     |
| 发起发送的入口：菜单栏"发送文件…"、拖进窗口、带着文件启动、Windows 右键菜单                                                | 都已实现；Windows 右键菜单只在打包后的 Windows 运行里登记                                                                                                                                                                | 带着文件启动在 macOS 上验过未登录；Windows 经典右键菜单已真机核对，点击能唤起应用；拖放未验              |
| 开机启动开关、右键菜单开关                                                                                                 | 已实现，只在打包后的运行里提供                                                                                                                                                                                           | 界面测试                                                                                                 |
| 日程规划                                                                                                                   | 日程、提醒、到点通知：服务端与桌面端都已实现（主窗口里的"日程"卡片、托盘里的"新建提醒…"、点通知定位到那一条）。手机上的提醒还没有做（[决策 0003](docs/decisions/0003-schedule-first-batch-and-conversational-entry.md)） | 自动化测试；服务端的调度与推送在真实运行里核对过，桌面端没有                                             |
| 对话入口 `assemble-mcp`：让 Codex、ZCode 经 MCP 管理日程与提醒                                                             | 已实现：登录成账户里的一台设备，十个工具（看名单，日程与提醒的新建、修改、完成或取消、删除，通知标已读）                                                                                                                 | 自动化测试（对着真实服务端）；发布构建对着正式地址握过手；真实的登录与接入还没有人做过                   |
| 持续集成                                                                                                                   | 仓库公开；每次推送在 macOS 与 Windows 上各跑一遍验证、构建服务端镜像；打标签或手动触发出安装包                                                                                                                           | 两个平台的验证都全绿过，两个平台的安装包都构建成功过                                                     |
| 部署到服务器                                                                                                               | **已上线**：<https://as.yxswy.com>，Docker Compose 加 nginx；2026-11-25 升级到带日程与提醒的版本                                                                                                                         | 从外网核对过：健康检查应答、登录页与证书正常、没带凭据的接口被拒；还没有人在上面登录用过                 |

哪些还没有在真实运行里验证过，逐条列在[运行手册](docs/runbooks/verify-macos-launch.md)的"还没有做过的"一节。

## 环境要求

- Bun 1.4.4 或更高（根目录 `package.json` 的 `packageManager`）
- Rust 1.100.0（`rust-toolchain.toml` 钉住的版本；装了 rustup 的话第一次运行时自动下载）
- macOS 上需要 Xcode 命令行工具

## 开发与验证

```sh
bun install --frozen-lockfile
bun run verify       # 一键验证，提交前运行
bun run acceptance   # 在 macOS 上真实验收：起好本机环境，等你登录，自动核对收发（约 15 分钟）
bun run server:dev   # 只要本地开发用的服务端：数据在 data/local/server，地址 http://127.0.0.1:8787
bun run dev          # 桌面端的开发运行：Vite 热更新 + 调试构建，连本地开发服务
bun run mcp          # 构建给助手用的入口 target/release/assemble-mcp，连正式服务
```

`bun run acceptance` 第一次运行时让你给本机账户 `dev` 设一个密码，然后启动桌面应用；要你亲手做的只有三件：在应用里登录、往回发一个文件、从菜单栏退出。其余的——起服务端、让一台叫"工作电脑"的替身设备发文件、核对落盘的摘要与回执——都由它做。用完 `bun run acceptance stop` 收摊。只起其中一样、或者主流程之外的不顺利情况怎么试，见[运行手册](docs/runbooks/verify-macos-launch.md)。

连接哪一套服务由构建决定，不从界面输入，也不从用户配置读取。正式服务只接受 HTTPS；HTTP 只允许用在回环地址上。`assemble-mcp` 是同一条规矩（调试构建另认 `ASSEMBLE_MCP_ORIGIN`，只给本机验证用）。

| 构建                  | 连到哪里                             | 怎么指定                                                   |
| --------------------- | ------------------------------------ | ---------------------------------------------------------- |
| `bun run dev`（调试） | 本地开发服务 `http://127.0.0.1:8787` | 不用指定；设了 `ASSEMBLE_SERVER_ORIGIN` 就连它             |
| 发布构建              | 构建时给定的地址                     | `ASSEMBLE_SERVER_ORIGIN=https://…`；没给就拒绝启动，不去猜 |

`bun run verify` 依次执行 Biome、类型检查、界面与工具脚本的测试、前端构建、Rust 格式、Rust 静态检查与全部 Rust 测试（含服务端与助手入口）。装了 Windows 的 Rust 目标时（`rust-toolchain.toml` 里列了它），它还会给能在本机上检查的五个基础库做一次针对 Windows 目标的静态检查。客户端与服务端带着 TLS 与 SQLite 的本机依赖，从 macOS 上检查不了 Windows 目标，依赖它们的助手入口也一样，桌面应用壳更是一次都没在这台 Mac 上为 Windows 编译过——那是持续集成里 Windows 那一栏的事。

主窗口的卡片下方固定显示“正式版本 / 预览版本 / 本地测试 - 版本号.commit号”。本机默认是本地测试，GitHub 上匹配应用版本的稳定标签为正式版本，其他持续集成构建为预览版本；可以用 `ASSEMBLE_BUILD_CHANNEL=stable|preview|local` 明确指定。版本号与 Git 提交号自动读取，与连接哪套服务独立；完整约定见[架构说明](docs/architecture.md#主窗口固定展示构建身份)。

### 持续集成与安装包

仓库是公开的：<https://github.com/puzzle-fuzzy/Assemble>。三个流程在 `.github/workflows/`，结果在 <https://github.com/puzzle-fuzzy/Assemble/actions>。

- **验证**（`verify.yml`）：每次推送到 `main` 和每个合并请求，在 macOS（arm64）与 Windows（x64）上各跑一遍 `bun run verify`。Windows 那一栏是服务端、客户端与桌面应用壳唯一的 Windows 环境：它的第一天就抓到了一批只在那边才有的问题，逐条记在[决策 0002](docs/decisions/0002-own-server-replaces-jarvis.md)"从真实启动中学到的"里。
- **服务端镜像**（`server.yml`）：推送到 `main` 或打标签时构建 linux/amd64 的镜像，推到 GitHub 的镜像仓库，公开可拉。
- **安装包**（`release.yml`）：推送 `v` 开头的标签时，产出 macOS 与 Windows 的桌面端安装包，挂在发布页上；也可以手动触发。桌面端连哪个服务在构建时定下：仓库变量 `ASSEMBLE_SERVER_ORIGIN` 是正式地址，手动触发时可以填别的。产出没有签名、没有公证。

工具链钉在 `rust-toolchain.toml`（目前 1.100.0），工作区的 `rust-version` 与 `deploy/Dockerfile` 里的基础镜像和它一致；升级时三处一起改。

### 部署

正式服务在 2026-11-24 上线：<https://as.yxswy.com>。材料在 `deploy/`（`Dockerfile`、`compose.yaml`、配置模板、反向代理的片段、送镜像的脚本）。做过的与还没有验证过的，逐条见[部署手册](docs/runbooks/deploy.md)——其中最要紧的一条：**还没有人在正式服务上登录用过**，第一个账户也还没有建。

## 让助手管理你的日程

你管理日程的方式是对话：对 Codex 或 ZCode 说"我明天 9:00 的火车，提前 90 分钟提醒我，带上身份证"，由助手听懂、算好时间、把要带的东西写进备注，再经 `assemble-mcp` 存进 Assemble；到点时 Assemble 在你的每台电脑上提醒。Assemble 自己不含模型，不做理解和排程（[决策 0003](docs/decisions/0003-schedule-first-batch-and-conversational-entry.md)）。

```sh
bun run mcp                                                  # 构建 target/release/assemble-mcp
target/release/assemble-mcp login --name "ZCode 的助手"      # 在系统浏览器里点一次"确认登录"
target/release/assemble-mcp status --name "ZCode 的助手"
target/release/assemble-mcp print-config zcode --name "ZCode 的助手"   # 或 codex
```

- **登录是助手自己的。** 它成为账户里的一台设备，名字就是你在 `--name` 里给的那个；令牌只存在系统钥匙串里。想收回，在桌面应用的"设备"卡片里移除它。
- **配置由你自己贴。** `print-config` 打印的片段里只有可执行文件的路径和参数：`zcode` 的并进 `~/.zcode/cli/config.json` 的 `mcp.servers`，`codex` 的贴进 `~/.codex/config.toml`。`serve` 时的名字要和登录时的一致。
- **它能做的只有日程与提醒**：看名单，新建、修改、完成或取消、删除日程与提醒，把到点的提醒标成已读。工具只有这十个，文件和云盘没有。
- **时间要带时区**（`2026-11-26T09:00:00+08:00`）。"明天 9:00"由助手换算，所以它会先看一眼名单里现在几点、已有什么安排。和已有日程撞车时只在结果里提示，不拦着。

十个工具、几条规矩和边界写在[架构说明](docs/architecture.md)的"助手入口"里；接进去之后怎么核对，见[运行手册](docs/runbooks/verify-macos-launch.md)。

## 目录

```text
apps/server/            服务端（axum + SQLite）：账户、登录页、设备、发送、云盘、日程与提醒
  migrations/           数据库结构，启动时自动执行
  examples/fake_device  本机验证用的无界面设备
apps/desktop/           桌面端（Tauri 2）
  src/                  界面（React + Vite + Tailwind + shadcn）
  contract/             Rust 与界面共用的状态样本，两侧各自对着它校验
  src-tauri/            应用壳：窗口、托盘、生命周期与具名命令
apps/mcp/               给助手用的入口 assemble-mcp：标准输入输出上的 MCP 服务端，十个日程与提醒的工具
crates/
  assemble-protocol     服务端 ⇄ 桌面端 ⇄ 界面的类型，只有这一份
  assemble-client       桌面端与助手入口访问服务端的唯一出口：HTTP、事件流、回环登录
  assemble-transfer     流式摘要、同目录暂存、校验后原子发布
  assemble-device       落盘的边界：接收目录、文件名、右键菜单的参数
  assemble-store        本机设置文件、系统钥匙串
  assemble-schedule     日程出现规则（服务端、桌面端与助手入口共用）
scripts/
  verify/run.ts         一键验证
  acceptance/           macOS 上的真实验收：起环境、等登录、自动核对
  release/              生成图标
deploy/                 正式服务用的：Dockerfile、compose、配置模板、nginx 片段、送镜像的脚本
.github/workflows/      持续集成：验证、服务端镜像、安装包
rust-toolchain.toml     钉住的 Rust 版本
docs/                   架构、决策、路线图、运行手册
```

依赖方向是单向的：应用依赖基础库，基础库不依赖应用；`assemble-protocol` 只依赖 `serde`，位于最底层。

## 不可破坏的边界

完整的约定在[协作约定](AGENTS.md)，与代码关系最紧的几条：

- **凭据不外流。** 令牌、Cookie 与密码只存在于 Rust 侧和系统钥匙串：不进界面，不进日志，不进命令返回值。
- **登录只发生在系统浏览器里。** 桌面端不加载登录页、不接触密码。
- **界面只有具名命令。** 窗口的能力清单里只有"接收状态"一项；文件路径不经过界面。助手入口也只有十个具名工具，围着日程与提醒。
- **文件只复制，不移动，绝不覆盖。** 落盘一律先暂存、校验摘要后再发布。
- **收到的内容不信任。** 按声明的长度与摘要核对，不一致就丢弃。
- **本机效果先落盘再回执。**

## 文档

- [架构说明](docs/architecture.md)：结构、各部分职责、安全边界、已知的限制
- [决策 0002](docs/decisions/0002-own-server-replaces-jarvis.md)：自带服务端、取代 jarvis，以及"从真实启动中学到的"；[决策 0001](docs/decisions/0001-rust-tauri-desktop.md)是转向之前的记录；[决策 0003](docs/decisions/0003-schedule-first-batch-and-conversational-entry.md)：日程规划第一批的范围、对话式入口与助手怎么登录；[决策 0004](docs/decisions/0004-invite-code-registration-and-password-change.md)：邀请码注册与在网页上改密码
- [路线图](docs/plans/2026-11-23-roadmap.md)：接下来做什么、每一步的完成标准、需要你决定的事
- [日程规划：方案](docs/plans/2026-11-24-schedule-planning.md)：对 jarvis 的调研，与 Assemble 打算怎么做；服务端、桌面端与对话入口的第一批已写完，手机上怎么提醒已定为原生的 iPhone 应用
- [运行手册](docs/runbooks/verify-macos-launch.md)：在 macOS 上真实启动并检查；Windows 真机验收清单；安装包从哪来
- [部署手册](docs/runbooks/deploy.md)：正式服务怎么部署、升级、备份；做过的与还没有验证过的
- [协作约定](AGENTS.md)
