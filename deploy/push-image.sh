#!/bin/sh
# 把持续集成构建的服务端镜像送到服务器上。
#
# 服务器在国内，直接拉 GitHub 的镜像仓库会握手超时（清单取得到，数据取不到）。
# 所以由能访问 GitHub 的开发机把 linux/amd64 的镜像拉下来，再经 SSH 传过去。
#
#   deploy/push-image.sh <标记>       标记是提交号的前 12 位，或者 latest
#
# 传完之后升级：在服务器上把 /opt/assemble/.env 里的 ASSEMBLE_IMAGE 换成这个标记，
# 再 docker compose up -d。完整步骤见 docs/runbooks/deploy.md。

set -eu

tag=${1:?用法：deploy/push-image.sh <标记>}
image="ghcr.io/puzzle-fuzzy/assemble-server:$tag"
# ~/.ssh/config 里的主机别名。
server=${ASSEMBLE_SERVER_HOST:-yxswy.com}

echo "拉取 $image（linux/amd64）"
docker pull --platform linux/amd64 "$image"
echo "传给 $server"
docker save "$image" | gzip -1 | ssh "$server" 'gunzip | docker load'
ssh "$server" "docker image inspect '$image' --format '服务器上已有：{{.Os}}/{{.Architecture}}'"
echo "完成。要换成这个版本：在服务器上改 /opt/assemble/.env 里的 ASSEMBLE_IMAGE，再 docker compose up -d。"
