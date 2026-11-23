# 在 macOS 上真实启动并检查 Assemble

自动化测试覆盖不到窗口、托盘、系统钥匙串和退出时的清理。涉及应用壳、本机网关或生命周期的改动，除了 `bun run verify`，还要按本文实际跑一遍。

这个仓库里已经有过教训：有两个缺陷是单元测试全部通过、实际启动之后才暴露的。

## 1. 构建并启动

```sh
cd apps/desktop
bunx tauri build --no-bundle     # 只产出可执行文件，最快
cd ../..
RUST_LOG=info ./target/release/assemble-desktop 2>&1 | tee /tmp/assemble-run.log
```

启动后应该看到一行 `local bridge started url=http://127.0.0.1:<端口>`。

## 2. 检查本机网关

地址和令牌在应用数据目录的 `bridge.json` 里。下面的写法不会把令牌打印出来。

```sh
F="$HOME/Library/Application Support/com.puzzlefuzzy.assemble/bridge.json"
URL=$(python3 -I -c "import json;print(json.load(open('$F'))['url'])")
TOKEN=$(python3 -I -c "import json;print(json.load(open('$F'))['token'])")

curl -s -H "Authorization: Bearer $TOKEN" "$URL/status"
curl -s -H "Authorization: Bearer $TOKEN" "$URL/capabilities"
```

对照三点：

- `status` 在未登录时 `connected` 为 `false`；服务可达时没有 `error` 字段。
- `capabilities` 列出的必须恰好是本构建真正提供的操作。
- 文件权限是 `600`。

拒绝规则各验一次：

```sh
# 期望 401
curl -s -o /dev/null -w "%{http_code}\n" -H "Authorization: Bearer wrong" "$URL/status"
# 期望 403：带 Origin 的请求来自浏览器环境
curl -s -o /dev/null -w "%{http_code}\n" -H "Authorization: Bearer $TOKEN" -H "Origin: http://127.0.0.1" "$URL/status"
```

## 3. 看窗口与托盘

窗口编号用一段 CoreGraphics 脚本按进程名查出来。把它存到仓库之外的某个文件，用 `swift <文件>` 运行。

```swift
import CoreGraphics
import Foundation

let options: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
let list = CGWindowListCopyWindowInfo(options, kCGNullWindowID) as! [[String: Any]]
for window in list {
    let owner = window[kCGWindowOwnerName as String] as? String ?? ""
    guard owner.lowercased().contains("assemble") else { continue }
    print(window[kCGWindowNumber as String] ?? 0, window[kCGWindowName as String] ?? "", window[kCGWindowBounds as String] ?? "")
}
```

只截取应用自己的窗口，不要截整个屏幕：

```sh
screencapture -x -o -l <窗口编号> /tmp/assemble-window.png
```

托盘图标在菜单栏右侧状态区的最左边。

不要点"使用浏览器登录"，除非这次就是要做真实登录验收：它会对正式服务建立一条真实的设备会话，并写入系统钥匙串。

## 4. 退出并确认清理

不带应用包的可执行文件不响应 AppleScript 的退出请求，要把窗口置前后发送 ⌘Q：

```sh
osascript -e 'tell application "System Events"' \
  -e 'set frontmost of process "assemble-desktop" to true' \
  -e 'delay 0.5' \
  -e 'keystroke "q" using command down' \
  -e 'end tell'
```

`.app` 应用包可以直接用 `tell application id "com.puzzlefuzzy.assemble" to quit`。

期望的结果：进程退出，`bridge.json` 被删除。这两条要分别确认——曾经出现过进程正常退出、文件却还在的情况。

## 2026-11-23 做过的

- 裸二进制与 `.app` 各启动一次，第 2、3、4 步全部符合预期。
- `bun run build` 跑通，产出 `Assemble.app` 与 `Assemble_0.1.0_aarch64.dmg`。

## 还没有做过的

这些需要你亲自操作，或者需要签名后的应用包：

- 真实登录：浏览器授权、换取设备会话、重启后恢复、退出登录。
- 托盘菜单的逐项操作。
- 开机启动开关的实际拨动。
- 系统通知。
- Windows。
