# Assemble

> **本文已过时，重写排在文件收发打通之后。** 下面描述的是转向之前的 Assemble——"Jarvis 桌面端的重写，服务端沿用现有的 Jarvis API"。从 2026-11-24 起，Assemble 是一个全新的、自带 Rust 服务端的项目，用来完全替换 jarvis，见 [决策 0002](docs/decisions/0002-own-server-replaces-jarvis.md)。
>
> 已经落地的部分：`apps/server`（账户、登录页、桌面授权、设备与在线状态、事件流），以及桌面端的登录流程与设备列表。怎么把它们真的跑起来，见 [运行手册](docs/runbooks/verify-macos-launch.md)。本文的其余内容不要照着做。

Jarvis 桌面端的重写：把原来以 Electron 为主的桌面应用，换成以 Rust 为核心、Tauri 2 只做窗口与系统集成的方式。服务端沿用现有的 Jarvis API，本仓库只负责桌面这一侧。

当前阶段是**起步骨架**：基础库、应用壳、登录与账户、本机设置已经打通；文件传输、日程规划的底层规则已经写好并有测试，但把它们连成完整功能的后台循环与界面还没有做。详见 [路线图](docs/plans/2026-11-23-roadmap.md)。

参考实现在 `jarvis` 仓库（`main` 分支，契约基线 `0d6eb5f`）。本仓库不修改那个仓库。

## 现在能做什么

| 能力                                                                                 | 状态                                                            |
| ------------------------------------------------------------------------------------ | --------------------------------------------------------------- |
| 六个 Rust 基础库：线上契约、本机存储、传输原语、设备运行时规则、API 客户端、日程推算 | 已实现，有测试                                                  |
| 应用壳：主窗口、托盘、单实例、关窗口收起到托盘、登录项后台启动                       | 已实现                                                          |
| 登录：系统浏览器授权 → 回环回调 → 换取设备会话；启动时恢复；退出                     | 已实现，**尚未对真实服务做过端到端验收**                        |
| 开机启动开关（以系统实际状态为准）                                                   | 已实现，仅在发布构建里提供                                      |
| 供本机工具（如 MCP）使用的回环网关                                                   | 已实现协议；本构建只提供 `status`、`capabilities`、`config.get` |
| 设备上线、目录授权、上传下载、扫描、通知投递、文件与日程界面                         | **未实现**，见路线图                                            |

## 环境要求

- Bun 1.4.4 或更高（根 `package.json` 的 `packageManager`）
- Rust 1.90 或更高（`Cargo.toml` 的 `rust-version`）
- macOS 上需要 Xcode 命令行工具

## 开发与构建

```sh
bun install --frozen-lockfile
bun run dev        # 开发运行：Vite 热更新 + 调试构建
bun run build      # 发布构建：生成应用包（macOS 上是 .app 与 .dmg）
bun run verify     # 一键验证，提交前运行
```

连接哪一套服务由构建类型决定，不从界面输入：

| 构建                    | API                        | 日常网页（登录页）              |
| ----------------------- | -------------------------- | ------------------------------- |
| `bun run dev`（调试）   | `http://127.0.0.1:3300`    | `http://127.0.0.1:5474/`        |
| `bun run build`（发布） | `https://jarvis.yxswy.com` | `https://jarvis.yxswy.com/app/` |

调试构建需要先在本机启动 jarvis 的本地 API 与网页，否则界面会显示"本地开发服务暂不可达"，登录按钮不可用。

构建产物在 `target/release/bundle/`。发布构建没有签名与公证。

其它入口：

```sh
bun run typecheck      # 界面与工具脚本的类型检查
bun run test           # 界面测试（bun test）
bun run lint           # Biome：lint 与格式
bun run format         # Biome：自动修复
bun run rust:fmt       # cargo fmt --check
bun run rust:clippy    # 严格静态检查
bun run rust:test      # cargo test --workspace
```

`bun run verify` 依次执行 Biome、类型检查、界面测试、前端构建、Rust 格式、clippy 与 Rust 测试，任一步失败即停止。

## 目录

```text
apps/desktop/            Tauri 应用
  src/                   界面（React + Vite + Tailwind + shadcn）
  contract/              Rust 与界面共用的状态样本，两侧各自对着它校验
  src-tauri/             应用壳：窗口、托盘、生命周期与具名命令
crates/
  assemble-protocol      Jarvis 线上契约的 Rust 类型与校验，不做 I/O
  assemble-client        访问云端的唯一 HTTP 出口：固定来源、凭据、回环登录
  assemble-store         本机状态文件的原子私有写入、系统钥匙串
  assemble-transfer      流式摘要、同目录暂存、校验后原子发布
  assemble-device        路径边界、目录授权账本、命令回执账本、本机网关
  assemble-schedule      日程出现规则推算、通知去重账本
scripts/
  verify/run.ts          一键验证
  release/generate-icons.ts  生成占位图标
docs/                    架构、决策与路线图
```

## 设计要点

- **界面不碰凭据，也不直接联网。** 界面只能调用 `src-tauri/src/commands.rs` 里的几个具名命令，并接收 Rust 推来的状态；主窗口的权限只有"接收事件"一项。
- **服务端没有确认撤销的会话，不在本机忘掉。** 退出时连不上服务，就保持登录并提示，不留下一条没人认领却仍然有效的会话。
- **云端记录不等于本机授权。** 共享目录必须与本机私有授权取交集，在云端改路径不会扩大本机权限。
- **本机效果先落盘再回执。** 租约丢失或断网后，同一条命令再到达时只重发回执，不重复执行。
- **只复制，不移动，绝不覆盖。** 内容先写进同目录的 `.part`，校验通过后以硬链接发布。

## 文档

- [架构说明](docs/architecture.md)：结构、各部分职责、安全边界、与 Electron 版的差异、已知的限制
- [决策记录](docs/decisions/0001-rust-tauri-desktop.md)：为什么这样选、放弃了什么、**需要你确认的十项**
- [路线图](docs/plans/2026-11-23-roadmap.md)：文件传输与日程规划的后续阶段，以及每个阶段的完成标准
- [运行手册：在 macOS 上真实启动并检查](docs/runbooks/verify-macos-launch.md)
- [协作约定](AGENTS.md)
