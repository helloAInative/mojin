# 摸金小王子 mj-server · 适配绿联 DXP4800 (N100) + 缓存复用
#
# 构建：docker build -t mojin/mj-server:0.1.0 .
# 构建（带本地 target 缓存）：
#   docker build -t mojin/mj-server:0.1.0 \
#     --build-arg CARGO_TARGET_DIR=/cache/target .
#
# 如果宿主上 ./target 已经有 release 产物，可以用 bind mount 把同一目录
# 传进 builder（见 docker-compose.yml 的 BUILDKIT_CACHE_MOUNT 注释）。

FROM rust:1.85-bookworm AS builder
ARG CARGO_TARGET_DIR=/app/target
WORKDIR /app
# 先复制 manifest，让 COPY . 触发源码变动时复用 deps 层
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src migrations \
 && echo 'fn main(){}' > src/main.rs \
 && echo '' > migrations/.gitkeep \
 && cargo build --release --bin mj-server || true
COPY src ./src
COPY migrations ./migrations
RUN touch src/main.rs && cargo build --release --bin mj-server \
 && strip "$CARGO_TARGET_DIR/release/mj-server" \
 && cp "$CARGO_TARGET_DIR/release/mj-server" /app/mj-server

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl tini \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -r -u 10001 -m mojin
WORKDIR /app
COPY --from=builder /app/mj-server /usr/local/bin/mj-server
RUN mkdir -p /data && chown -R mojin:mojin /data
USER mojin
ENV MJ_HOST=0.0.0.0 \
    MJ_PORT=8787 \
    MJ_DB_PATH=/data/mojin.db \
    RUST_LOG=mj_server=info,tower_http=info
EXPOSE 8787
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD curl -fsS http://127.0.0.1:8787/healthz || exit 1
# tini 处理 PID 1 信号 + zombie
ENTRYPOINT ["/usr/bin/tini","--","mj-server"]
