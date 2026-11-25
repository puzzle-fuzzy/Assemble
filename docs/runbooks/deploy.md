# 部署 Assemble 服务端

正式服务在 2026-11-24 第一次部署上线：<https://as.yxswy.com>。下面"第一次部署"写的就是照着做过的步骤；哪些真的验过、哪些还没有，分别在最后两节里如实列出。用的是 Docker Compose 加 nginx。

## 材料在 `deploy/` 里

| 文件                   | 用途                                                                                                                    |
| ---------------------- | ----------------------------------------------------------------------------------------------------------------------- |
| `Dockerfile`           | Bun 构建共享 UI 管理端、官方 Rust 编译服务端、Debian slim 运行。Rust 版本与工具链一致，信号直接送到专用账户运行的服务端 |
| `compose.yaml`         | 一个服务。端口只交给本机的反向代理，日志自动轮转。镜像与数据的位置由 `.env` 决定                                        |
| `compose.build.yaml`   | 只在本机自己构建镜像时叠加到 `compose.yaml` 上                                                                          |
| `compose.secrets.yaml` | 密钥文件部署时叠加，将 secrets/ 只读挂载进容器；目录缺失则拒绝启动                                                      |
| `.env.example`         | 配置模板：镜像、数据位置、对外地址必填，其余都可以不填                                                                  |
| `nginx.conf.example`   | 反向代理：TLS、上传的上限、事件流的长连接。与正式服务器上的那一份只差域名                                               |
| `push-image.sh`        | 把持续集成构建的镜像经 SSH 送到服务器上。服务器直接拉 GitHub 的镜像仓库会握手超时，所以要它                             |

服务端在 Linux 上只依赖 C 库：SQLite 是静态编进去的，钥匙串的依赖只在 macOS 与 Windows 上进入构建。镜像约 120 MB。

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

- **两台真实设备互传、云盘操作的完整真人验收**：Windows 已保留登录并读到原云盘列表，不能替代完整的上传、取回与两设备收发。
- **经反向代理的大文件上传与事件流的长连接**。配置是按服务端的行为写的，没有对着真实设备跑过。
- **从备份恢复真实数据。** 第四次升级已带着真实账户、设备和云盘做过备份与迁移；恢复仍没有演练过。
- **退回旧版本。** 步骤写了（恢复备份、换回标记），没有做过。
- **公开注册与邮箱变更的真人完整流程、GitHub 真人授权。** 入口、SMTP 真发送与防线已核对，收信验证和身份绑定需要用户本人完成。
- **在途上传时停止服务的表现**：第四次升级已量出停旧服务到新服务就绪为 0.85 秒，但没有用真实上传压停止等待。
- **续期钩子**没有真的触发过（证书 2027-02 到期）。
