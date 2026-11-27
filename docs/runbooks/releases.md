# 产品下载与安装包发布

2026-11-27 用户要求统一的产品发布/下载页面，包含 Windows、macOS、iOS、Android。页面为同源 `/download/`，根路径 `/` 临时重定向到这里；`/download` 永久规范到 `/download/`。复用共享 UI、字体、简体中文/英语及浅色/深色/跟随系统偏好，保留影视站入口。每个平台显示实际架构、版本、文件大小和下载；无可用包显示“未发布”。正常状态不放刷新按钮，只有读取失败时提供重试。校验信息使用弹窗，关闭后焦点回到入口。

本轮实现不等于公开发行：当前 GitHub CI artifacts 仍是构建证据，需要登录或可能过期的链接不能放到正式下载页。本仓库不内置演示安装包、虚构商店地址或签名公证声明。iOS/Android 没有正式产物时保持未发布；未来即使提供 IPA，它也只是文件下载，不代表可以从浏览器直接安装。

手动安装包与桌面更新器是两条入口：这里的 `releases/` 用于本人主动下载安装包；`updates/` 和 `/updates/latest.json` 由桌面更新功能使用，发布必须经过 `publish-update` 的签名校验。本页的清单不能替代更新清单或绕过验签，二者都不保存签名私钥。详见[决策 0015](../decisions/0015-desktop-auto-update.md)。

## 持久目录与接口

服务端从既有 `ASSEMBLE_DATA_DIR` 下的 `releases/` 读取：

```text
releases/
  manifest.json
  objects/
    <64位小写SHA-256>
```

它属于持久发行资料，不是临时缓存，不随镜像重建或桌面升级删除。安装包不计入普通账户云盘配额或内容预算，但会占用同一卷的磁盘余量；规划空间时要把它们计算在内。备份和恢复必须同时包括清单与对象。

- `GET /v1/releases` 匿名读取，返回 `{schemaVersion:1,artifacts:[...]}`。每个平台/架构按实际发布时间选择最新可用包，最新包缺失或损坏时可回退之前的可用版本。未建立发布目录或清单时返回空数组；清单无效返回 503。接口不读账户或登录凭据，不缓存清单。
- `GET/HEAD /v1/releases/files/{sha256}` 只接受清单中的固定摘要标识。未知、越界、缺失对象返回 404；摘要不符返回 503。首次按已打开句柄核验完整文件，之后比较长度、修改/创建时间（Unix 另含设备、inode、ctime）复用校验缓存，发生变化就重新核验；HTTP 进程只读、维护脚本不修改既有对象。HEAD 不重复全量哈希或构造传输流，匹配 `If-None-Match` 时返回 304。实际传输使用同一个已检查句柄，返回安全的附件文件名、`Content-Length`、`Content-Digest: sha-256=:base64:`、ETag 与 `nosniff`，按不可变摘要长缓存。哈希并发最多四个，即使请求已取消，工作额度也保留到后台任务结束。暂不提供断点续传 Range；需要重试时完整下载。
- 清单只允许 `schemaVersion`、`artifacts`。每个条目只有 `platform/architecture/version/fileName/sha256/size/publishedAt`；平台与架构组合、文件扩展名、ASCII 文件名、版本及长度均校验。时间统一使用维护脚本输出的 UTC 毫秒格式（如 `2026-11-27T00:00:00.000Z`），拒绝不存在的日期、24点、闰秒及其他偏移写法；Rust 字段仍为共享 Timestamp。清单不超过 128KiB，最多 128 个历史条目，单包最多 2GiB。拒绝重复摘要或同平台/架构/版本的冲突。
- 不接受任意 URL、服务器路径或外部重定向字段，因此不存在过期 artifact URL、任意源地址或凭据 URL。下载链接只由固定路由和校验过的摘要生成。包及对象目录不能是符号链接，不公开目录遍历或上传/写管理 API。

## 发布维护

命令只由拥有发布目录写权限的维护者执行。HTTP 进程仅需只读权限，普通上传及用户文件不得写入此目录。准备安装包时核对实际目标架构、应用“关于”版本和提交、正式服务来源；签名、公证与安装可用性需要真实平台证据，下面的脚本只验证容器格式、长度和字节摘要。

在仓库根运行，例如：

```sh
bun scripts/release/publish-artifact.ts publish --directory <data_dir>/releases --source <Windows安装包.exe> --platform windows --architecture x64 --version 0.1.0.abc1234
bun scripts/release/publish-artifact.ts publish --directory <data_dir>/releases --source <macOS安装包.dmg> --platform macos --architecture arm64 --version 0.1.0.abc1234
bun scripts/release/publish-artifact.ts check --directory <data_dir>/releases
```

版本和路径是示例，不能把示例版本当成已发布。Windows 支持 EXE/MSI；macOS 支持 DMG/PKG；未来 iOS 是 ARM64 IPA，Android 是 APK。脚本检查 MZ、CFB、UDIF、XAR 或 ZIP 的对应格式标记，不能代替平台的签名验证或安装验收。

脚本只复制源文件，不移动或修改原件；按目录锁串行维护。复制到暂存对象，比较源文件与暂存文件的 SHA-256 及长度，并刷盘；以同目录硬链接原子公布完整对象，不覆盖已有对象；最后刷写临时清单并原子替换 `manifest.json`。此方案需要发布盘支持硬链接和同目录原子重命名（NTFS/ext4 等）。同一包重复发布返回原身份、不改变发布时间；同版本不同字节拒绝，不会悄悄替换已发布安装包。

历史清单条目和对象保留，旧摘要链接继续可用。脚本没有自动清理或批量删除命令。中断后，先确认没有维护进程，再核对 `.publish-lock`、暂存文件和清单；未被清单引用的对象不会被 HTTP 提供下载。不要为了处理失败直接覆盖损坏对象或删掉既有发行历史，先保留证据并核对备份。

如果在另一台电脑准备发布目录，先完整校验，将对象复制到服务端持久卷，最后再原子切换清单；不能先上传清单后慢慢上传安装包。正式目录初始化或首次公开安装包仍需按发布流程执行，本功能不会自动创建 GitHub release、部署或安装。

## 构建与验证

前端：`bun run releases:dev`（1423，`/v1/releases` 代理到本机 8787）；`bun run releases:build` 输出 `apps/releases-web/dist`。生产静态目录配置为 `ASSEMBLE_RELEASES_WEB_ASSETS_DIR`，和持久安装包目录分别管理；页面资源可随镜像更新，安装包不能烘焙进镜像充当持久存储。静态资源具备同源 CSP、nosniff 和缓存约束，开发代理移除 Cookie/Authorization 与伪造转发头。

定向检查：

```sh
bun test apps/releases-web/src/App.test.tsx scripts/release/publish-artifact.test.ts
cargo test -p assemble-protocol -p assemble-server --lib releases::
bun run --filter @assemble/releases-web typecheck
bun run releases:build
```

测试覆盖可用/未发布状态、语言、失败重试、校验弹窗与焦点、旧请求取消、清单字段和大小边界、真实文件发布与重复运行、冲突和损坏拒绝、真实 TCP HTTP 下载字节/HEAD/摘要头/公开页面与跳转。符号链接拒绝另有 Unix 测试；Windows 不申请创建符号链接的额外权限。完整工作区验证、Docker 实际启动、浏览器窄屏和主题验收由整合发布流程继续核对，不能把定向测试说成正式发行或安装验收。
