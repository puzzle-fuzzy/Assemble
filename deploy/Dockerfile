# Assemble 服务端的镜像。构建上下文是仓库根目录（见 .dockerignore），用法见 docs/runbooks/deploy.md。
#
#   docker build -f deploy/Dockerfile -t assemble-server:local .
#
# 服务端在 Linux 上只依赖 C 库：SQLite 是静态编进去的，钥匙串的依赖只在 macOS 与 Windows 上进入构建。

# ---- 构建：官方 Rust 镜像自带编译 C 代码要用的工具链 ----
FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY apps/server/ apps/server/
# 编译产物与下载的依赖放在缓存卷里，不进镜像层；成品拷出来交给下一阶段。
ENV CARGO_TARGET_DIR=/target
RUN --mount=type=cache,target=/target \
    --mount=type=cache,target=/usr/local/cargo/registry \
    cargo build --release --locked -p assemble-server --bin assemble-server \
    && mkdir /out \
    && cp /target/release/assemble-server /out/

# ---- 运行：只带着可执行文件，用一个没有登录壳的专用账户跑 ----
FROM debian:bookworm-slim
RUN useradd --system --uid 10001 --home-dir /var/lib/assemble --shell /usr/sbin/nologin assemble \
    && mkdir -p /var/lib/assemble \
    && chown assemble:assemble /var/lib/assemble
COPY --from=build /out/assemble-server /usr/local/bin/assemble-server

# 容器里的回环地址外面连不上，所以监听所有网卡；对不对外，由部署时的端口映射与反向代理决定。
ENV ASSEMBLE_DATA_DIR=/var/lib/assemble \
    ASSEMBLE_ADDR=0.0.0.0:8787
EXPOSE 8787
VOLUME /var/lib/assemble

# 镜像里没有 curl；用 bash 自带的 /dev/tcp 问一句 /healthz。
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["bash", "-ec", "exec 3<>/dev/tcp/127.0.0.1/8787; printf 'GET /healthz HTTP/1.1\\r\\nHost: localhost\\r\\nConnection: close\\r\\n\\r\\n' >&3; head -n 1 <&3 | grep -q ' 200 '"]

# 服务端自己收 SIGTERM 并在十秒内退出；信号直接送到它，不经壳。
STOPSIGNAL SIGTERM
ENTRYPOINT ["assemble-server"]
CMD ["serve"]
