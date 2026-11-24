# 部署 Assemble 服务端

**这份手册里的步骤一步都还没有执行过。** 部署用的文件（`deploy/`）写好了，其中不依赖容器的那部分在本机核对过；但镜像还没有构建过，也没有在任何服务器上运行过。动手之前需要你定两件事（见[路线图](../plans/2026-11-23-roadmap.md)"需要你决定的事"）：域名，以及用 Docker Compose 还是 systemd。这里只准备了 Docker Compose 这一种。

## 材料在 `deploy/` 里

| 文件                 | 用到哪一步 | 用途                                                                                       |
| -------------------- | ---------- | ------------------------------------------------------------------------------------------ |
| `Dockerfile`         | 4          | 两阶段：官方 Rust 镜像里编译，Debian slim 里运行。专用账户、没有登录壳，信号直接送到服务端 |
| `compose.yaml`       | 4、日常    | 一个服务。端口只交给本机的反向代理，数据放在命名卷里，日志自动轮转                         |
| `.env.example`       | 3          | 配置模板。对外地址必填，其余都可以不填                                                     |
| `nginx.conf.example` | 5          | 反向代理：TLS、上传的上限、事件流的长连接                                                  |

服务端在 Linux 上只依赖 C 库：SQLite 是静态编进去的，钥匙串的依赖只在 macOS 与 Windows 上进入构建。镜像构建出来多大、要多久，还没有量过。

本机核对过的：`compose.yaml` 能被完整解析；容器里那组环境变量服务端认（`check-config` 通过），对外地址缺失或者不是 HTTPS 时服务端拒绝启动；镜像里那条健康检查命令对着真实运行的服务端时返回"健康"，对着没人监听的端口时返回"不健康"。

## 第一次部署

1. **域名。** 把域名的解析指到服务器。DNS 要你本人操作。
2. **服务器。** 装好 Docker（含 compose 插件）、nginx 与 certbot。磁盘要留出两块：构建（Rust 工具链与编译缓存，几个 GB），以及数据（随云盘和未送达的文件增长：每个账户的云盘上限默认 2 GiB，可调）。
3. **配置。** 把仓库放上服务器（`git clone` 或 `scp`），然后：

   ```sh
   cd deploy
   cp .env.example .env
   ```

   填上 `ASSEMBLE_PUBLIC_ORIGIN=https://<域名>`，必须与浏览器实际访问的完全一致。按磁盘余量决定要不要改 `ASSEMBLE_DRIVE_QUOTA_BYTES`。
4. **起服务端。**

   ```sh
   docker compose build
   docker compose up -d
   docker compose logs -f        # 看到 "starting Assemble server"；Ctrl+C 只是不再看日志
   curl -fsS http://127.0.0.1:8787/healthz    # {"status":"ok"}
   ```

   服务器磁盘很紧、装不下 Rust 工具链的话，在别的机器上构建再传过去，compose 里把 `build:` 那一段删掉：

   ```sh
   # 在装了 Docker 的开发机上（服务器的 CPU 是 arm64 就把 amd64 换成 arm64）
   docker buildx build --platform linux/amd64 -f deploy/Dockerfile -t assemble-server:local --load .
   docker save assemble-server:local | gzip | ssh <服务器> 'gunzip | docker load'
   ```
5. **反向代理。** 把 `nginx.conf.example` 放进 nginx 的 `conf.d`，换掉域名与证书路径。先用 certbot 拿证书，再启用这个站点：

   ```sh
   sudo certbot certonly --webroot -w /var/www/certbot -d <域名>
   sudo nginx -t && sudo systemctl reload nginx
   ```

6. **自检。**

   ```sh
   docker compose exec assemble-server check-config     # 最后一行是账户数
   curl -fsS https://<域名>/healthz                      # 经反向代理也要应答
   ```

   浏览器打开 `https://<域名>/web/login`，应当是登录页。
7. **建第一个账户。** 账户只能从这里建，不开放注册；密码在终端里输两遍，不回显，也不作为参数传入。

   ```sh
   docker compose exec assemble-server create-account <用户名>
   docker compose exec assemble-server list-accounts
   ```
8. **桌面端指向正式地址。** 桌面端连哪个服务在构建时定下，所以安装包必须以 `ASSEMBLE_SERVER_ORIGIN=https://<域名>` 构建：给 GitHub 仓库设变量 `ASSEMBLE_SERVER_ORIGIN`，再打标签或者手动运行"安装包"流程（见[运行手册](verify-macos-launch.md)的"安装包从哪来"）。
9. **验收。** 两台设备各登录一次，互发一个文件，再看一眼云盘。

## 日常

```sh
cd deploy
docker compose ps                       # 健康状态
docker compose logs --since 1h         # 日志
docker compose exec assemble-server create-account <用户名>
docker compose exec assemble-server list-accounts
```

某台设备不用了，在桌面应用的"设备"卡片里移除它：它的令牌随即作废。

### 升级

```sh
git pull
docker compose build
docker compose up -d
```

数据库结构在启动时自动迁移。迁移只向前，所以升级之前先备份一份；要退回旧版本，得同时恢复那份备份。

### 备份与恢复

SQLite 开着写的时候直接拷文件可能不一致，所以备份时停几十秒：

```sh
docker compose stop
docker run --rm -v assemble_assemble-data:/data -v "$PWD":/backup alpine \
  tar czf "/backup/assemble-$(date +%F).tgz" -C /data .
docker compose start
```

备份里有：账户（密码的散列，没有明文）、设备令牌的摘要、发送记录、云盘与未送达的文件。令牌的摘要不能拿去登录，但云盘里的文件本身是想给谁看就给谁看的东西，备份要当敏感数据保管。

恢复是反过来：停服务，把归档解进一个新的卷，再启动。

## 反向代理要注意的

- **上传上限。** 单文件 100 MB，nginx 那边留到 110 MB。桌面端在挑文件时就拦下超过上限的，服务端也会拒绝，所以这道上限平时碰不到，它只是最后一道闸。
- **不缓冲请求体。** 服务端会提前拒绝超限或不合法的上传，并等发送方把话说完；nginx 边收边转，不在自己的磁盘上再存一份。
- **事件流。** `/v1/events` 是一条不结束的长连接，服务端每 15 秒发一次心跳；读超时要放宽，缓冲要关掉。
- **只映射到本机。** compose 里端口绑在 `127.0.0.1` 上。不要为了省事把它直接对外，那样会绕过 TLS。
- **服务端不读转发头。** 在它看来，所有请求都来自反向代理。日志里看到的对端都是 `127.0.0.1`；以后如果要按来源限流或者封禁，要先决定怎么信任代理头。

## 换成 systemd 的话

`deploy/` 里没有准备这一种。要做的话：用一个带静态链接目标（`x86_64-unknown-linux-musl`）的构建产出服务端可执行文件，加一个以专用账户运行、`Environment=` 指向同一份 `.env`、`Restart=on-failure` 的 unit 文件，反向代理的部分照旧。

## 还没有验证过的

- 镜像没有构建过：本机的 Docker 守护进程没有启动，启动它可能会连带拉起别的项目的容器，所以没有擅自启动。
- 上面"第一次部署"的九步一步都没有走过；`nginx.conf.example` 也没有经过 `nginx -t`。
- 真实容器里的表现：数据卷的属主、健康检查、停止时能否在宽限期内退出。
- 构建缓存与镜像在磁盘紧的服务器上占多少。
