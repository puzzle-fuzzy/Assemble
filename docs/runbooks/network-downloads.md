# 网络文件下载

桌面侧栏的“网络文件下载”不要求登录 Assemble。页面名称仅保留在面包屑，“新建下载”“选择文件夹”两个小按钮放在“下载任务”卡片右上角，保存位置列在任务标题下。新建弹窗里选择 M3U8 或抖音、粘贴地址并解析，再选择画质和开始下载。两处下拉使用共享 shadcn Select；关掉弹窗可保留当前草稿，开始成功后弹窗自动关闭。页面只列任务、真实进度和保存位置。

下载、合并和校验在当前电脑的 Rust 侧执行。云服务器已安装的 FFmpeg 不参与这条流程，也不经过设备传输或云盘的 100 MB 文件上限。

## 本机媒体工具

当前版本需要本机安装 FFmpeg 与 ffprobe。按以下顺序查找：各自的 `ASSEMBLE_FFMPEG_PATH` / `ASSEMBLE_FFPROBE_PATH`、桌面可执行文件同目录、系统 PATH，最后是几个常见的安装位置（用户目录下的 `.local/bin`；macOS 上另有 `/opt/homebrew/bin` 与 `/usr/local/bin`）。两者都找到才允许开始下载；安装后重启应用。路径只在 Rust 内部使用。

最后那一步是 2026-11-26 加的。从启动台、托盘或开机启动项起来的应用拿不到你在终端里设的 PATH——macOS 上 launchd 只给系统的那几个目录——而 Homebrew 和不少工具正是装在那些位置里。那天在这台 Mac 上就是这样：终端里找得到 FFmpeg 9，装好的应用却找不到。四种先后关系都有单元测试（`media.rs`）。

目前没有将这两个工具捆绑进正式安装包，也未更改安装包发布流程。分平台固定版本、来源校验与许可证随包分发仍列在下载功能计划中，不能把开发机已有工具当成安装包自带。

## 支持范围

- M3U8 支持带 `ENDLIST` 的点播、主清单画质、相对网址、TS、fMP4 初始化片段、字节范围、独立音轨和普通 AES-128 密钥。不支持直播、DRM 与 SAMPLE-AES。
- 本地 M3U8 点击“选择 M3U8 文件”导入，先直接选择文件即可；其中已有完整视频链接时无需再填地址。如果文件里的链接不完整，再在同一个“M3U8 下载链接”框填写下载该文件的链接并重新选择文件，也可用浏览器插件直接获取来源。这个链接指向 M3U8 文件，不是视频播放网页；只下载 HTTP(S) 内容，不读取文件里指定的本地路径。
- 来源网页可选，供需要 Referer 的公开清单使用；不提供任意请求头或 Cookie 输入。
- 抖音接受公开视频网址、含 `modal_id` 的网址、短链与整段分享文字。只解析目标视频，不抓取推荐视频或用户列表。图集、需要登录或受限的视频会失败。
- 首版最多同时运行两个任务、排队二十个；每个任务最多 10 GiB，媒体清单最多 20,000 分片，清单元数据最多 4 MiB。分片有有限重试，不支持跨重启断点续传。
- 下载目录所在的磁盘至少要剩 200 MiB 才开始写，已知道要写多少时还得放得下那一份；排队中的任务轮到自己时再看一眼。点播清单的总大小事先不知道，所以开始之后仍然可能写满：那时候报"磁盘空间不够"（`DOWNLOAD_NO_SPACE`），不和别的存储故障混在一起。合并还要再写一份成品，所以不是"刚好放得下"就够。规则在 `space.rs`。查询前确认目录存在，已有下载目录查询它自身的磁盘；避免 Windows 对缺失目录仍返回卷容量，或将挂载目录误查为父目录所在的磁盘。

成功任务可打开视频或定位文件；取消和失败任务可在当前进程内重试。来源过期需重新解析。重启时，未完成历史标为中断，需重新输入来源，不假装仍在下载。

## 文件与凭据边界

默认保存在系统下载目录下的 `Assemble/网络下载`；可以用系统选择器更换目录。界面只显示文件夹简称，文件操作只传任务标识。下载和合并先放在该目录的独立暂存子目录，ffprobe 确认真实 MP4 视频后才用同卷硬链接原子发布；同名自动加序号，绝不覆盖。不支持硬链接的文件系统明确报保存失败。

进程被杀掉的时候暂存目录会留下来，里面可能是好几个 G 的分片。应用启动时、窗口开出来之前，会把它们清掉（`DownloadManager::sweep_staging`）：只删名字对得上的目录，目录先过边界检查，不跟着链接走；下载好的文件和别的目录不动。

