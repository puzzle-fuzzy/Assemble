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
7. **建第一个账户。** 密码在终端里输两遍，不回显，也不作为参数传入；这一步由你自己做。之后的账户可以在网页上凭邀请码注册（见下面"日常"里的"邀请码注册"）：

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

### 邀请码注册

在 `.env` 的末尾加一行邀请码（不要加引号，前后不要有空格），再让容器重新读一次配置：

```sh
cd /opt/assemble
cp -p .env .env.before-invite
#   ASSEMBLE_INVITE_CODE=<8 到 128 个字符，不能有空白>
chmod 600 .env
docker compose up -d
docker compose exec -T assemble assemble-server check-config   # 最后一行应是"网页注册：开放（要邀请码）"
```

之后 `https://<域名>/web/register` 就是注册页，登录页上也会多一个"注册"的入口。想换邀请码就改那一行再 `docker compose up -d`；清空它，注册页就没了（已经注册的账户不受影响）。从此 `.env` 里有一份要保管的秘密：服务器上的 root 看得到它，别把这个文件拷到别处。

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

恢复是反过来：停服务，把归档解进数据目录（属主要是 10001），再启动。备份在 2026-11-25 真的做过一次（停机 1 秒）；**恢复还没有演练过**。

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
- 这一次是带着真实账户做的升级；**恢复仍然没有演练过**。

## 做过的（2026-11-25 上午）：第三次升级，带上授权页的修复

起因：你在正式服务上第一次给自己登录（`assemble-mcp login`），在授权页点"确认登录"没有任何反应。缘故与修法记在决策 0002 第 24 条：授权页自己的内容安全策略拦掉了跳回本机的那一步。修复是 `52aed64`，这一次上的是它后面的 `0b6d40993a5b`（再往后只多一个文档提交）。

- **先核对，再动手。** 正式库里记着的两个迁移的校验和，和仓库里的脚本逐一相符；这一次也没有新的迁移脚本。
- **送镜像。** `deploy/push-image.sh 0b6d40993a5b`：拉 linux/amd64、经 SSH 传入，一次就成。
- **备份与换版本连着做，只停一次。** 10:50:43 停服务，归档 `/var/backups/assemble/2026-11-25-1050.tgz`（12 KB），`.env` 留下 `.env.before-0b6d40993a5b`，换掉标记，`docker compose up -d`；10:50:44 健康检查就应答了，停机 1 秒。`.env` 的权限（仅本人可读写）和邀请码那一行原样没动。
- **换版本之后。** 容器 20 秒内显示健康，跑的镜像摘要与拉下来的那一个一致，以专用账户运行；启动日志只有"监听"两行；自检里账户数是 1，网页注册仍然"开放（要邀请码）"。
- **从外网核对。** `/healthz`、`/readyz`、登录页、注册页都是 200；没带凭据的 `/v1/planner`、`/v1/devices`、`/v1/events` 是 401；HTTP 入口 301 跳到 HTTPS。登录页的内容安全策略和以前一样（只认本站、没有脚本）；换掉的那一份只在授权页上，从外网看不到——它要登录之后才出现，是在真实浏览器里对着本机服务端核对的（见[运行手册](verify-macos-launch.md)"2026-11-25 做过的"）。

## 还没有验证过的

- **建第一个账户之后的每一步**：在应用里登录、两台设备收发、云盘。
- **经反向代理的大文件上传与事件流的长连接**。配置是按服务端的行为写的，没有对着真实设备跑过。
- **带着真实数据再升级一次。** 2026-11-25 的两次升级，前一次库是空的，后一次库里只有一个刚建的账户（还没有设备、日程和文件）。用一阵子之后再升一次，才算数。恢复没有演练过。
- **退回旧版本。** 步骤写了（恢复备份、换回标记），没有做过。
- **邀请码注册在正式服务上真的注册一个账户。** 防线从外网核对过（乱填的被拒）；用对的邀请码走完一遍，还没有。
- **停止时在真实服务器上的表现**：本机实测 0.18 秒退出，服务器上没有量。
- **续期钩子**没有真的触发过（证书 2027-02 到期）。
