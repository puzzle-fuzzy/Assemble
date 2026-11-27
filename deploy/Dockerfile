# Assemble 服务端的镜像。构建上下文是仓库根目录（见 .dockerignore），用法见 docs/runbooks/deploy.md。
#
#   docker build -f deploy/Dockerfile -t assemble-server:local .
#
# 服务端在 Linux 上只依赖 C 库：SQLite 是静态编进去的，钥匙串的依赖只在 macOS 与 Windows 上进入构建。

# ---- 管理端：同一锁文件、同一份 shadcn 组件，产物随镜像发布 ----
FROM oven/bun:1.4.4 AS admin
WORKDIR /src
COPY package.json bun.lock bunfig.toml ./
COPY apps/desktop/package.json apps/desktop/package.json
COPY apps/admin/package.json apps/admin/package.json
COPY apps/browser-extension/package.json apps/browser-extension/package.json
COPY apps/media-web/package.json apps/media-web/package.json
COPY apps/releases-web/package.json apps/releases-web/package.json
COPY packages/media/package.json packages/media/package.json
COPY packages/ui/package.json packages/ui/package.json
COPY packages/display/package.json packages/display/package.json
RUN bun install --frozen-lockfile
COPY apps/admin/ apps/admin/
COPY apps/media-web/ apps/media-web/
COPY apps/releases-web/ apps/releases-web/
COPY packages/media/ packages/media/
COPY packages/ui/ packages/ui/
COPY packages/display/ packages/display/
RUN bun run admin:build && bun run media:build && bun run releases:build

# ---- 构建：官方 Rust 镜像自带编译 C 代码要用的工具链 ----
# 版本与仓库根目录的 rust-toolchain.toml 一致（官方镜像不用 rustup，那个文件在这里不生效）。升级时两处一起改。
FROM rust:1.100.0-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY apps/server/ apps/server/
# 根目录的清单把桌面端和给助手用的入口（assemble-mcp）也列为工作区成员，cargo 解析工作区时
# 要读到它们的清单与入口文件。它们都不会被编译（上面只构建 assemble-server）；见 .dockerignore。
COPY apps/desktop/src-tauri/ apps/desktop/src-tauri/
COPY apps/mcp/ apps/mcp/
# 给桌面更新包验签的公钥由 assemble-protocol 用 include_str! 读进去，编译服务端时要它在。
COPY deploy/updater.pubkey deploy/updater.pubkey
# 编译产物与下载的依赖放在缓存卷里，不进镜像层；成品拷出来交给下一阶段。
ENV CARGO_TARGET_DIR=/target
ARG CARGO_BUILD_JOBS=2
RUN --mount=type=cache,target=/target \
    --mount=type=cache,target=/usr/local/cargo/registry \
    CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS}" cargo build --release --locked -p assemble-server --bin assemble-server \
    && mkdir /out \
    && cp /target/release/assemble-server /out/

# ---- 运行：只带着可执行文件，用一个没有登录壳的专用账户跑 ----
FROM debian:bookworm-slim
# GitHub HTTPS 与 SMTP TLS 需要系统信任根；不能为了缺证书而关闭验证。
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates ffmpeg \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home-dir /var/lib/assemble --shell /usr/sbin/nologin assemble \
    && mkdir -p /var/lib/assemble \
    && chown assemble:assemble /var/lib/assemble
COPY --from=build /out/assemble-server /usr/local/bin/assemble-server
COPY --from=admin /src/apps/admin/dist/ /usr/share/assemble/admin/
COPY --from=admin /src/apps/media-web/dist/ /usr/share/assemble/movies/
COPY --from=admin /src/apps/releases-web/dist/ /usr/share/assemble/download/
# 从这里起以专用账户运行；少了这一行，容器会以 root 跑。
USER assemble:assemble

# 容器里的回环地址外面连不上，所以监听所有网卡；对不对外，由部署时的端口映射与反向代理决定。
ENV ASSEMBLE_DATA_DIR=/var/lib/assemble \
    ASSEMBLE_ADDR=0.0.0.0:8787 \
    ASSEMBLE_ADMIN_ASSETS_DIR=/usr/share/assemble/admin \
    ASSEMBLE_MEDIA_WEB_ASSETS_DIR=/usr/share/assemble/movies \
    ASSEMBLE_RELEASES_WEB_ASSETS_DIR=/usr/share/assemble/download
EXPOSE 8787
VOLUME /var/lib/assemble

# 镜像里没有 curl；用 bash 自带的 /dev/tcp 问一句 /healthz。
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["bash", "-ec", "exec 3<>/dev/tcp/127.0.0.1/8787; printf 'GET /healthz HTTP/1.1\\r\\nHost: localhost\\r\\nConnection: close\\r\\n\\r\\n' >&3; head -n 1 <&3 | grep -q ' 200 '"]

# 服务端自己收 SIGTERM 并在十秒内退出；信号直接送到它，不经壳。
STOPSIGNAL SIGTERM
ENTRYPOINT ["assemble-server"]
CMD ["serve"]
