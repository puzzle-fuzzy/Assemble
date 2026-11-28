# 部署 Assemble 服务端

实际运行版本以 `docker inspect` 的镜像引用为准，下列发布记录保留各次证据。

## 服务器磁力升级

新增迁移 `0018_hosted_magnets.sql`，升级前照本文核对旧迁移校验和并保存一致数据库及完整数据备份。私有磁力缓存位于数据卷 `hosted-magnets/`，与数据库一起备份；停止服务会显式停止 BT，重启先暂停，不自动恢复下载。无需新增端口、nginx 或系统代理设置。先升级服务器，再安装带新接口的客户端。

磁力缓存按两份所选容量预留，与上传和内容共享全站预算；账户活跃所选总量受现有 `ASSEMBLE_DRIVE_QUOTA_BYTES` 约束。管理端存储中的“传输缓存预留”包含这些缓存。清理成功才释放预留；暂停缓存 24 小时未续期自动取消，终态回执保留 30 天。实际使用与诊断见[网络下载手册](network-downloads.md)。

## 既往发布：2026-11-27 云盘目录、回收站与操作日志

正式服务已更新为 `ghcr.io/puzzle-fuzzy/assemble-server:9a7dfe5eefdd`，源码 `9a7dfe5eefdde0b5de488b10adc8f25afaac36c2`，镜像标识 `sha256:dd7bbba7c02d2ca84c691deb574ec0ddec9c659319547dd8b4167fe27572a9c9`。[Windows/macOS 验证](https://github.com/puzzle-fuzzy/Assemble/actions/runs/46014498628)与[服务端镜像](https://github.com/puzzle-fuzzy/Assemble/actions/runs/46014498660)全部成功。本机提交前完整 verify 通过，用时 137.8 秒；N3 真实 Windows、Chrome 与镜像验收见[回收站手册](drive-trash.md)。

受限备份位于 `/var/backups/assemble/20261127-141559-n3-release/`，包含配置、一致数据库、完整数据归档（759,920,640 字节）及摘要和逐表核对回执。先用真实三份 Compose、无网络临时容器和线上数据副本演练，再停写升级正式库；迁移从 1～13 更新为 1～16，旧脚本校验一致。原业务列的内容指纹和数量均相同：账户 1、设备 3、云盘文件 2、日程 5、提醒 5、影视条目 147、合集 16、备忘录 13；实体内容长度与摘要一致。停服务到就绪约 57.14 秒，随后容器 healthy，`check-config` 通过。

只改镜像引用，Compose、其余环境、密钥、nginx、已签名桌面更新均核对未变。HTTPS 健康、就绪、管理端、影视、网页登录与下载页均 200；匿名管理会话、云盘目录及回收站均 401、no-store。没有创建正式测试账户或更改用户文件。本次未发布或安装桌面签名更新；Mac WebView、两台真实设备目录与恢复交互仍待验收。服务重新接受写入后必须保全最新数据向前修复，不能用旧数据库覆盖。

## 此前发布：2026-11-27 图集同步与封面弹窗

服务端与管理端已更新为 `ghcr.io/puzzle-fuzzy/assemble-server:e94b1631cd16`，对应源码 `e94b1631cd16fdfc87e3d48d741eb1353f81356e`。镜像标识为 `sha256:fd0f6620a163c88d141a79f5f0d0f6f060dd7b27cb5c452984ec579dc532c6b8`。[Windows/macOS 验证](https://github.com/puzzle-fuzzy/Assemble/actions/runs/45981364782)和[服务端镜像](https://github.com/puzzle-fuzzy/Assemble/actions/runs/45981364521)均成功；本机最终 `bun run verify` 为 481 项 TS、690 项 Rust 通过，2 项既有忽略，用时 121.4 秒。新增回归覆盖来源超量去重与翻页预算、真实任务批量替换及损坏图片保留原图集、简洁错误提示、已知失败与未知提交的不同重试策略及版本冲突恢复。

用户反馈封面图集弹窗说明过多、“刷新”用途不清，以及来源超过 100 张图片时整次失败。弹窗现只保留片名、数量、同步和当前封面标识：单张海报居中，多图采用可滚动的响应式网格；任务进度与错误按需出现，不显示任务编号。来源改为有界去重同步，每次最多 100 张，全部校验成功后再原子替换图集并保留原封面；真实下载、校验或配额错误仍保留原图集。行为与重试规则见[影视手册](media.md#管理导入与失败恢复)。

镜像经现有 SSH 通道传入。升级前用线上数据库的一致副本和现有三份 Compose 完成 `check-config`，核对实际挂载指向副本；迁移 1～13 校验和一致，无新迁移。受限备份在 `/var/backups/assemble/20261127-182850-media-gallery-e94b1631cd16/`：配置归档、最终一致库、完整数据归档 `data-before.tar`（647,751,680 字节）及摘要、切换前后每张业务表指纹、部署脚本与回执；目录 0700、文件 0600。停服务到 healthy 为 25.19 秒，对外重开前每张表数量和内容指纹均相同。只改镜像引用，其余环境、Compose、密钥、nginx 与已签名桌面更新逐项核对未变；容器仍为专用账户、回环端口与只读密钥挂载。

正式 HTTPS 的健康、就绪、管理端、影视、登录与下载页均为 200，未登录管理会话入口为 401。正式 Chrome 用已有会话在“饥饿游戏2：星火燎原”上实际同步成功：从 1 张变为 101 张（100 张来源图加原封面），界面显示“图片已同步”，随后公开接口再次核对原封面资产引用未变；没有更换封面或创建正式测试账户。本机浏览器另验单图、多图、失败重试与 375px 窄屏无横向溢出。本次没有发布或安装桌面更新，桌面 Mac WebView 的实际显示仍待真机验收。

## 2026-11-27 管理端列表网址状态

服务端与管理端已更新为 `ghcr.io/puzzle-fuzzy/assemble-server:5dc2b92638e1`，对应源码 `5dc2b92638e1d52f6834e1c32f3dfeab08cb82d6`。镜像标识为 `sha256:5d0a488eef225c7c67b743f947954a17c7d4b8218519d99ccb8cdbcf5cce4b57`。[Windows/macOS 验证](https://github.com/puzzle-fuzzy/Assemble/actions/runs/45973806894)和[服务端镜像](https://github.com/puzzle-fuzzy/Assemble/actions/runs/45973806832)均成功；本机 `bun run verify` 为 478 项 TS、688 项 Rust 通过，2 项既有忽略。新增 12 项网址恢复回归，覆盖所有管理列表、DHT 筛选、影视页签与合集、前进后退、非法参数和页码越界；另核对桌面影视原有 4 项测试通过。

用户要求翻页后刷新仍保留当前页，并明确选择地址栏保存状态。页码、条数、搜索和列筛选现写入哈希查询串，不用 localStorage；影视合集独立保存页码。正式 Chrome 使用已有会话打开 DHT 第 2 页、每页 10 条，翻到第 3 页后刷新仍显示第 3 页和每页 10 条；没有新建正式测试账户或修改采集模式。完整行为见[管理端手册](admin.md#开发与发布)。

镜像经现有 SSH 通道传入。升级前用线上数据库的一致副本和现有三份 Compose 完成 `check-config`，核对实际挂载指向副本；迁移 1～13 校验和一致，无新迁移。受限备份在 `/var/backups/assemble/20261127-173103-admin-pagination-5dc2b92638e1/`：配置归档、最终一致库、完整数据归档 `data-before.tar`（642,856,960 字节）及摘要、切换前后每张业务表指纹与部署回执；目录 0700、文件 0600。停服务到 healthy 为 23.06 秒，对外重开前每张表数量和内容指纹均相同。只改镜像引用，其余环境、Compose、密钥、nginx 与已签名桌面更新逐项核对未变；容器仍为专用账户、回环端口与只读密钥挂载。本次未发布或安装桌面更新。

## 2026-11-27 云盘预览

服务端与管理端现已部署为 `ghcr.io/puzzle-fuzzy/assemble-server:ef808271dafc`，源码 `ef808271dafce03685582b8b6d28689d0e0e443c` 已先提交并推送 main，镜像标识 `sha256:311875afea73f83d4c5cc9ffcc27d3427e7839c0b1174e4ca6b63e46d0cf6866`。[Windows/macOS 验证](https://github.com/puzzle-fuzzy/Assemble/actions/runs/45965360406)与[服务端镜像](https://github.com/puzzle-fuzzy/Assemble/actions/runs/45965360550)全部成功；本机完整验证为 466 项 TS、688 项 Rust 通过，2 项既有忽略。

这是已提交主干的完整服务镜像，连同既有影视公开、远程控制中继和下载页的服务端能力一起对齐，不再沿用日程补丁的裁剪版本。

- 云盘预览本身没有新迁移；此前正式服务是日程补丁版，账本为 `1…11,13`。本次补上已提交的 `0012_media_publication.sql`，现在为 1～13，旧脚本校验和逐项一致。先在受保护在线备份副本上，用真实迁移器与无网络容器验证了启动和正常退出。
- 正式停写后逐表核对：除影视新增公开/归属字段外，所有业务表内容指纹和数量一致；私人记录保持未公开。迁移前后账户 1、设备 3、云盘文件 2、日程 5、提醒 5、影视条目 147、合集 16、私人备忘录 13；实体文件的大小与 SHA-256 全部匹配，数据库自检与外键检查通过。
- 完整备份位于 `/var/backups/assemble/20261127-082539-n1-ef808271dafc/`，外层目录 0700、配置与数据材料 0600。最终一致库为 `final-before.sqlite3`，完整数据归档为未压缩 `data-before.tar`（644,608,000 字节，另存摘要）；`config-before.tgz` 包含原环境、Compose 与受保护密钥。源码归档、迁移前后指纹、演练及部署回执也保存在该目录；不复制真实数据到源码仓库。
- 配置仅更换镜像引用。其余环境、三份 Compose、密钥与既有签名更新文件逐项核对未变；nginx 未修改，`nginx -t` 通过。容器 healthy、以 uid 10001 运行，HTTP 仍只映射到回环地址，密钥继续只读挂载，镜像内 ffprobe 可用。
- 正式 HTTPS 的健康、就绪、管理端、影视、登录与下载页均为 200；两个未登录预览入口为 401 并禁止缓存。正式 Chrome 用已有会话核对“预览”、现有安装文件的不支持提示、下载退路及关闭后的原行焦点；没有创建正式测试账户或上传测试文件。支持格式的播放证据仍是本机隔离测试，详见[预览手册](file-preview.md)。
- 本次有两次停服尝试：第一次把 `compose exec` 的命令写法误用于 `compose run`，程序名重复，自检在迁移前拒绝；保护脚本恢复原库和旧镜像，原服务恢复健康。修正后先用同一 Compose 调用验证了副本挂载。第二次从停止旧容器到新容器 healthy 为 24 秒；首次就绪探测遇到连接重置，监测脚本提前退出，但新版容器已经正常启动。之后只做向前核验，没有在对外重开后恢复旧数据；最终完整复核通过。首次尝试材料以 `attempt1-` 保留，未覆盖。

本次未修改 iOS、APNs 或推送配置，没有发布桌面签名更新或安装客户端。旧签名更新保持不变；桌面 Mac WebView 仍待实际验收。下一步仅规划 [N2 云盘文件夹](../plans/2026-11-27-drive-folders.md)，未提前执行目录迁移。

## 2026-11-27 停用旧 Jarvis

用户在日程迁移完成后明确要求停止 `movie.yxswy.com` 的跳转、停止服务器上的 Jarvis，只使用 Assemble。已执行：

- `movie.yxswy.com` 与 `jarvis.yxswy.com` 的 HTTP/HTTPS 业务入口均返回 **410 Gone**，不再发送 `Location`，也不会落入其他站点的默认虚拟主机。保留原 TLS 证书及 HTTP ACME 校验路径；DNS 没有改动。
- `systemctl disable --now jarvis.service`；复核为 `inactive`、`disabled`、`MainPID=0`，原 `127.0.0.1:3300` 无监听，没有定时器或 socket 触发单元。
- 只修改上述两个域名的 nginx 文件；其余 12 份主配置/站点配置摘要相同。修改前后 `nginx -t` 通过，之后 reload；原有其他域名的重复 MIME/主机名警告未随本次扩大处理。
- 从本机公网验证两个域名、两种协议、五类旧路径共 20 项均为 410 且无跳转；Assemble 的健康、就绪、登录、管理端、影视五个入口均 200，正式容器 healthy。

旧程序、数据库、存储、证书和已有备份均保留；没有删除旧账户数据，也没有修改 `R:/jarvis` 源仓库。本次原始站点配置、服务单元和状态保存在 `/var/backups/assemble/20261127-retire-jarvis-mjjcad1r/`（目录 0700）；`manifest.json`、`applied.json` 与 nginx 校验日志记录前后状态。恢复时先核对保留的数据和程序，再按备份恢复相应 nginx 文件并校验/reload，最后明确启用 Jarvis；日常维护不自动恢复旧服务。

## 2026-11-27 日程迁移补丁

日程迁移补丁当时部署为 `assemble-server:planner-prod-20261127`，镜像标识 `sha256:ab68b55f113733889d2ab2b4750d5a94bcbe7a540f49f6222ca77af8947a291b`。它以当时实际线上 `873131d75607` 为基础，只加入日程导入和受控地址读取；没有混入共享工作区里尚在修改的远程控制、影视公开和下载发布页功能。镜像在本机构建后经 SSH 传入，未发布 GHCR。

停服后保存最终一致库，再事务导入并核对后启动。备份与源导出保存在 `/var/backups/assemble/20261127-planner-z9fzt62m/`，目录 0700，备份及环境副本 0600；只读导入挂载属于容器专用 UID，外层受限备份目录仍属 root。包含 `final-before.sqlite3`、`before.sqlite3`、原镜像引用、原环境副本、来源摘要及字段核对报告。源 Jarvis 业务导出在迁移前后摘要一致，源站健康。

实际新增 4 条日程、5 条提醒、13 条私人备忘录，55 个地址绑定；目标原有 1 条日程、1 条通知保留。339 项逐字段及原记录核对、数据库完整性/外键检查通过；重复维护导入返回 `alreadyImported=true`。正式容器健康，HTTPS 健康/就绪、登录、影视入口正常，匿名导航接口 401；所有环境配置除镜像引用外相同，密钥目录、Compose 文件、nginx 均未改动。

本次账本是 `1…11,13`：待发影视迁移 `0012` 没有顺带执行。SQLx 后续可正常补上它，新增的兼容回归已在 Linux 真实迁移器验证。原镜像不能直接重开含 `0013` 的库；服务重新接受写入后须保全最新数据向前修复，不能用旧备份覆盖。桌面地址交互已完成代码和测试，但本次没有替换用户的安装版、发布桌面更新、提交或推送共享工作区。操作规则见[日程迁移手册](planner-migration.md)。

正式服务在 2026-11-24 第一次部署上线：<https://as.yxswy.com>。下面"第一次部署"写的就是照着做过的步骤；哪些真的验过、哪些还没有，分别在最后两节里如实列出。用的是 Docker Compose 加 nginx。

此前发布（2026-11-27 11:24）：桌面端自动更新的服务端这一半（清单与更新包两个只读接口、`publish-update` 子命令，[决策 0015](../decisions/0015-desktop-auto-update.md)）已部署为 `ghcr.io/puzzle-fuzzy/assemble-server:873131d75607`。镜像经 `deploy/push-image.sh` 传入。升级前核对：库里迁移 1～11 的校验和与仓库逐个一致，本次没有新迁移。停服务到健康 29 秒，启动日志只有"监听"两行；每张表的行数前后一致；自检通过，容器以 uid 10001 运行；`/movies/`、`/admin/`、`/web/register`、`/healthz` 都是 200，`/updates/latest.json` 是 404 且 `cache-control: no-store`（还没有发布过）。完整数据备份在 `/var/backups/assemble/20261127-112345-updater-873131d75607/`（`data-before.tgz` 566,452,122 字节、`config-before.tgz`、`env-before`、`counts-before.txt` 与 `counts-after.txt`，目录 0700、文件 0600）。**这一次升级停机两次**：第一遍脚本在停服务之后出错（服务器上的 Python 不允许 f-string 里有反斜杠），服务停了约 25 秒后被用旧镜像起回来；改好脚本、先在不停服务的情况下试过那一段之后，第二遍才成功。教训写在下面"升级"一节的末尾。

上一次发布（2026-11-27 09:51）：管理端影视详情、封面预览与失败占位同步已部署为 `ghcr.io/puzzle-fuzzy/assemble-server:28c9551fc6bf`。完整本机隔离验证、macOS CI 和镜像启动通过；发布提交的 Windows 在界面测试、构建和静态检查通过后因后续推送取消，未将其记作全套成功。迁移 1～11 校验一致、无新迁移，升级前后所核对的业务表计数一致；完整数据备份位于 `/var/backups/assemble/20261127-095117-media-28c9551fc6bf/`，目录 0700、归档 0600。服务健康，HTTPS 管理端样式资源已更新。公开影视页 Lighthouse 移动三次 98、桌面三次 100，其他三项均 100，CLS/TBT 均为 0。证据及登录验收范围见[同步记录](../reports/2026-11-27-admin-media-style.md)。

上一次发布（2026-11-27 02:48）：手机第一期里设备的平台值从 `iphone` 改成 `ios`（按操作系统命名），已部署为 `ghcr.io/puzzle-fuzzy/assemble-server:8986d0a232a6`。[macOS 与 Windows 验证](https://github.com/puzzle-fuzzy/Assemble/actions/runs/45876060043)与[镜像构建](https://github.com/puzzle-fuzzy/Assemble/actions/runs/45876059828)全部成功。镜像经 `deploy/push-image.sh` 传入。升级前核对：库里迁移 1～11 的校验和与仓库逐个一致，本次没有新迁移；设备的平台只有 macos 2 台、windows 1 台，没有存过 `iphone`。备份在 `/var/backups/assemble/20261127-024813-ios-platform-8986d0a232a6/`（`data-before.tgz` 563,058,907 字节、`config-before.tgz`、`env-before`、`counts-before.txt`；目录 0700、文件 0600），旧镜像保留。只改了 `.env` 的镜像引用，三个 Compose 文件、密钥目录和共用 nginx 都没有碰。从停服务到新容器健康共 28 秒；新容器健康、以专用账户运行、密钥只读挂载，启动日志干净，`check-config` 通过（按来源的限额、全站总量上限、SMTP 与 GitHub 配置都在）。升级前后每一张表的行数逐项一致。正式 HTTPS：健康与就绪 200、登录页 200、配对页 303 到登录页、影视页 200、设备接口 401；两个配对接口对空请求回 400（在，不是 404）。手机应用当时还没有装到任何一台真机上，平台值的改名不涉及已有数据。

上一次发布（2026-11-27 01:32）：影视翻页、详情预览与性能调整已部署为 `ghcr.io/puzzle-fuzzy/assemble-server:c79edf032e08`，双平台验证、镜像构建和隔离容器运行通过。备份位于 `/var/backups/assemble/20261127-013204-media-c79edf032e08/`，旧迁移校验一致，启动新增迁移 11，原账户和业务计数不变；共用 nginx 与密钥配置保持原样。正式 Lighthouse 移动三次 98、桌面三次 100，其他三类均 100，CLS 为 0。完整发布、备份权限和证据边界见[本轮记录](../reports/2026-11-27-media-web-performance.md#正式发布与-https-复测)。

## 材料在 `deploy/` 里

| 文件                   | 用途                                                                                                                    |
| ---------------------- | ----------------------------------------------------------------------------------------------------------------------- |
| `Dockerfile`           | Bun 构建共享 UI 管理端、官方 Rust 编译服务端、Debian slim 运行。Rust 版本与工具链一致，信号直接送到专用账户运行的服务端 |
| `compose.yaml`         | 一个服务。端口只交给本机的反向代理，日志自动轮转。镜像与数据的位置由 `.env` 决定                                        |
| `compose.build.yaml`   | 只在本机自己构建镜像时叠加到 `compose.yaml` 上                                                                          |
| `compose.secrets.yaml` | 密钥文件部署时叠加，将 secrets/ 只读挂载进容器；目录缺失则拒绝启动                                                      |
| `compose.dht.yaml`     | 明确启用公网 DHT 时叠加，单独映射 IPv4 UDP 6882；HTTP 仍只交给本机代理                                                  |
| `.env.example`         | 配置模板：镜像、数据位置、对外地址必填，其余都可以不填                                                                  |
| `nginx.conf.example`   | 反向代理：TLS、上传的上限、事件流的长连接。与正式服务器上的那一份只差域名                                               |
| `push-image.sh`        | 把持续集成构建的镜像经 SSH 送到服务器上。服务器直接拉 GitHub 的镜像仓库会握手超时，所以要它                             |
| `push-update.sh`       | 把发布用的那台 Mac 上做好的桌面更新目录经 SSH 送到服务器上，交给 `publish-update` 校验或发布，然后删掉临时目录          |
| `updater.pubkey`       | 给桌面更新包验签的公钥，编进服务端和桌面应用。私钥只在发布用的那台 Mac 上                                               |

SQLite 静态编入服务端，钥匙串依赖只在 macOS 与 Windows 上进入构建。运行镜像另带 FFmpeg/ffprobe 做受限媒体探测，体积以实际构建结果为准。

## 当前账户与邮件配置

管理端随服务端镜像同源发布于 `/admin/`，不另外修改共享 nginx。新增迁移 `0004_admin.sql` 前须按本文升级步骤备份并核对已有迁移；不能再次清空已经重建的账号。超级管理员指定与本人验证步骤见[管理端手册](admin.md)，真实邮箱仅写受保护 `.env`。

2026-11-25 用户纠正为公开邮箱注册，邀请码限制已经撤销。新代码需配置 SMTP，GitHub 登录另需 OAuth App；配置、旧账户过渡与验收见[账户配置手册](email-and-github.md)。本文后半的既往升级记录保留当时情况，不代表当前注册规则。2026-11-25 已部署 `be875da9899a`，SMTP、GitHub 与 callback 日志保护均已启用；部署前后都通过 `nginx -t`，详见当天的第四次升级记录。

## 第一次部署

1. **域名。** 把域名的解析指到服务器。DNS 要你本人操作，做完之后用公共解析服务确认一下已经生效。
2. **服务器上要先有的。** Docker（含 compose 插件）、nginx、certbot。
3. **配置。** 建一个目录放 compose，再建数据目录。数据目录要属于容器里的那个账户（编号 10001），否则服务端写不进去：

   ```sh
   install -d -m 0755 /opt/assemble
   install -d -o 10001 -g 10001 -m 0700 /var/lib/assemble
   ```

   把 `deploy/compose.yaml` 放进 `/opt/assemble`，`cp .env.example .env`，填上四项：
   - `ASSEMBLE_IMAGE`：持续集成构建出来的镜像，按提交号的前 12 位钉住一个版本。
   - `ASSEMBLE_DATA_PATH=/var/lib/assemble`。
   - `ASSEMBLE_PUBLIC_ORIGIN=https://<域名>`，必须与浏览器实际访问的完全一致。
   - `ASSEMBLE_DRIVE_QUOTA_BYTES`：默认 2 GiB；磁盘紧的服务器按余量调小。

   填完先确认 compose 认得：`cd /opt/assemble && docker compose config --quiet`。
4. **镜像与服务。** 镜像由持续集成构建（`.github/workflows/server.yml`，推送到 `main` 或打标签时触发），只构建 linux/amd64。

   服务器能访问 GitHub 的镜像仓库的话，`docker compose pull`；不能的话，在一台能访问的开发机上运行 `deploy/push-image.sh <标记>`，它把镜像拉下来经 SSH 传过去。

   然后：

   ```sh
   cd /opt/assemble
   docker compose up -d
   docker compose ps                      # 一两秒之后是 healthy
   curl -fsS http://127.0.0.1:8787/healthz    # {"status":"ok"}
   ```

5. **反向代理，分两步。** 先只放 `nginx.conf.example` 里那个 HTTP 的 `server`（证书验证的目录按你的改），`nginx -t` 通过后 `systemctl reload nginx`；用它申请证书：

   ```sh
   certbot certonly --webroot -w /var/www/assemble-acme -d <域名> -n
   ```

   再把完整的那一份放上去，换成你的域名与证书路径，`nginx -t` 通过后重载。别忘了续期的钩子：证书自动续期之后要让 nginx 重新加载，否则到期的还是旧证书：

   ```sh
   # /etc/letsencrypt/renewal-hooks/deploy/70-assemble-nginx.sh，记得 chmod +x
   #!/bin/sh
   set -eu
   if [ "${RENEWED_LINEAGE:-}" = /etc/letsencrypt/live/<域名> ]; then
       nginx -t
       systemctl reload nginx
   fi
   ```

6. **自检。**

   ```sh
   docker compose exec -T assemble assemble-server check-config    # 最后一行是账户数
   curl -fsS https://<域名>/healthz                                # 经反向代理也要应答
   curl -s -o /dev/null -w '%{http_code}\n' https://<域名>/v1/devices   # 401：没带凭据就被拒
   ```

   注意 `docker compose exec` 后面要带可执行文件的名字：`assemble assemble-server <子命令>`，第一个 `assemble` 是服务名，第二个是程序名。
7. **注册第一个账户。** 配好 SMTP 后由本人在 `/web/register` 注册、收信验证并设置密码。下列运维命令仍保留，但创建的旧式账户需到 `/web/legacy` 验证原密码并绑定邮箱：

   ```sh
   ssh -t <服务器> 'cd /opt/assemble && docker compose exec assemble assemble-server create-account <用户名>'
   ```

8. **桌面端指向正式地址。** 桌面端连哪个服务在构建时定下。给 GitHub 仓库设变量 `ASSEMBLE_SERVER_ORIGIN=https://<域名>`，再手动运行"安装包"流程（见[运行手册](verify-macos-launch.md)的"安装包从哪来"）。
9. **验收。** 两台设备各登录一次，互发一个文件，再看一眼云盘。

## 日常

```sh
cd /opt/assemble
docker compose ps                      # 健康状态
docker compose logs --since 1h         # 日志
docker compose exec -T assemble assemble-server list-accounts
```

某台设备不用了，在桌面应用的"设备"卡片里移除它：它的令牌随即作废。

忘了某个账户的密码，或者想让它所有的网页登录状态作废：

```sh
docker compose exec assemble assemble-server set-password <用户名>   # 密码在终端里输两遍
```

某个账户出了事（比如在往磁盘里灌东西），按邮箱停用它。之后它哪里都登不进去，电脑与助手上的 Assemble 被拒绝；数据留着，所以不会自己腾出空间，但它再也放不进新的东西：

```sh
docker compose exec -T assemble assemble-server suspend-account <邮箱>
docker compose exec -T assemble assemble-server resume-account <邮箱>   # 恢复：设备照旧能用，网页要重新登录
```

停用、恢复的状态变化会写审计记录；重复操作不重复记账。当前源码的管理端用户详情也提供确认后的停用、恢复，不能停用自己；网页操作会立即通知该账户现有事件流结束。独立 CLI 无法通知服务进程内存，现有流在下一次心跳检查时结束。两种入口都撤销被停用账户的网页会话，数据保留；恢复后网页要重新登录。

### 按来源的限额与总量上限

两件事都在 `.env` 里设，改完 `docker compose up -d`，再用 `check-config` 看一眼：

```sh
#   ASSEMBLE_CLIENT_IP_HEADER=X-Real-IP          # 例子里 nginx 写来源地址的头；不填则按来源的限额不生效
#   ASSEMBLE_STORAGE_BUDGET_BYTES=8589934592     # 全站的内容总量（这里是 8 GiB），按磁盘留给 Assemble 的余量填
#   ASSEMBLE_STORAGE_WARN_PERCENT=80            # 预算预警线，1～100 的整数；默认 80
docker compose exec -T assemble assemble-server check-config
#   报告里应有"全站内容总量上限"和"请求来源：按来源的限额读 x-real-ip 头"
```

总量上限管的是云盘和还没送达的发送加起来的字节数，到了就一律拒绝上传与发送。每个账户仍有自己的配额，可注册是公开的，只按账户设限攒起来仍能把磁盘写满——而这台服务器上还有别的站点。

当前源码的 `check-config` 另报文件内容已用、数据库（主文件、WAL、SHM）、数据卷可用空间和预警线。管理概览显示同一口径和前十账户用量；内容达到预警线或可用空间低于 2 GiB 时显示预警。维护日志每进程一小时最多一条容量或测量异常提示，不发邮件、不自动删除文件。无法测量的值显示暂不可用，不能按零占用处理。

升级后将数据卷可用空间与宿主 `df -B1 <数据目录>` 对照；数据库用主文件及当前 `-wal`、`-shm` 的文件长度之和核对。SQLite 日志会变化，检查应尽量在同一时刻。`du` 报物理分配块，不能与按声明字节和摘要去重的内容预算直接画等号；影视图片存于数据库中，单独占用磁盘。前十账户包含云盘和待接收发送，每账户内部去重，跨账户不能求和代替全站用量。

达到预警时先确认同卷其他站点和数据库的增长，预留预算之外的空间，再按服务器既有维护流程扩盘或提高预算；不要直接删除 `blobs/` 或数据库文件。正式阈值只写受保护 `.env`，本轮实现没有改变正式服务配置或执行扩盘。运营接口和空值约定见[管理端手册](admin.md)。

### 公开邮箱注册

任何人可从 `/web/register` 申请邮件；验证后设置密码并建立账户，不再读取 `ASSEMBLE_INVITE_CODE`。SMTP、GitHub、网页找回密码与旧账户绑定步骤见[账户配置手册](email-and-github.md)。

### 升级

```sh
# 开发机上：把新的镜像送到服务器
deploy/push-image.sh <新的标记>
# 服务器上：先备份（见下），再换掉 .env 里的 ASSEMBLE_IMAGE，重新起来
cd /opt/assemble && cp -p .env .env.before-<新的标记> \
  && sed -i 's|^ASSEMBLE_IMAGE=.*|ASSEMBLE_IMAGE=ghcr.io/puzzle-fuzzy/assemble-server:<新的标记>|' .env \
  && docker compose up -d && docker compose ps
docker compose logs --since 5m          # 启动日志里不该有迁移报错
```

**升级之前先核对一件事：正式库里记着的迁移校验和，和要上的那一版脚本对不对得上。** 对不上，新版本一启动就会拒绝运行（见下面"做过的"里 2026-11-25 那一段的来由）。服务器上没有 `sqlite3` 命令，用系统自带的 Python 以只读方式看一眼，不碰服务：

```sh
python3 - <<'SCRIPT'
import sqlite3
con = sqlite3.connect("file:/var/lib/assemble/assemble.sqlite3?mode=ro", uri=True)
for version, checksum, success in con.execute("select version, checksum, success from _sqlx_migrations order by version"):
    print(version, checksum.hex() if isinstance(checksum, bytes) else checksum, success)
SCRIPT
```

开发机上算出每个脚本应有的值来比：`python3 -c "import hashlib;print(hashlib.sha384(open('apps/server/migrations/0001_initial.sql','rb').read()).hexdigest())"`。已经记录在库里的那几个版本，必须逐一相符；新的脚本还不在这张表里，是启动时才加进去的。

数据库结构在启动时自动迁移。迁移只向前，所以升级之前先备份一份；要退回旧版本，得同时恢复那份备份，并把 `ASSEMBLE_IMAGE` 换回原来的标记（换 `.env` 之前留的那份副本还在）。

备份和换版本可以连着做，只停一次：停服务、打包、换标记、`docker compose up -d`（不必在打包之后先 `start` 再停第二回）。2026-11-25 的第三次升级就是这样做的，停机 1 秒。

**把要在服务器上跑的脚本先写成一个文件，停服务之前的每一段都在不停服务的时候先试一遍。** 2026-11-27 升级 `873131d75607` 时，脚本在 `docker compose stop` 之后才出错，服务白白停了约 25 秒，只能先用旧镜像起回来再来一遍。`set -e` 的脚本里任何一步都可能出错，所以"停服务"要排在能出错的步骤之后，行数统计这类能先跑的先跑。

**镜像的构建上下文是白名单（`.dockerignore`），编译时要读的文件得在里面。** 同一天，公钥 `deploy/updater.pubkey` 被编进 `assemble-protocol`（`include_str!`），可它不在白名单里，"服务端镜像"从 `406eab0` 起连着三次都红，当时没有人看：`bun run verify` 不构建镜像，只有那个流程会报。现在 `Dockerfile` 里有 `COPY deploy/updater.pubkey`，`.dockerignore` 里放行了它。推送之后三个流程都要看一眼："验证"的两栏、"服务端镜像"，触发过的"安装包"。

### 升级命令的入口与探测

下列命令名仅说明入口；操作正式服务时沿用正在运行容器的全部 Compose 叠加文件。预演必须将数据卷替换为副本。

`docker compose run --rm assemble check-config` 使用镜像已定义的程序入口，只传子命令；`docker compose exec -T assemble assemble-server check-config` 在现有容器启动程序，必须带可执行文件名。不要混用。`check-config` 会打开并迁移数据库，服务仍在写入时只能挂载副本验证，不能提前对正式库执行新版本迁移。

切换脚本使用的确切 Compose 命令、镜像与挂载也要在副本预演，不能只验证等价的裸 Docker 命令。新容器刚重建时，连接重置、暂时拒绝连接等应在有限期限内重试；必须同时通过就绪探测和容器健康检查。对外接受请求后保全最新数据、向前修复，不因监测脚本报错就恢复旧库。

### 发布桌面端的更新

桌面应用的更新包由这台服务端托管。`GET /updates/latest.json` 是清单，`GET /updates/<平台>/<版本>/<文件>` 是更新包；两个都是谁都能读、按来源限速的接口（清单每个来源每小时 120 次，更新包 30 次）。为什么这样做见[决策 0015](../decisions/0015-desktop-auto-update.md)。

平时只在发布用的那台 Mac 上跑一条命令，它把下面这几步连起来做完（`scripts/release/publish.ts`，为什么这样做见决策 0015）：

```sh
bun run release:publish             # 取 Windows 安装包、构建并签名 Mac 应用、给两个包签名、送到服务器上只校验
bun run release:publish --apply     # 同上，服务器上校验通过就发布
```

它要求：工作树是干净的（发出去的是提交）；"安装包"流程已经为当前这个提交成功地跑过——Windows 的安装包只由它出，所以先在 GitHub 上手动运行那个流程，或者推一个 `v<版本>` 的标签；钥匙串里有本地证书（[运行手册](verify-macos-launch.md)的"本地签名"）。版本是 `0.1.<提交数>`。最后一步是 `deploy/push-update.sh <发布目录> [--apply]`：把发布目录传进数据目录下的一个临时目录，交给下面这两条命令，然后删掉临时目录。

也可以手工做。发布用子命令，不开写的接口。把一个发布目录放进数据目录（属主得是 10001），先只校验，再发布：

```sh
docker compose exec -T assemble assemble-server publish-update /var/lib/assemble/<发布目录>            # 只校验
docker compose exec -T assemble assemble-server publish-update /var/lib/assemble/<发布目录> --apply    # 发布
```

发布目录里有一份 `manifest.json`，和清单里列出的每个更新包连同它的 `.sig` 文件，不能有别的文件：

```json
{
  "version": "0.1.116",
  "notes": "Assemble 0.1.116",
  "pubDate": "2026-11-27T10:30:00.000Z",
  "platforms": [
    { "platform": "windows-x86_64", "file": "Assemble_0.1.116_x64-setup.exe" },
    {
      "platform": "darwin-aarch64",
      "file": "Assemble_0.1.116_aarch64.app.tar.gz"
    }
  ]
}
```

核对不过时退出码不是 0，标准错误上一行 `UPDATE_PUBLISH_FAILED: <错误码>: <原因>`。常见的几种：

| 错误码                           | 意思                                                                                     |
| -------------------------------- | ---------------------------------------------------------------------------------------- |
| `UPDATE_SIGNATURE_INVALID`       | 包不是那把钥匙签的，或者签完之后被改过                                                   |
| `UPDATE_SIGNED_VERSION_MISMATCH` | 签名不是为清单上的版本签的：签名时要带上版本（`tauri signer sign --app-version <版本>`） |
| `UPDATE_VERSION_NOT_NEWER`       | 不比已经发布的新                                                                         |
| `UPDATE_PLATFORM_MISSING`        | 已经发布过的平台，这一版里没有它                                                         |
| `UPDATE_UNEXPECTED_FILE`         | 目录里有多余的文件                                                                       |

核对通过时标准输出是一份 JSON 报告，`applied` 说明这一回有没有真的发布。先校验再发布，中间不要换目录里的东西。

发布之后：更新包在数据目录的 `updates/<平台>/<版本>/` 下，清单是 `updates/latest.json`；每个平台只留最新的一版，旧版本的目录被删掉。它们不在 `blobs/` 里，不计入云盘预算，备份时随数据目录一起打包；恢复演练目前只核对 `blobs/`，这一部分还没有演练过。

公钥是仓库里的 `deploy/updater.pubkey`，编进服务端和桌面应用。私钥只在发布用的那台 Mac 上（`~/.assemble/updater.key`，没有口令），不进仓库、日志、界面和持续集成；脚本只把它的路径交给 `tauri signer sign`。丢了就再也发不出更新，装出去的应用只能重装，所以请另留一份只有你自己能读的备份。重做一对钥匙要同时换掉编进两边的公钥，并且先手动给所有人装一次带着新公钥的版本——所以它救不了已经装出去的应用。

**2026-11-27 11:39 第一次发布：`0.1.117`。** 先只校验地跑了一遍，再 `--apply`：取的是"安装包"流程为 `873131d` 跑出的 Windows 安装包，Mac 应用在那台 Mac 上构建并用本地证书签名，两个包用私钥签名后经 `deploy/push-update.sh` 传到服务器，服务器用编进去的公钥验签通过；清单在 `https://as.yxswy.com/updates/latest.json`（`cache-control: public, no-cache`，带 ETag），更新包支持按范围取（206）、长缓存。当时主干已经往前走了（另一个会话合并了一批新功能），所以是在单独检出的 `873131d` 里跑的。发出去的版本号只要求比已经发布的大，不要求是主干的最新提交。装出去的应用要等各自的下一次检查才会发现它（启动 15 秒后一次，之后每 6 小时一次，托盘里"检查更新"是手动的一次）。

发布目录由 `scripts/release/stage.ts` 做（`publish.ts` 会调它）：Mac 的应用包打成 `Assemble_<版本>_aarch64.app.tar.gz`，顶层就是 `Assemble.app`，这是 Tauri 更新插件要的样子；两个包各用 `--app-version <版本>` 签一次；只打应用包本身，不带 macOS 的元数据文件。发布用的 Mac 应用包必须是本地那张证书签的，脚本会认一遍：换了证书，更新之后系统就不认得它，会再问一次钥匙串。

### 备份与恢复

SQLite 开着写的时候直接拷文件可能不一致，所以备份时停几十秒：

```sh
cd /opt/assemble
install -d -m 0700 /var/backups/assemble
docker compose stop
tar czf "/var/backups/assemble/$(date +%F-%H%M).tgz" -C /var/lib/assemble .
docker compose start
```

备份里有：账户（密码的散列，没有明文）、设备令牌的摘要、发送记录、云盘与未送达的文件。令牌的摘要不能拿去登录，但云盘里的文件本身是想给谁看就给谁看的东西，备份要当敏感数据保管。

恢复是反过来：停服务，把归档解进数据目录（属主要是 10001），再启动。备份在 2026-11-25 真的做过多次；恢复在 2026-11-26 第一次演练过——在隔离的一次性容器里，不碰正式服务，做法见下面"恢复演练"。把恢复出来的数据真的换到正式位置上（连同换回旧镜像）还没有做过。

### 恢复演练

不必等出了事才知道备份能不能用。隔一阵就这样演练一次：在服务器上另起一个不接网络、不发布端口的容器，把备份恢复进去，看它能不能被同一个镜像打开、能不能启动。正式服务全程不受影响。

```sh
# 服务器上。备份目录只有 root 能进；image 换成现在在跑的那一个。
image=$(grep '^ASSEMBLE_IMAGE=' /opt/assemble/.env | cut -d= -f2)
drill=$(mktemp -d /var/backups/assemble/restore-drill-XXXXXX)
cleanup() { docker rm -f assemble-restore-drill >/dev/null 2>&1 || true; rm -rf "$drill"; }
trap cleanup EXIT
mkdir "$drill/data"
tar xzf /var/backups/assemble/<某一次备份>/data.tgz -C "$drill/data"
chown -R 10001:10001 "$drill/data" && chmod 700 "$drill/data"

# 一、用同一个镜像打开恢复出来的数据并自检。入口已经是那个程序，参数只写子命令。
docker run --rm --network none -v "$drill/data":/var/lib/assemble \
  -e ASSEMBLE_PUBLIC_ORIGIN=https://<域名> "$image" check-config

# 二、真的启动，在容器里面问它几句（镜像里没有 curl，用 bash 自己的 /dev/tcp）。
docker run -d --name assemble-restore-drill --network none -v "$drill/data":/var/lib/assemble \
  -e ASSEMBLE_PUBLIC_ORIGIN=https://<域名> "$image"
sleep 5
docker exec assemble-restore-drill bash -ec "exec 3<>/dev/tcp/127.0.0.1/8787; \
  printf 'GET /readyz HTTP/1.1\r\nHost: <域名>\r\nConnection: close\r\n\r\n' >&3; head -n 1 <&3"
docker logs assemble-restore-drill

# 三、核对：库里的每一份内容在磁盘上都在、大小都对；各项计数与备份前一致。
python3 - "$drill/data" <<'SCRIPT'
import sqlite3, os, sys
root = sys.argv[1]
con = sqlite3.connect(f"file:{root}/assemble.sqlite3?mode=ro", uri=True)
blobs = con.execute("select sha256, size from blobs").fetchall()
def path(sha): return os.path.join(root, "blobs", sha[:2], sha)
print("缺", sum(not os.path.isfile(path(sha)) for sha, _ in blobs),
      "大小不对", sum(os.path.isfile(path(sha)) and os.path.getsize(path(sha)) != size for sha, size in blobs),
      "共", len(blobs))
for table in ("accounts", "devices", "drive_files", "schedules", "reminders"):
    print(table, con.execute(f"select count(*) from {table}").fetchone()[0])
SCRIPT

# 四、收尾：上面的 trap 会删掉演练容器和临时目录。看一眼确实没有残留。
docker ps -a | grep restore-drill; ls -d /var/backups/assemble/restore-drill-*
```

要核对的东西：自检的退出码是 0、报告里的账户数对；`/readyz` 回 200、不带凭据的接口回 401、启动日志只有"监听"；缺 0、大小不对 0；计数和备份之前记下的（升级脚本存进备份目录的 `counts-before.txt`）逐项一致。演练容器没有 SMTP 与 GitHub 的配置，所以它发不出信、也连不上外面；正式的密钥目录没有挂进去。这一步验的是"归档完整、能被这一版镜像打开"，验不了登录——令牌只存摘要，密码在用户手里。

## 反向代理要注意的

- **上传上限。** 单文件 100 MB，nginx 那边留到 110 MB。桌面端在挑文件时就拦下超过上限的，服务端也会拒绝，所以这道上限平时碰不到，它只是最后一道闸。
- **不缓冲请求体。** 服务端会提前拒绝超限或不合法的上传，并等发送方把话说完；nginx 边收边转，不在自己的磁盘上再存一份。
- **事件流。** `/v1/events` 是一条不结束的长连接，服务端每 15 秒发一次心跳；读超时要放宽，缓冲要关掉。
- **只映射到本机。** compose 里端口绑在 `127.0.0.1` 上。不要为了省事把它直接对外，那样会绕过 TLS。
- **服务端默认不读转发头。** 在它看来，所有请求都来自反向代理，日志里看到的对端都是 `127.0.0.1`。2026-11-25 起有一个显式的开关：在 `.env` 里写 `ASSEMBLE_CLIENT_IP_HEADER=X-Real-IP`（示例配置里的 nginx 就是把来源地址写在这个头里），服务端才按它区分请求的来源，而且只用来分限额——旁人锁不住你的账户，一个来源也用不掉全站的额度。它成立的条件是只有反向代理能连到服务端，所以端口必须继续只绑在本机。来源不进日志。
- **安全响应头。** CSP 与 Referrer-Policy 由服务端自己发；`X-Content-Type-Options` 与 HSTS 是 nginx 加的。

## 换成 systemd 的话

`deploy/` 里没有准备这一种。要做的话：用一个带静态链接目标（`x86_64-unknown-linux-musl`）的构建产出服务端可执行文件，加一个以专用账户运行、`Environment=` 指向同一份 `.env`、`Restart=on-failure` 的 unit 文件，反向代理的部分照旧。

## 做过的（2026-11-24）

- **本机的镜像。** 真的构建了三次才成，只做不依赖容器的检查时没发现三处问题：构建上下文里少了桌面端的清单与入口文件，cargo 解析不了工作区；建了专用账户却忘了写 `USER`，容器以 root 运行；手册里 `docker compose exec` 的命令少了可执行文件名。之后用 compose 把镜像跑了一遍：6 秒内健康、以专用账户运行、端口只绑回环地址、建账户与自检正常、停止 0.18 秒且退出码 0、重启后数据还在。
- **持续集成构建镜像。** linux/amd64 的镜像在 GitHub 上构建成功，公开可拉。
- **服务器上的服务。** 镜像经 SSH 传入（服务器直接拉 GitHub 的镜像仓库握手超时）。容器 6 秒内健康、以专用账户运行、只监听本机的 8787；数据目录属于 10001、仅本人可读写；自检里对外地址与云盘上限都对。
- **证书与反向代理。** 证书申请成功、自动续期已安排；nginx 的配置 `nginx -t` 通过，重载之后新站点应答、抽查的两个老站点不受影响。
- **从外网核对。** `/healthz` 应答；登录页 200 且证书有效；HTTP 入口 301 跳到 HTTPS；没带凭据的接口与事件流都是 401。

## 做过的（2026-11-25）：第一次升级

- **先核对，再动手。** 以只读方式看了正式库的 `_sqlx_migrations`：迁移 1 的校验和与仓库里的脚本相符。这一步是当天在本机撞出来的教训：本机的开发库是迁移 1 的第一版建的，那个脚本提交后二十分钟被原地改过，于是那个库再也打不开了——服务端一启动就报 `migration 1 was previously applied but has been modified`。正式库是用改完之后的那一版建的，不受影响。
- **送镜像的脚本第一次真跑就倒了。** `$image（linux/amd64）` 在 `/bin/sh` 里把全角括号的前几个字节当成了变量名的一部分，报 `unbound variable`。改成 `${image}` 之后，linux/amd64 的镜像经 SSH 传入成功（上一次部署是手工送的，这个脚本写完之后一直没跑过）。
- **备份。** 按上面的步骤停服务、打包、再启动：停机 1 秒，归档在 `/var/backups/assemble/`（当时库里还没有账户，只有 3 KB）。
- **换版本。** 留下 `.env` 的副本，换成新的标记，`docker compose up -d`。容器重建后 9 秒内健康；启动日志只有"监听"两行，没有迁移报错；库里多了迁移 2 和四张新表；仍然以专用账户（uid 10001）运行。
- **从外网核对。** `/healthz`、`/readyz` 应答；登录页 200；没带凭据的 `/v1/planner` 与事件流是 401；HTTP 入口 301 跳到 HTTPS。共用的 nginx 没有动，`nginx -t` 仍然通过。
- 核对时临时写的一段等待脚本写错了（bash 里没有 `${$(…)#…}` 这种写法）：容器已经起来了，检查却没跑完，重写之后才核对完。上面的步骤里不放这种东西：`docker compose ps` 看到 `(healthy)` 就行。

## 做过的（2026-11-25 上午）：第二次升级，带上邀请码注册与改密码

- **先核对，再动手。** 正式库记录的迁移没有变（这一次没有新的迁移脚本）；`.env` 里只有镜像、数据位置、对外地址与云盘上限，没有秘密。
- **备份。** 停机 1 秒，归档 `/var/backups/assemble/2026-11-25-1040.tgz`。这时库里已经有一个账户了，备份里带着它。
- **在服务器上生成邀请码。** 16 位小写字母与数字，在服务器上生成、直接写进 `.env`，没有经过任何人的屏幕或对话。`.env` 的权限从 644 改成 600：从这一刻起它是一份要保管的秘密。换之前的那份留在 `.env.before-bd7788e12421`（里面没有邀请码）。
- **换版本。** `docker compose up -d` 之后 6 秒内健康；启动日志只有"监听"两行；仍然以专用账户运行；账户还在。
- **从外网核对。** `/web/register` 应答，带着防伪 Cookie，页面上没有邀请码；登录页上有"注册"的入口；`/web/account` 没登录时跳去登录。不带 `Origin` 的提交是 403；配对齐全、来自本站但邀请码乱填的提交是 403"邀请码不对"，账户没有多出来；没登录就提交改密码是 403。日志里记下了这两次拒绝，没有回显提交上来的任何东西。
- 这一次是带着真实账户做的升级；当时恢复还没有演练过（2026-11-26 第一次演练，见文末）。

## 做过的（2026-11-25 上午）：第三次升级，带上授权页的修复

起因：你在正式服务上第一次给自己登录（`assemble-mcp login`），在授权页点"确认登录"没有任何反应。缘故与修法记在决策 0002 第 24 条：授权页自己的内容安全策略拦掉了跳回本机的那一步。修复是 `52aed64`，这一次上的是它后面的 `0b6d40993a5b`（再往后只多一个文档提交）。

- **先核对，再动手。** 正式库里记着的两个迁移的校验和，和仓库里的脚本逐一相符；这一次也没有新的迁移脚本。
- **送镜像。** `deploy/push-image.sh 0b6d40993a5b`：拉 linux/amd64、经 SSH 传入，一次就成。
- **备份与换版本连着做，只停一次。** 10:50:43 停服务，归档 `/var/backups/assemble/2026-11-25-1050.tgz`（12 KB），`.env` 留下 `.env.before-0b6d40993a5b`，换掉标记，`docker compose up -d`；10:50:44 健康检查就应答了，停机 1 秒。`.env` 的权限（仅本人可读写）和邀请码那一行原样没动。
- **换版本之后。** 容器 20 秒内显示健康，跑的镜像摘要与拉下来的那一个一致，以专用账户运行；启动日志只有"监听"两行；自检里账户数是 1，网页注册仍然"开放（要邀请码）"。
- **从外网核对。** `/healthz`、`/readyz`、登录页、注册页都是 200；没带凭据的 `/v1/planner`、`/v1/devices`、`/v1/events` 是 401；HTTP 入口 301 跳到 HTTPS。登录页的内容安全策略和以前一样（只认本站、没有脚本）；换掉的那一份只在授权页上，从外网看不到——它要登录之后才出现，是在真实浏览器里对着本机服务端核对的（见[运行手册](verify-macos-launch.md)"2026-11-25 做过的"）。

## 做过的（2026-11-25）：第四次升级，开放邮箱注册与 GitHub

- **部署前检查。** 迁移 1、2 的 SHA-384 与正式库逐项一致；迁移 3 保持原提交内容。本机实际容器使用 `compose.secrets.yaml` 通过自检、健康检查和网页检查，确认 uid 10001、密钥目录只读、未认证接口返回 401。桌面日程草稿跨账户残留用两条回归测试复现后修复；`bun run verify` 通过（138 项界面与脚本测试、450 项 Rust 测试）。[双平台 CI](https://github.com/puzzle-fuzzy/Assemble/actions/runs/45551280453) 和[镜像构建](https://github.com/puzzle-fuzzy/Assemble/actions/runs/45551280467) 均通过。
- **送镜像。** `deploy/push-image.sh be875da9899a` 经 Windows OpenSSH 传入 linux/amd64 镜像，服务器实际镜像与本机摘要一致。Git Bash 自带 ssh 未使用 Windows 的凭据上下文，所以本次仅为该命令优先使用系统 OpenSSH，不改全局 SSH 设置。
- **配置与备份。** 配置备份位于 `/var/backups/assemble/2026-11-25-130834-be875da9899a/`，包括旧 `.env`、Compose 和该站点的 nginx 配置。停止服务后生成 `data.tgz`（31,654 字节），保留账户、设备和云盘；切换后的配置与密钥另存为同目录下的 `identity-config.tgz`。备份目录 `0700`，环境与归档 `0600`；归档包含密钥，须按敏感配置保管。部署暂存区的密钥副本已清理。没有在正式环境创建临时账户。
- **密钥与公开注册。** SMTP 和 OAuth Secret 通过受保护文件传入，容器只读挂载 `/run/assemble-secrets`；文件属主 10001、权限 `0400`。`.env` 只有文件引用，不含这两项明文，旧邀请码配置已删除，后续 `docker compose` 通过 `COMPOSE_FILE` 保留挂载。
- **反向代理。** 只对 Assemble 的 GitHub callback 添加访问日志关闭和错误日志丢弃规则；原配置 SHA-256 在变更前复核，其他站点配置未改。`nginx -t` 前后均通过；用无效 callback 测试标记核对访问与错误日志，均未记录。原有其他站点的重名与 MIME 警告仍存在，未扩大范围处理。
- **切换结果。** 从停旧服务到新 `/readyz` 应答为 0.85 秒。容器健康、仍以专用账户运行，库内迁移为 1、2、3；原有账户、设备和云盘文件记录均各 1 条，前后相同，云盘上限仍是 1 GiB。
- **从外网与真实浏览器核对。** 健康、就绪、邮箱登录、公开注册、找回密码和旧账户绑定页面均可用；注册表单不含邀请码，登录只填邮箱密码。未登录账户页跳转登录，未认证的设备、日程、事件流接口仍为 401，缺少来源校验的注册提交为 403。GitHub 跳转指向正确的 HTTPS callback、只申请 `read:user user:email`，携带 S256 PKCE 和安全 Cookie；GitHub 本身应答跳转至登录页，没有 client id 或 callback 配置错误。真人授权尚需用户亲自完成。
- **SMTP 实际发送。** 正式服务经既有“找回或设置密码”入口向发信邮箱自身发送一封账户提示邮件，SMTP 传输成功、网页返回 200；未创建账户、未重置密码。是否实际进入收件箱需本人确认，不能把 SMTP 接受当成收件确认。
- **本机桌面端。** 安装并启动 `正式版本 - 0.1.0.be875da`，保留原登录与本机设置，连接正式服务正常。代码推送、桌面安装和正式服务部署都已完成；跨设备收发、用户邮箱绑定和 GitHub 真人授权仍单独验收。

## 2026-11-25：按用户要求重置正式账号

用户明确要求“把旧账户的什么的都删除，账号部分完全重新开始”，取代第四次升级时保留原账号的做法。本次只重置运行数据，服务镜像仍为 `be875da9899a`；SMTP、GitHub、站点、配额与本机设置均保留。

- 重置前：账号 1、网页会话 2、设备 1、云盘文件与内容各 1；另外有审计记录和邮件限流记录。未创建临时测试账户。
- 停止服务后，把原数据归档到 `/var/backups/assemble/2026-11-25-134318-account-reset/data-before-reset.tgz`。备份目录 `0700`，归档和原配置副本 `0600`；只作为受限人工恢复材料，不参与现有账号查询或登录。
- 重新创建属主 `10001:10001`、权限 `0700` 的正式数据目录，强制重建容器，由原有迁移 1、2、3 自动初始化空库；未手改表结构、未修改已提交迁移。初始化成功后删除原数据目录的临时副本，包括旧云盘内容。
- 账号、网页会话、设备、桌面授权码、文件内容、云盘、传输、日程、提醒、通知、幂等记录、GitHub 绑定、邮件凭据、限流和审计记录全部为 0。配置文件与密钥文件摘要在重置前后逐一一致，从停止旧服务到空库就绪为 1.75 秒。
- 桌面端真实状态已显示“这台设备的登录已失效，请重新登录”，并回到未登录界面；没有替客户端预置新会话。后续用户直接公开注册或 GitHub 登录，不再按旧账号补绑流程操作。
- 重置只针对正式服务中的账号及关联数据，不删除用户电脑上的原始文件或已接收文件。历史运维备份仍按受限备份管理，不会自动恢复旧账号。

## 还没有验证过的

2026-11-25 16:40 已更新到 `e6b975faa26a`，正式管理端中英与主题切换、刷新恢复及已有管理员数据已在真实浏览器核对；Windows 主窗口和发送面板也已实际安装、启动并核对。迁移、受保护配置和原数据保留，备份及 CI 见[本轮管理端验收](admin.md#2026-11-25中英与外观正式验收)。下方早期管理员待注册记录为当时状态。

2026-11-25 管理端升级：`123ae575c306` 已发布，迁移 1～4 与受保护配置均核对，容器健康；网页管理入口及静态资源 200，匿名接口 401。备份、CI 与 Windows 客户端记录见[管理端手册](admin.md#2026-11-25-正式发布)。此次没有再次重置账号，指定管理员仍须本人注册并验证邮箱。

- **两台真实设备互传、云盘操作的完整真人验收**：Windows 已保留登录并读到原云盘列表，不能替代完整的上传、取回与两设备收发。
- **经反向代理的大文件上传与事件流的长连接**。配置是按服务端的行为写的，没有对着真实设备跑过。
- **把恢复出来的数据真的换到正式位置上。** 2026-11-26 在隔离的一次性容器里演练过恢复：归档完整、能被同一版镜像打开并启动到就绪、计数与内容逐项一致（见"做过的"里当天那一段）。真正停服务、替换正式的数据目录、必要时换回旧镜像，还没有做过。
- **退回旧版本。** 步骤写了（恢复备份、换回标记），没有做过。
- **公开注册与邮箱变更的真人完整流程。** 账户是本人 2026-11-25 用 GitHub 注册的，GitHub 真人授权实际发生过；用邮箱收信验证注册、改密码、找回密码、给账户绑定 GitHub 这些，真人还没有逐项走过。入口、SMTP 真发送与防线已核对。
- **在途上传时停止服务的表现**：第四次升级已量出停旧服务到新服务就绪为 0.85 秒，但没有用真实上传压停止等待。
- **续期钩子**没有真的触发过（证书 2027-02 到期）。
- **装出去的应用自己换上新版，还没有亲眼看到**（2026-11-27）。`0.1.117` 已经发布（见"发布桌面端的更新"），这台 Mac 上是 `0.1.116`，要等它的下一次检查；Windows 上还是旧版，要先手动装带更新器的第一版。本机端到端和各项自动化测试见[决策 0015](../decisions/0015-desktop-auto-update.md)。另外：锁屏的时候应用照样会按"空闲"换上新版，换完之后要等人回来解锁才恢复登录（钥匙串锁着），这一条还没有遇到过。

## 2026-11-25：影视三端正式升级

18:33 将正式服务升级到 `ghcr.io/puzzle-fuzzy/assemble-server:b984a100b01f`。该源码已推送 `main`，macOS/Windows 完整验证与镜像流程均成功；本机完整验证为 242 条 TypeScript 与 476 条 Rust 测试，另有 1 条既有文档示例标记忽略。完整功能、权限和证据边界见[影视手册](media.md)。

- 升级前备份到 `/var/backups/assemble/2026-11-25-183346-media/`：完整数据归档、原 `.env`、Compose 及受限密钥材料。旧迁移 1～4 的校验和逐项核验，启动只追加 `0005_media.sql`；没有清空或重建账号。
- 账户、设备、云盘文件、网页会话记录在切换前后分别为 1、1、2、1，管理员权限保留。除镜像和新增来源文件引用外，其余环境、Compose 和已有密钥摘要一致；容器以 uid 10001 运行并达到 healthy。
- 两个网页同源发布：[影视维护](https://as.yxswy.com/admin/#media)、[影视展示站](https://as.yxswy.com/movies/)。真实浏览器已查看已登录维护页、检索 TMDB 影片，以及展示站和真实海报详情；没有在正式库创建测试条目、账号或磁力。未认证的管理与私人接口为 401，已知深链接为 200，缺失静态资源为 404。
- 按用户授权只复制了 Jarvis 受保护的 TMDB 配置，未改 Jarvis 文件、业务数据或站点。主机直连在部署前曾超时；正式容器启用后，TMDB 与豆瓣的实际公网资料、海报请求均为 200，没有新增代理。
- 沿用既有 nginx，切换前后 `nginx -t` 均通过；其他站点已有的重名与 MIME 警告未扩大处理。镜像经既有 SSH 传送脚本送达，没有要求服务器直拉 GitHub 镜像仓库。
- Windows 正式来源的 NSIS 安装包已构建并单独交付；原生页面验收使用隔离预览，未替换正在运行的安装版或中断另一会话的下载调试。真人维护写入、跨设备影视更新与带影视数据的备份恢复尚未验收，不由页面可读和夹具测试替代。

## 做过的（2026-11-26 凌晨）：补上限额与停用账户

起因是一次只读审查（决策 0002 第 25 条）：公开注册之后，几处"全站共用一份"的额度谁都能用光，云盘也只按账户设限。代码是 `76b925a`，先升到它，再因为一处回归升到 `581d1832e009`。

- **第一次（00:17，`76b925af6ca4`）。** 升级前核对：迁移 1～5 的校验和与仓库逐项相符；记下账户、设备、云盘文件、内容、会话、GitHub 绑定、日程、提醒的计数。停服务、打包（78 MB）、换标记、往 `.env` 追加 `ASSEMBLE_CLIENT_IP_HEADER=X-Real-IP` 与 `ASSEMBLE_STORAGE_BUDGET_BYTES=8589934592`（8 GiB：这台服务器还跑着别的站点），`docker compose up -d`。停机 4 秒。迁移只追加 `0006_account_suspension.sql`；容器 13 秒内健康，以专用账户运行，密钥目录仍然只读挂着；各项计数前后一致。
- **从外网核对。** 健康检查、登录页、注册页、管理端、影视站都是 200，没带凭据的接口 401，HTTP 入口 301。按来源的限额真的生效：同一个地址连着请求 GitHub 登录的起点，前八次跳去 GitHub，第九、第十次是 503 并带 `Retry-After: 60`——nginx 写的 `X-Real-IP` 被服务端读到了。
- **一处回归，随即修掉（00:28，`581d1832e009`）。** 升级后照手册跑 `check-config`，报"数据库打不开"：上一回往报告里加"停用的账户数"时，把查询放在了连接池关闭之后。服务本身不受影响。补上测试（`config_report`），把自检、建账户、停用、恢复几条命令对着一个一次性的数据目录真的跑了一遍，然后升了第二次，同样停机 4 秒，自检通过。教训记在决策 0002 第 26 条。
- **自检报告多了三样**：全站内容总量上限、停用的账户数、请求来源读哪个头。现在线上是：总量上限 8 GiB，停用 0 个，按来源的限额读 `x-real-ip` 头。
- **恢复演练，第一次真的做。** 用 00:28 那次升级前的真实备份，按上面"恢复演练"的做法：同一个镜像自检通过（退出码 0），真的启动到就绪（`/readyz` 200、登录页 200、不带凭据的接口 401），库里的内容在磁盘上都在、大小都对，各项计数与备份前逐项一致。演练用的容器与临时目录都清掉了，正式服务全程没动。
- 升级前后的备份在 `/var/backups/assemble/2026-11-26-001731-guards/` 与 `2026-11-26-002849-selfcheck/`：各有一份数据归档、换之前的 `.env` 和计数。nginx 没有动，`nginx -t` 通过。

## 2026-11-26：整合发布与 DHT 公网采集

16:34 将正式服务从 `581d1832e009` 升级到 `e9852e6602a2`，包含管理端维护、系统通知、私人备忘录与 DHT 管理端。用户明确授权提交全部改动、合并主分支、推送、全量部署并开启公网采集；没有导入旧 DHT 历史数据。

- 源码与片库分支提交已整合到 `main`。本机完整验证 352.8 秒通过，macOS/Windows CI、服务端镜像和两平台安装包均成功。使用 `push-image.sh` 经 SSH 传送镜像，两端镜像摘要一致。
- 升级前停服务并完整备份数据、原 `.env`、Compose 与受保护密钥目录到 `/var/backups/assemble/20261126-163440-e9852e6602a2/`，目录 0700、数据归档 0600。旧迁移 1～6 校验和逐一一致，新迁移只追加 7～9。升级前后账户 1、设备 3、云盘文件 2、内容 2、日程 1、提醒 0，影视条目和图片均为 0；文件缺失、大小不符、外键违规均为 0，SQLite 自检通过。
- 保留密钥只读挂载与现有反向代理，叠加 `compose.dht.yaml`。HTTP 仍只监听宿主回环 8787，独立发布 UDP 6882，并在腾讯云轻量服务器防火墙新增同一端口的 IPv4 允许规则；其它规则及旧 DHT 的 6881 保留。
- 通过具名命令开启采集并核对审计。外部 BEP 5 ping 收到匹配响应，真实采样和摘要验证后的元数据已入库。健康、就绪、管理端和影视站均为 200，匿名管理与私人接口为 401 且不缓存；容器健康、无重启。采集快照、CI 链接及验收边界见 [DHT 手册](dht.md#2026-11-26-正式启用)。
- 桌面安装包已经构建交付；本次没有替换用户电脑上的现有安装，也没有把安装包构建结果视为 Windows/macOS 真机功能验收。
