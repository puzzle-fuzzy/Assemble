# Chrome Web Store — Assemble 视频下载

最后更新：2026-11-25。当前状态：开发版，未提交审核或上架。

## 商店内容

名称：Assemble 视频下载（English：Assemble Video Downloader）。

短描述：发现当前网页的视频与 M3U8，交给本机 Assemble 下载。

详细描述草案：

在当前网页发现视频与 M3U8，点击后加入本机 Assemble 下载队列。

打开视频页面，点击工具栏上的 Assemble 图标，选择列表中的视频下载。未发现来源时，可主动开启网络捕获，再播放视频。已加入的任务在桌面显示进度，支持取消和失败重试。

支持点播 M3U8、公开抖音单视频和可直接访问的视频文件。需要同版本 Assemble 桌面程序、浏览器连接组件以及本机 FFmpeg 和 ffprobe。受保护、私人 Cookie、直播和 DRM 来源不支持。

资料在本机处理，没有广告或遥测。仅在用户操作后检测当前页面；网络捕获需要另行授权。简体中文与 English，浅色、深色或跟随系统。

分类：Productivity。单一用途：发现用户当前网页的视频来源并交给本机 Assemble 下载。主要语言：简体中文；同时提供 English。

## 图像素材

| 素材       | 尺寸                | 状态                   | 文件                                             |
| ---------- | ------------------- | ---------------------- | ------------------------------------------------ |
| 商店图标   | 128×128 PNG         | 已构建，尺寸已核对     | `dist/icons/icon-128.png`                        |
| 工具栏图标 | 32×32 PNG           | 已构建，尺寸已核对     | `dist/icons/icon-32.png`                         |
| 商店截图   | 1280×800 或 640×400 | 上架前制作             | 当前自动化弹窗截图用于开发验收，不能代替商店截图 |
| 宣传图     | 440×280             | 未制作，非当前交付范围 | 无                                               |

截图应展示真实网页发现列表和对应的桌面下载任务，避免拍到私人来源地址与账户资料。

## 权限说明

| 权限                        | 类型          | 面向审核的用途说明                                                                                                                                                                                                  |
| --------------------------- | ------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `activeTab`                 | required      | A toolbar click grants temporary access to the current page so the user can find its video sources.                                                                                                                 |
| `scripting`                 | required      | Reads video elements and media links on the page selected by that toolbar action. It does not alter the page.                                                                                                       |
| `storage`                   | required      | Keeps bounded detected sources in browser-session memory so closing the popup or restarting the extension worker does not lose the list.                                                                            |
| `webRequest`                | required      | Detects media request URLs and response type/length only for tabs where the user explicitly enables network capture. Does not read cookies or response bodies.                                                      |
| `nativeMessaging`           | required      | Sends a source selected by a Download click to the locally installed Assemble connector. The desktop app owns the download queue.                                                                                   |
| `http://*/*`, `https://*/*` | optional host | The user may enable capture on arbitrary video websites, including third-party video CDNs. Permission is requested only when the user clicks Capture network; code saves requests only for explicitly enabled tabs. |

没有 `tabs`、`cookies`、`downloads`、静态全站注入、网页对扩展的外部消息入口或远程执行代码。原生宿主允许列表只匹配扩展的精确 ID。官方 `activeTab` 临时授权覆盖工具栏操作后的当前标签 URL 读取；真实 Chrome 已验证该流程。

## 隐私与数据用途

插件处理网页地址、标题、用户选中的媒体地址和捕获时的媒体类型/长度。这属于本机处理的网站内容与当前页活动，不能以“完全不处理任何用户资料”填写商店表单。

| 数据类型                                            | 处理         | 离开设备                                   | 用途                   |
| --------------------------------------------------- | ------------ | ------------------------------------------ | ---------------------- |
| 当前网页地址和标题、媒体地址                        | 是，本机临时 | 插件仅交给本机组件；桌面向所选源站发起下载 | 发现及下载所选视频     |
| 当前页捕获开启、已加入状态                          | 是，本机会话 | 否                                         | 控制捕获与避免重复加入 |
| 语言、外观                                          | 是，本机持久 | 否                                         | 用户偏好               |
| 身份、健康、金融、通信、精确位置、Cookie 和账户凭据 | 不主动读取   | 无此数据流                                 | 无                     |

源地址可能包含访问签名，后台仅临时保留，不向弹窗或持久历史暴露。无遥测、不出售资料、不用于无关目的或信用判断。下载源站会收到正常 HTTP 请求；Fake-IP 回退时 Cloudflare DNS 服务只收到目标域名。完整说明：[PRIVACY.md](PRIVACY.md)。

## 发布前待办

隐私政策公开 URL：尚未发布，提交商店前托管当前政策并核对实际内容。发布者姓名、公开联系邮箱、地区和公开/不公开分发由项目所有者在提交前确定；本文件不编造。主页与反馈使用 [Assemble 项目](https://github.com/puzzle-fuzzy/Assemble)及[问题反馈](https://github.com/puzzle-fuzzy/Assemble/issues)。

上架前仍需正式安装包集成原生连接组件、macOS 真机验收、固定版本媒体工具分发、商店尺寸截图、正式隐私页面与联系邮箱，以及商店分配 ID 的宿主对应。开发 ZIP 只含 `dist` 文件，不含源码、此文档、原生配置、凭据或测试浏览器资料。

## 版本与审核记录

| 版本  | 日期       | 内容                                                               | 状态          |
| ----- | ---------- | ------------------------------------------------------------------ | ------------- |
| 0.1.0 | 2026-11-25 | 主动页面检测、可选网络捕获、所选来源交给本机队列、中英文和外观选择 | Draft；未提交 |

没有审核拒绝记录。已知来源与格式限制见 [README](README.md)。
