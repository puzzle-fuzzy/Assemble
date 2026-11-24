# 在 macOS 上真实启动并检查 Assemble

自动化测试覆盖不到窗口、托盘、系统钥匙串、真实浏览器和退出时的清理。这个仓库里已经有四条缺陷是测试全绿、实际运行之后才暴露的（见 [决策 0002](../decisions/0002-own-server-replaces-jarvis.md#从真实启动中学到的)）。涉及应用壳、服务端或生命周期的改动，除了 `bun run verify`，还要按本文实际跑一遍。

## 0. 先知道这几件事

- **模拟的浏览器不算浏览器。** 测试里的"浏览器"是 HTTP 客户端，请求头是人为构造的。凡是浏览器参与的流程（登录页、授权确认页），必须在真实的浏览器里走一遍。
- **屏幕锁定时做不了界面层面的验证。** 锁屏后没有前台应用，读不到窗口内容，截图会超时。后台进程照常运行，所以进程层面、接口层面的检查仍然有效。
- **区分"卡死"和"读不到"。** 窗口读不出来时，先用 `sample <进程号> 1 -file /tmp/sample.txt` 看主线程：停在正常的事件循环等待里是健康的；停在我们自己的代码里、或者在等一把锁，就是卡死。

## 1. 准备：本地服务端、账户和一台替身设备

```sh
# 本地开发用的服务端：数据放在 data/local/server（不进仓库），地址 http://127.0.0.1:8787
bun run server:dev
```

另开一个终端，建一个账户并登记一台没有界面的"替身设备"，充当第二台电脑：

```sh
printf '你的密码\n' | ASSEMBLE_DATA_DIR=data/local/server ASSEMBLE_PUBLIC_ORIGIN=http://127.0.0.1:8787 \
  cargo run -p assemble-server -- create-account dev
cargo build -p assemble-server --example fake_device
F=./target/debug/examples/fake_device
$F --token data/local/fake-a.json register --data-dir data/local/server --account dev --name 替身A
$F --token data/local/fake-a.json online   --origin http://127.0.0.1:8787
```

`online` 会把收到的事件打印出来，`Ctrl+C` 下线。要同时扮演两台设备，给每台各指定一个 `--token` 文件。替身直接写服务端的数据库，只能用在本地开发的服务上。

## 2. 构建并启动桌面端

```sh
cd apps/desktop
ASSEMBLE_SERVER_ORIGIN=http://127.0.0.1:8787 bunx tauri build --no-bundle
cd ../..
./target/release/assemble-desktop 2>&1 | tee /tmp/assemble-run.log
```

连接哪个服务在构建时定下：不设 `ASSEMBLE_SERVER_ORIGIN` 的发布构建拒绝启动。调试构建加载的是开发服务器上的页面，需要先 `bun run dev:frontend`。

## 3. 检查窗口与登录

- 窗口出现，左上角写着"本地开发服务"（按地址判断，不是按调试还是发布），账户卡片里有"使用浏览器登录"。
- 点"使用浏览器登录"：系统默认浏览器打开授权页；还没登录时会先跳到登录页。在浏览器里登录并点"确认登录"，回到应用时应当已登录，账户卡片下面写着"这台设备：<本机名>"。
- "设备"卡片列出账户里的全部设备，替身设备显示在线，本机带"本机"标记。停掉替身（`Ctrl+C`），列表里它变成离线。
- 关掉窗口：应用留在托盘（菜单栏右侧）。再运行一次可执行文件，第二个实例会把窗口叫回来然后自己退出，进程始终只有一个。

读窗口内容可以用系统自带的无障碍接口（Xcode 的"辅助功能检查器"，或任何读屏工具）：页面里的文字和按钮都应当读得到，主线程卡死时则什么都读不到。

## 4. 退出并确认清理

带应用包的构建可以直接退出：

```sh
osascript -e 'tell application id "com.puzzlefuzzy.assemble" to quit'
```

不带应用包的可执行文件要把窗口置前后按 ⌘Q。期望：进程退出；替身设备收到一次"设备下线"的事件，`online` 的输出里本机变成离线。

再确认服务端能按时退出：对它发 `kill -TERM`，应当在一秒内结束，即使此刻有事件流连着。曾经出现过服务端收到信号后无限期挂着的情况。

## 5. 收尾

```sh
pkill -TERM -f 'fake_device --token data/local'
rm -f data/local/fake-*.json      # 替身设备的令牌，仅本人可读
```

真实登录过的应用会在系统钥匙串里留一条 `assemble.device-session:http://127.0.0.1:8787`。在应用里"退出登录"会删掉它。

## 2026-11-24 做过的

- 服务端与两台替身设备：两台都在线时互相可见，一台到时下线后另一台立刻看到它离线。
- 桌面端（发布构建）：窗口与界面内容正确；点"使用浏览器登录"后进入等待状态并打开系统浏览器；关窗收起后由第二个实例唤回。
- 真实浏览器（Chromium 内核）里的登录页：登录成功。
- 服务端带着打开的事件流收到 `SIGTERM` 后 0.02 秒退出。
- 抓到并修复了四条缺陷，见决策 0002。

## 还没有做过的

- **在桌面应用里完成整条登录**（点登录 → 浏览器确认 → 应用里显示已登录和设备列表）。2026-11-24 做到一半时屏幕锁定了，界面层面的操作做不下去。各段分别验证过，但没有一口气走完。
- 退出时的清理（⌘Q 之后替身设备看到本机离线）。
- 系统钥匙串的读写。
- 托盘菜单的逐项操作、开机启动开关、系统通知。
- Windows。
