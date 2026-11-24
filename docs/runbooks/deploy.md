# 部署 Assemble 服务端

正式服务在 2026-11-24 第一次部署上线：<https://as.yxswy.com>。下面"第一次部署"写的就是照着做过的步骤；哪些真的验过、哪些还没有，分别在最后两节里如实列出。用的是 Docker Compose 加 nginx。

## 材料在 `deploy/` 里

| 文件                 | 用途                                                                                                                                |
| -------------------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| `Dockerfile`         | 两阶段：官方 Rust 镜像里编译（版本与 `rust-toolchain.toml` 一致），Debian slim 里以一个没有登录壳的专用账户运行。信号直接送到服务端 |
| `compose.yaml`       | 一个服务。端口只交给本机的反向代理，日志自动轮转。镜像与数据的位置由 `.env` 决定                                                    |
| `compose.build.yaml` | 只在本机自己构建镜像时叠加到 `compose.yaml` 上                                                                                      |
| `.env.example`       | 配置模板：镜像、数据位置、对外地址必填，其余都可以不填                                                                              |
| `nginx.conf.example` | 反向代理：TLS、上传的上限、事件流的长连接。与正式服务器上的那一份只差域名                                                           |
| `push-image.sh`      | 把持续集成构建的镜像经 SSH 送到服务器上。服务器直接拉 GitHub 的镜像仓库会握手超时，所以要它                                         |

服务端在 Linux 上只依赖 C 库：SQLite 是静态编进去的，钥匙串的依赖只在 macOS 与 Windows 上进入构建。镜像约 120 MB。

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
7. **建第一个账户。** 账户只能从这里建，不开放注册；密码在终端里输两遍，不回显，也不作为参数传入。这一步由你自己做：

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

### 升级

```sh
# 开发机上：把新的镜像送到服务器
deploy/push-image.sh <新的标记>
# 服务器上：换掉 .env 里的 ASSEMBLE_IMAGE，先备份（见下），再重新起来
cd /opt/assemble && docker compose up -d && docker compose ps
```

数据库结构在启动时自动迁移。迁移只向前，所以升级之前先备份一份；要退回旧版本，得同时恢复那份备份，并把 `ASSEMBLE_IMAGE` 换回原来的标记。

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

恢复是反过来：停服务，把归档解进数据目录（属主要是 10001），再启动。**这套备份与恢复还没有演练过。**

## 反向代理要注意的

- **上传上限。** 单文件 100 MB，nginx 那边留到 110 MB。桌面端在挑文件时就拦下超过上限的，服务端也会拒绝，所以这道上限平时碰不到，它只是最后一道闸。
- **不缓冲请求体。** 服务端会提前拒绝超限或不合法的上传，并等发送方把话说完；nginx 边收边转，不在自己的磁盘上再存一份。
- **事件流。** `/v1/events` 是一条不结束的长连接，服务端每 15 秒发一次心跳；读超时要放宽，缓冲要关掉。
- **只映射到本机。** compose 里端口绑在 `127.0.0.1` 上。不要为了省事把它直接对外，那样会绕过 TLS。
- **服务端不读转发头。** 在它看来，所有请求都来自反向代理。日志里看到的对端都是 `127.0.0.1`；以后如果要按来源限流或者封禁，要先决定怎么信任代理头。
- **安全响应头。** CSP 与 Referrer-Policy 由服务端自己发；`X-Content-Type-Options` 与 HSTS 是 nginx 加的。

## 换成 systemd 的话

`deploy/` 里没有准备这一种。要做的话：用一个带静态链接目标（`x86_64-unknown-linux-musl`）的构建产出服务端可执行文件，加一个以专用账户运行、`Environment=` 指向同一份 `.env`、`Restart=on-failure` 的 unit 文件，反向代理的部分照旧。

## 做过的（2026-11-24）

- **本机的镜像。** 真的构建了三次才成，只做不依赖容器的检查时没发现三处问题：构建上下文里少了桌面端的清单与入口文件，cargo 解析不了工作区；建了专用账户却忘了写 `USER`，容器以 root 运行；手册里 `docker compose exec` 的命令少了可执行文件名。之后用 compose 把镜像跑了一遍：6 秒内健康、以专用账户运行、端口只绑回环地址、建账户与自检正常、停止 0.18 秒且退出码 0、重启后数据还在。
- **持续集成构建镜像。** linux/amd64 的镜像在 GitHub 上构建成功，公开可拉。
- **服务器上的服务。** 镜像经 SSH 传入（服务器直接拉 GitHub 的镜像仓库握手超时）。容器 6 秒内健康、以专用账户运行、只监听本机的 8787；数据目录属于 10001、仅本人可读写；自检里对外地址与云盘上限都对。
- **证书与反向代理。** 证书申请成功、自动续期已安排；nginx 的配置 `nginx -t` 通过，重载之后新站点应答、抽查的两个老站点不受影响。
- **从外网核对。** `/healthz` 应答；登录页 200 且证书有效；HTTP 入口 301 跳到 HTTPS；没带凭据的接口与事件流都是 401。

## 还没有验证过的

- **建第一个账户之后的每一步**：在应用里登录、两台设备收发、云盘。
- **经反向代理的大文件上传与事件流的长连接**。配置是按服务端的行为写的，没有对着真实设备跑过。
- **升级。** 步骤写了，没有做过一次。
- **备份与恢复**没有演练过。
- **停止时在真实服务器上的表现**：本机实测 0.18 秒退出，服务器上没有量。
- **续期钩子**没有真的触发过（证书 2027-02 到期）。
