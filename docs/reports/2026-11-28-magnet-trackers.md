# 磁力 Tracker 接入与 Mac 验证（2026-11-28）

用户要求先修复元数据阶段 Tracker 传递、混合 Fake-IP，再接入筛选池，测试实际下载速度并提交、推送和安装本机新版；本轮不做 Windows 专项工作。

## 实现

- 元数据阶段的内部磁力 URI 带上已校验、固定为公网 IP 的 UDP tr；下载阶段继续使用同一规则。librqbit 9.1.0 的磁力分支不读取 `AddTorrentOptions.trackers`，只读 URI 的 trackers，这正是此前漏掉它们的原因。
- 同时识别 `198.18.0.0/15` 和 `2001:2::/48` 的域名 Fake-IP 集合，经受限 DoH 查询真实公网地址。优先 A，没有 A 时查 AAAA；实际连接固定到已校验地址，私网与字面 Fake-IP 仍拒绝。DHT bootstrap 也走该路径。
- HTTP(S) 用独立、保留域名/SNI/证书验证的客户端短时查询节点，过滤私网节点，然后发送 stopped；引擎持续使用 UDP/DHT/PEX。这是节点引导，不是另建 HTTP 下载或中转服务。响应最多 64 KiB，不跟随重定向；引导最多 128 节点。
- 默认 12 个经本机协议检查的候选，8 UDP、4 HTTPS。缓存只包含无查询参数的公开地址；超过 24 小时按固定来源后台刷新，每轮最多抽查 64 个新候选和原缓存，8 路并发、45 秒探测预算，按协议响应、延迟及服务器地址去重。更新失败保留最后有效缓存，退出显式取消并等待后台刷新。
- 上传仍为每任务 256 KiB/秒，完成、暂停和退出即停；不增加入站监听、UPnP、界面命令或服务端接口。启动及恢复为暂停时不因 Tracker 池自动联网。

## 来源与筛选

候选来源是 [adysec/tracker](https://github.com/adysec/tracker)，初始参考提交 [04ab360](https://github.com/adysec/tracker/tree/04ab3604943d8915f96fdb8aaaa4ffc8aa4521db)（2026-11-27T13:46:27Z）。优选文件实际有 331 条：173 UDP、110 HTTP、43 HTTPS、5 WSS；不是按本机下载速度排名的列表。运行时只抽查 UDP/HTTPS，WSS 不交给引擎。

本机对优选 UDP/HTTPS 共 216 个候选做连接/无 info_hash 的协议检查；有 22 个端点在该轮得到协议响应，存在重复服务器和跨轮波动。初始池进一步去重，并根据实际 announce 的 Invalid peer_id / unknown torrent 回应调整候选；Ubuntu 官方端点另用其合法公开资源做实际 announce 核对。协议可达不等于某个资源有可连接节点。

上游仓库声明 GPL-3.0，参见其 [LICENSE](https://github.com/adysec/tracker/blob/04ab3604943d8915f96fdb8aaaa4ffc8aa4521db/LICENSE)；这里只引用少量公开端点数据及更新来源，没有复制其检查器实现。协议处理是本仓库实现，bencode 解码复用已锁定的 librqbit-bencode 9.1.0。

## 自动化证据

真实回环 BT 回归关闭 DHT、去掉 x.pe，只允许本机 UDP Tracker 返回种子节点：元数据解析、选择文件、暂停、销毁后恢复、损坏分片重下、同名保留、完成即停和缓存清理均走真实协议。HTTP 回归用不会被系统正常解析的域名、固定到本机测试服务器，核对 Host 与参数、节点过滤，以及取消时发送 stopped。另测混合 Fake-IP、A/AAAA 公网边界、凭据不进入公共缓存、取消刷新后缓存保留且不重启。

默认测试不依赖公网。手工网络检查为 ignored 测试，只在明确提供合法公开磁力文件时运行，使用临时目录与内存凭据，不触碰真实钥匙串和下载历史，来源不写入输出：

```sh
ASSEMBLE_PUBLIC_MAGNET_FILE=/absolute/path/to/public.magnet cargo test -p assemble-download --lib magnet::tests::public_tracker_download_benchmark -- --exact --ignored --nocapture
ASSEMBLE_PUBLIC_MAGNET_FILE=/absolute/path/to/public.magnet cargo test -p assemble-download --lib magnet::tests::public_tracker_connection_diagnostics -- --exact --ignored --nocapture
```

测速最多在元数据阶段等待 90 秒；进入下载后最多 90 秒或 128 MiB，先到即停止并清理临时任务。只输出耗时、字节量、速率和稳定错误码，未完成不声称通过整文件校验。

## 公网结果与限制

测试来源是 [Debian 官方 BT 下载](https://www.debian.org/CD/torrent-cd/) 的 Debian 13.8.0 arm64 netinst（737,456,128 字节）与 [Ubuntu 官方发布目录](https://releases.ubuntu.com/24.04/) 的 Ubuntu 24.04.5 Server（4,080,486,400 字节），并追加 Ubuntu 24.04.4 Server（3,405,469,696 字节）复测。先用原实现 `9a3ff84` 加同一个数值输出测试保存基线二进制，随后使用修复实现；未调整系统代理、防火墙或上传上限。

| 样本                      | 原实现                                   | 修复后实际结果                                      |
| ------------------------- | ---------------------------------------- | --------------------------------------------------- |
| Debian，带官方 tr         | 约 8 ms，DOWNLOAD_INVALID_SOURCE，0 字节 | 约 91,010 ms，DOWNLOAD_METADATA_UNAVAILABLE，0 字节 |
| Debian，不带 tr           | 约 91,005 ms，元数据超时，0 字节         | 未单独对比，不据此声称裸磁力提速                    |
| Ubuntu 24.04.5，带官方 tr | 地址校验失败，0 字节                     | 多轮约 91,005–91,008 ms，元数据超时，0 字节         |
| Ubuntu 24.04.4，带官方 tr | 未单独测基线                             | 最终候选池约 91,007 ms，元数据超时，0 字节          |

修复后的 Rust 连接诊断实际得到 8 个固定地址的 UDP Tracker、4 个 HTTP/显式节点；官方 Ubuntu Tracker 另一次有效回应中报告 746 个完成者，但返回节点少，不能据此推断当前 Mac 能连上它们。上述端到端尝试均没有进入分片下载，**无法给出有效下载速率或提速倍数**，不能把 0 字节、端点可达或回环测试通过写成公网提速成功，也不能据此断定整个 UDP 网络永久不可用。

本机原始探测与测速记录保留在忽略目录 `data/local/tracker-audit/`。本机安装回执单独保存在 `data/local/local-install/`，真实安装与启动结果以回执为准；不会因为提交或自动化测试通过就声称已经安装。全站自动更新发布和服务端部署不在本次操作范围。
