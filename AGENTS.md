# 开发约定

## 这个仓库是什么

Assemble 是 Jarvis 桌面端的重写：Rust 负责业务规则，Tauri 2 只做窗口与系统集成。服务端不在本仓库，也不修改。优先级是文件传输与日程规划。

先读 [README](README.md)、[架构说明](docs/architecture.md) 与 [路线图](docs/plans/2026-11-23-roadmap.md)。参考实现在 `jarvis` 仓库，只读，不要修改它。

## 不可破坏的边界

- **凭据不外流。** 令牌与 Cookie 只存在于 Rust 侧：不进界面，不进日志，不进命令返回值，不进状态快照，不进文档。
- **界面只有具名命令。** 新增能力时同时在 `src-tauri/src/commands.rs` 与 `src/lib/bridge.ts` 添加，并补上桥接测试。主窗口的 capability 保持最小，不要为了方便放开窗口或文件权限。
- **状态形状由样本钉住。** 改动界面能看到的状态时，同步修改 `apps/desktop/contract/snapshots.json`：Rust 与 TypeScript 两侧的测试都会对着它失败。
- **公布的必须等于提供的。** 网关对外公布的能力目录来自实际实现的操作，不得退回到契约里的全部操作。
- **服务端没有确认撤销的会话，不在本机忘掉。**
- **文件只复制，不移动，绝不覆盖。** 任何文件访问都先经过 `assemble-device` 的路径边界检查；新目录只能来自系统选择器。
- **本机效果先落盘再回执。**
- **退出时的清理显式地做。** 不要指望析构：框架托管的状态在进程结束前未必会被释放。
- **线上契约变化时**，改 `assemble-protocol`，并同步对应的客户端方法与测试。

## 怎么写

- 规则放在 `crates/` 里，脱离窗口与网络就能测试；`apps/desktop/src-tauri` 只做编排。
- 每个基础库的入口固定写 `#![cfg_attr(test, allow(clippy::pedantic))]`：库代码从严，测试代码以可读为先。
- Rust 工作区禁止 `unsafe`；clippy 的 `all` 与 `pedantic`（除 `must_use_candidate`）按警告即错误执行。
- 注释与文档用中文，解释"为什么"而不是复述代码；面向用户的文案用中文；错误码与开发日志用英文。
- 不留占位：没有写完的分支不要用空实现、`todo!` 或恒返回值的函数凑数。

## 验证

- 提交前运行 `bun run verify`。它覆盖 Biome、类型检查、界面测试、前端构建、Rust 格式、静态检查与测试。
- 汇报时区分三种证据，不能互相替代：
  - 自动化测试（`bun run verify`）；
  - 真实启动（在 macOS 上构建并运行，核对窗口、托盘、网关与退出清理，步骤见 [运行手册](docs/runbooks/verify-macos-launch.md)）；
  - 真机验收（登录、通知、开机启动、Windows）。
- 实际运行抓到过单元测试没暴露的问题，所以涉及应用壳、网关或生命周期的改动，要真的把应用跑起来。

## 文档

- 代码变化后同步更新受影响的文档；文档不描述尚未实现的能力，计划放在 `docs/plans/`。
- 用户的反馈或纠正，连同原因一起记进对应的文档，而不是只改代码。

## 依赖

- TypeScript 侧只使用 Bun：根目录唯一的 `bun.lock`，不引入其它包管理器。
- 格式与 lint 只使用 Biome；Biome 也会格式化 Markdown 表格。
- 前端组件使用 shadcn 生成的 `apps/desktop/src/components/ui`，图标使用 Lucide。
