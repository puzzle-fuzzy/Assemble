#!/bin/sh
# 把这台 Mac 上做好的发布目录（scripts/release/stage.ts）送到服务器上，交给服务端发布。
#
#   deploy/push-update.sh <发布目录>              只校验
#   deploy/push-update.sh <发布目录> --apply      校验通过就发布
#
# 通常是 scripts/release/publish.ts 在调它。发布目录进的是数据目录下的一个临时目录：数据目录
# 只有 root 进得去，而容器里的服务端以另一个账户（编号 10001）运行，所以传进去之后把属主
# 改成它。校验或发布之后临时目录就删掉，不在那块很紧的磁盘上留东西。
# 完整步骤见 docs/runbooks/deploy.md。

set -eu

directory=${1:?用法：deploy/push-update.sh <发布目录> [--apply]}
apply=${2:-}
if [ ! -f "$directory/manifest.json" ]; then
  echo "发布目录里没有 manifest.json：$directory" >&2
  exit 2
fi
if [ -n "$apply" ] && [ "$apply" != "--apply" ]; then
  echo "第二个参数只能是 --apply" >&2
  exit 2
fi

# ~/.ssh/config 里的主机别名。
server=${ASSEMBLE_SERVER_HOST:-yxswy.com}
name="incoming-update-$(date -u +%Y%m%d-%H%M%S)"
remote="/var/lib/assemble/$name"
# 不在提示上停下来：钥匙用不了就失败。
ssh_options="-o BatchMode=yes"

echo "传给 $server"
ssh $ssh_options "$server" "install -d -m 0700 '$remote'"
# 只打包发布目录里的文件本身：COPYFILE_DISABLE 不把 macOS 的元数据文件带进去。
COPYFILE_DISABLE=1 tar czf - -C "$directory" . \
  | ssh $ssh_options "$server" "tar xzf - -C '$remote' && chown -R 10001:10001 '$remote'"
# 校验也好，发布也好，成了没成都把临时目录删掉。
cleanup() {
  ssh $ssh_options "$server" "rm -rf '$remote'"
}
trap cleanup EXIT

ssh $ssh_options "$server" "cd /opt/assemble && docker compose exec -T assemble assemble-server publish-update /var/lib/assemble/$name $apply"
