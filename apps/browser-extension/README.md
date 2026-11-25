# Assemble 视频下载插件

在 Chrome 当前网页发现视频与 M3U8，点击“下载”后加入本机 Assemble 队列，并打开桌面的网络下载页。无需 Assemble 账户。插件不会把来源交给云服务器。

## 构建和加载

在仓库根目录运行：

```powershell
bun install --frozen-lockfile
bun run browser:build
bun run browser:host
```

打开 Chrome 的 `chrome://extensions`，启用开发者模式，选择“加载已解压的扩展程序”，选中 `apps/browser-extension/dist`。打包分发时仅压缩这个目录内的文件；不要把源码、原生宿主配置或浏览器测试资料打进 ZIP。

当前未上架 Chrome 应用商店。开发版固定扩展 ID 为 `mbdcafmapadhggadghdibebbgfpldhge`；清单中的 `key` 是公开密钥。以后上架若分配了不同 ID，必须同步原生宿主的精确允许列表。

## 连接本机 Assemble

需要包含浏览器下载入口的同版本 Assemble 桌面程序，以及本机 FFmpeg 和 ffprobe。旧版客户端没有这个入口，仅加载插件不能让旧版支持下载。媒体工具发现方式见[网络下载说明](../../docs/runbooks/network-downloads.md)。

从当前源码构建配套桌面程序时，先完成前端构建，再指定服务来源（构建不会安装或启动客户端）：

```powershell
bun run build:frontend
$env:ASSEMBLE_SERVER_ORIGIN='https://as.yxswy.com'
cargo build --release -p assemble-desktop --bin assemble-desktop
```

产物为 `target/release/assemble-desktop[.exe]`。正式安装包另由现有 Tauri 发布流程生成；这里的开发插件不会自动替换桌面客户端。

先检查将要登记的路径，再显式安装连接组件：

```powershell
bun run browser:install --desktop 'C:\绝对路径\Assemble.exe' --dry-run
bun run browser:install --desktop 'C:\绝对路径\Assemble.exe'
```

路径替换为本机更新后客户端的真实可执行文件。macOS 对应 `.app/Contents/MacOS/` 内的可执行文件。安装器默认复制 `target/release/assemble-browser-host[.exe]`；可用 `--host` 指定已构建的宿主绝对路径。独立开发应用标识必须追加相同的 `--app-id`，默认是 `com.puzzlefuzzy.assemble`。

安装器只登记当前用户：Windows 写入 `HKCU\Software\Google\Chrome\NativeMessagingHosts\com.puzzlefuzzy.assemble`，macOS 写入用户 Chrome 的 `NativeMessagingHosts` 目录。宿主与非敏感路径配置保存在用户的 `Assemble/BrowserConnector/<程序摘要>` 目录；配置不含连接令牌。构建和普通自动测试不会登记这些设置。

关闭并重新打开插件后，点击下载即可连接；客户端未启动时，宿主尝试打开所配置的程序。下载任务由桌面进程统一拥有，浏览器关闭不会取消已经加入的任务。

## 使用

1. 打开视频网页，点击 Chrome 工具栏上的 Assemble 图标。
2. 列表显示网页可见的视频来源、M3U8 与抖音单视频页；点击对应的“下载”。
3. 未找到时，点击“网络捕获”，明确授予网站访问权限，再播放视频或刷新同一网站的页面。插件只保存主动启用的标签页资源；点击“停止捕获”可停止，Chrome 扩展详情可撤销网站权限。
4. 桌面网络下载页显示真实进度、取消、失败重试和已完成文件。插件的“已加入”表示任务已进入队列，不表示已经完成。

插件复用共享 shadcn 控件和本机语言、外观选择，提供简体中文与 English。列表只显示标题、类型和来源主机，不展示可能带签名的完整来源地址。

## 支持与限制

支持点播 M3U8、公开抖音单视频地址、可直接访问的 MP4/MOV/M4V/WebM。浏览器选中 M3U8 主清单时使用最高带宽版本，桌面新建弹窗仍可自行选择画质。视频以不转码方式封装为 MP4，编码不兼容时明确失败。

不读取浏览器 Cookie 或账户密码。需要私人登录 Cookie、DRM、SAMPLE-AES、直播和独立 DASH 音画合并的来源不支持。`blob:` 不是可下载的网络地址；底层请求如果是受支持的 HTTP(S) 资源，可通过主动网络捕获发现。受保护的 Chrome 页面不允许检测。主页面扫描不直接读取跨站 iframe 内容；启用捕获后可发现该标签请求到的受支持资源。

完整数据说明见[隐私说明](PRIVACY.md)，商店准备材料见[CHROMEWEBSTORE.md](CHROMEWEBSTORE.md)。Windows 的真实原生链路已单独验证；macOS 仍需真机验收。

## 验证

```powershell
bun run verify
cargo test -p assemble-download real_media_pipeline -- --ignored
```

Windows 真实 Chrome 测试使用独立临时资料，不操作日常 Chrome：

```powershell
bun run browser:build
cargo build -p assemble-desktop --bin assemble-browser-host
cargo build -p assemble-download --example browser-smoke
bun scripts/browser-extension/chrome-smoke.ts --native
```

该测试仅创建独有名称的临时原生宿主登记与系统凭据项，实际下载公开测试视频，退出时删除所创建的登记与凭据；截图和下载产物留在忽略目录 `.artifacts/browser-tests/`。追加 `--release-host` 可改验发布构建的宿主；不带 `--native` 时验证发现来源与未安装连接组件的错误提示。

移除组件时，在 Chrome 移除扩展；Windows 删除上面精确命名的当前用户注册表项，macOS 删除对应的 `com.puzzlefuzzy.assemble.json`，再删除本用户的 `Assemble/BrowserConnector` 目录。不会删除下载文件与桌面历史。