清单、重定向、分片和密钥均由 Rust 检查协议、标准 HTTP(S) 端口、公网地址并固定连接。FFmpeg 仅处理重建的本地清单及本任务文件，关闭网络协议；分片解封装只允许 MPEG-TS、MOV/MP4 和 AAC。不会读取系统浏览器 Cookie、密码或 Assemble 的设备令牌。

下载客户端不继承环境 HTTP 代理，以免代理替换已检查的 DNS 连接。系统 DNS 全部返回代理软件的 `198.18.0.0/15` Fake-IP 地址时，才通过固定 Cloudflare DoH 地址查询真实公网 IPv4，最多缓存三十秒。它只收到目标域名；完整网址和签名参数不会发给 DNS 服务。真实私网解析、直接输入的保留 IP 和重定向到私网仍会被拒绝。DoH 不可达时明确失败。[Cloudflare DoH JSON 接口](https://developers.cloudflare.com/1.1.1.1/encryption/dns-over-https/make-api-requests/dns-json/)

完整来源、签名网址、匿名抖音 Cookie 和密钥只留在 Rust 内存或当前任务暂存文件，不写任务展示状态或历史。私有历史文件保存展示字段和已发布文件的位置；普通失败仅向界面返回稳定错误码。没有来源摘要时，本地计算的摘要不能证明上游内容真实，完整性还依赖声明长度、范围、所有资源成功和媒体检查。

## 验证与后续入口

自动化检查运行 `bun run verify`。真实媒体测试另需本机工具，明确运行：

```powershell
cargo test -p assemble-download real_media_pipeline -- --ignored
```

它实际生成含声音的测试视频，分别经 TS、fMP4 单文件字节范围、AES-128 和独立音轨清单走完整下载、合并、校验与发布，检查音视频流、旧文件不覆盖和暂存清理。普通测试机缺工具不会把这项伪装为通过；默认测试将其标为需单独运行。

2026-11-26 在 macOS 上用本机的 FFmpeg 与 ffprobe 9.0.1 真跑过一次，通过（1.4 秒）。同一天补的几条规矩各有单元测试：磁盘余量怎么算、写满是它自己的一个错、崩溃留下的暂存目录被清掉而别的都不动、PATH 里找不到时去常见位置找。**装好的应用里还没有人在 macOS 上点过这一页**，扩展也还没有装进日常的 Chrome。

在 macOS 上把开发版插件装进日常 Chrome（2026-11-26 核对过安装脚本，**还没有人真做过**；Windows 的真实链路验过，见[插件说明](../../apps/browser-extension/README.md)）：

```sh
bun install --frozen-lockfile
bun run browser:build                                                   # 插件本体，产出到 apps/browser-extension/dist
cargo build --release -p assemble-desktop --bin assemble-browser-host   # 本机桥接的宿主程序
bun run browser:install --desktop /Applications/Assemble.app/Contents/MacOS/assemble-desktop --dry-run
bun run browser:install --desktop /Applications/Assemble.app/Contents/MacOS/assemble-desktop
```

最后两条会写两处用户级的东西：Chrome 的 `NativeMessagingHosts` 目录里一份清单，以及 `~/Library/Application Support/Assemble/BrowserConnector/` 下宿主程序的一份副本和非敏感配置。然后在 Chrome 打开 `chrome://extensions`，开"开发者模式"，"加载已解压的扩展程序"选 `apps/browser-extension/dist`。桌面上那份 Assemble 必须是带下载入口的新版本；旧版本只有插件是用不了的。装好之后拔掉再插一次插件（关掉再打开），点一个视频页上的 Assemble 图标试下载。

仅针对公开、可分享来源的手工验证可运行 `cargo run -p assemble-download --example inspect -- <公开地址>`；追加 `--download` 会实际下载到临时目录、校验音视频流后清理。不要把带私密签名的地址放进命令行或终端历史。

Chrome 插件已实现主动检测网页视频/M3U8，并可将所选来源加入同一个桌面下载队列。它经受限制的原生宿主交接，临时连接凭据仅在系统凭据库与 Rust 内部；插件不接触 Assemble 登录凭据。MP4/MOV/M4V/WebM 直链先核对真实容器，再无转码封装为 MP4；浏览器提交主清单时使用最高带宽版本。加载、原生组件登记、支持范围与真实 Chrome 验证见[插件说明](../../apps/browser-extension/README.md)。当前是开发版，尚未安装到日常 Chrome 或上架商店，MCP 下载入口仍未实现。
