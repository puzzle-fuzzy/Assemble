# Assemble 的 iOS 应用

第一期的范围（[决策 0014](../../docs/decisions/0014-iphone-client.md)）：看日程与提醒、到点靠本地通知响、点通知定位到那一条、扫电脑上的二维码登录。**不做**收发文件、云盘、影视、远程推送、后台刷新。密码从不进这个应用：登录只有扫码配对这一条路。

## 目录

| 位置                       | 是什么                                                                                                                                                                                                 |
| -------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `Package.swift`            | SwiftPM 包 `AssembleCore`：线上的类型、时间的解析、重复规则的推算、通知的规划、接口、配对、登录的保存。不依赖界面，`swift test` 在 Mac 上直接跑                                                        |
| `Sources/AssembleCore/`    | 核心包的源码                                                                                                                                                                                           |
| `Tests/AssembleCoreTests/` | 核心包的测试（Swift Testing）                                                                                                                                                                          |
| `Fixtures/`                | 样本：`protocol.json` 是线上的类型按各种取值序列化的样子，`recurrence.json` 是 Rust 那一份重复规则算法的推算结果。**不要手改**（Biome 也不碰这个目录）：它们由 Rust 侧的测试生成、按生成的样子逐字比对 |
| `project.yml`              | XcodeGen 的工程定义。工程文件 `Assemble.xcodeproj` 每次现生成，不进仓库                                                                                                                                |
| `App/`                     | 应用壳（SwiftUI）：`AppModel` 是全部状态与动作，`Views/` 是界面，`Words.swift` 是给人看的文字。图标在 `App/Assets.xcassets`，由根目录的 `bun run icons` 生成，不手改                                   |
| `AppTests/Unit/`           | 应用层的单元测试：对着替身把配对、恢复登录、退出、通知的排布走一遍                                                                                                                                     |
| `AppTests/UI/`             | 界面测试：在模拟器里真的把应用跑起来，走完整条路                                                                                                                                                       |

## 样本怎么更新

Rust 侧的两条测试平时只是逐字比对：

```sh
cargo test -p assemble-protocol -p assemble-schedule --test ios_fixtures
```

协议或重复规则改了，这两条会红。确认改动是有意的之后刷新样本，再跑 Swift 这边的测试：

```sh
ASSEMBLE_UPDATE_FIXTURES=1 cargo test -p assemble-protocol -p assemble-schedule --test ios_fixtures
bun run ios:test
```

Swift 测试应当跟着改：样本变了就意味着线上的格式变了。

## 怎么开发

```sh
bun run ios:test            # 核心包的测试，不用模拟器
bun run ios:simulate        # 在模拟器里走完整条路（见下）
```

想在模拟器里手点：先 `xcodegen generate`，再用 Xcode 打开 `Assemble.xcodeproj`，选一个 iPhone 模拟器运行。应用默认连本机的开发服务（`http://127.0.0.1:8787`，用 `bun run server:dev` 起），登录页上"粘贴配对地址"可以代替摄像头：在电脑的浏览器里登录 `http://127.0.0.1:8787/web/account` →"添加手机"，把页面上那一串粘贴进模拟器。要改连哪个服务，重新生成工程：

```sh
ASSEMBLE_SERVER_ORIGIN=https://… xcodegen generate
```

服务地址在构建时定下，应用里没有可以填地址的地方（和桌面端同一条规矩）。连本机开发服务走的是明文 HTTP，所以 Info.plist 里允许了本地网络；正式服务是 HTTPS，用不到这一条。

## `bun run ios:simulate` 做什么

1. 找到要用的模拟器（默认 `iPhone 17 Pro`），关着的直接启动、不开窗口；把模拟器里的应用卸掉，每一回都从一台"新手机"开始。
2. 编译服务端，生成工程，先把应用和测试构建好。构建失败时还没起服务端，也没有东西要收摊。
3. 用一次性的数据目录在回环地址上起一个服务端（端口从一个随机的位置往后找），建一个测试账户：密码随机、经标准输入，邮箱标成已验证——配对只对验证过的账户开放。
4. 跑测试：先是应用层的单元测试，再是界面测试。界面测试自己扮演"电脑那一侧"：用测试账户登录网页、生成二维码、放一条两天后的日程和一条一分多钟之后到点的提醒（它被安排在服务端刚查完到点之后，所以点开横幅时名单里它已经是"已到点"——真人多是通知响了一会儿才点）；然后在模拟器里把配对地址粘贴进应用，核对应用显示的六位数字，再在"电脑"上点允许。接着在应用里打开到点提醒（系统的"允许通知"框由测试替人点），把应用切到后台，等系统通知弹出来，点它，看应用是不是回到前台并定位到那条提醒，标成看过了，等它的通知也到了之后再读一次名单（不能再冒出来），最后回服务端核对那一条的状态。
5. 结束后停服务端、删临时目录和带密码的测试配置；日志和界面测试留下的截图在 `.artifacts/ios/<时间>/`。

它不打开模拟器的窗口，不碰这台 Mac 的系统钥匙串（模拟器的钥匙串是它自己的），也不动"应用程序"里装着的桌面端。模拟器里点不出摄像头，所以走的是"粘贴配对地址"这条路。真机上用摄像头扫码、锁屏上通知的样子，只有真机回答得了。

几个开关：

```sh
ASSEMBLE_IOS_ONLY=AssembleTests bun run ios:simulate     # 只跑单元测试（或 AssembleUITests）
ASSEMBLE_IOS_KEEP_APP=1 bun run ios:simulate             # 不卸应用：走"更新之后通知的权限还在"的那条路
ASSEMBLE_IOS_SIMULATOR="iPhone Air" bun run ios:simulate
```

应用先卸再装清的是通知的权限和本机的账本；模拟器的钥匙串不随应用卸掉，存下来的登录按服务地址区分，所以服务端每一回换一个端口，上一回的登录就读不到。

## 装到自己的 iPhone 上

需要 Xcode 里登录着 Apple ID。免费账户签出来的安装七天要重装一次；买了开发者账户之后改走 TestFlight。

```sh
cd apps/ios
DEVELOPMENT_TEAM=<团队号> ASSEMBLE_SERVER_ORIGIN=https://… xcodegen generate
xcodebuild -project Assemble.xcodeproj -scheme Assemble -configuration Debug \
  -destination 'id=<设备的标识>' -derivedDataPath .build/DerivedData \
  -allowProvisioningUpdates build
xcrun devicectl device install app --device <设备的标识> \
  .build/DerivedData/Build/Products/Debug-iphoneos/Assemble.app
```

`xcrun devicectl list devices` 列出配对过的设备和它们的标识；手机和 Mac 在同一个 Wi-Fi 里时可以走无线，不用插线。团队号写在 Xcode 的"Settings → Accounts"里；它和设备标识都不进仓库。图标是和桌面端同一个占位点阵（根目录 `bun run icons` 生成），上架之前要换成正式的。

**手机上连哪个服务要先想好。** 模拟器能连本机回环地址，真机连不上：它只能连手机够得着的服务。连正式服务之前，按[决策 0014](../../docs/decisions/0014-iphone-client.md)的顺序来——正式服务先升级到认得 `ios` 这个平台值的版本，两台电脑上的桌面端再更新，然后才有手机登录；反过来，旧版桌面端读到 iPhone 这台设备会读不了设备名单。

## 代码怎么分

- `AppModel`（`App/AppModel.swift`）是唯一的状态来源。界面只读它、调它的方法。网络、钥匙串、系统通知、时钟都是它指名的依赖，测试换成替身。应用层的单元测试跑在应用自己的进程里，宿主应用真正的界面也在：要模拟"点开通知的消息没人收"，用 `NotificationTap.stash`，别用会广播的 `record`。
- `AssembleCore` 里没有界面、没有全局状态：`NotificationPlan` 是纯函数，`Recurrence` 是 Rust 那份算法的逐行移植，`AssembleAPI` 是一个不带 Cookie、不存凭据、不缓存、不跟随重定向的 `URLSession` 包装。
- 通知的标识就是服务端通知的标识（提醒 `n-<标识>`，日程 `<标识>-<场次开始的毫秒>`）。交给系统去响的标识记进账本：本机响过之后，服务端到点才生成的同一条通知不会再响第二遍；手机既没排过也没弹过的才补弹。
- 每一次异步回来都先核对"是不是同一轮"：退出登录之后，旧请求的结果一律丢掉。退出登录以服务端确认为准，服务端没说撤销，本机就不忘掉。"正在读名单""正在做改动"这两个标记不论怎么结束都要放回去——包括读到"不认"之后自己把登录忘掉那一下。
- 通知代理的两个方法用带回调的写法，不用 `async` 的那一种：那一种在后台线程上收尾，UIKit 跟着更新界面快照时会断言崩溃（2026-11-27 在模拟器里点开通知时抓到，每一次点开必崩）。
- 排进系统的通知用的是从排的那一刻起算的时间间隔，不是固定时区的日历触发：同样和时区、夏令时无关，代码更短；手机改了系统时间的话，那一批要等下一次同步才纠正。
- 办完一条日程或提醒，它到点的通知跟着标成已读，服务端晚一步才生成的那一条也算。"看过了"碰上服务端刚把提醒标成到点（版本号加一）时，照新的版本号再交一次。
- 详情页开着时以名单里最新的那一条为准（别处改过、到了点，版本号都会变）。提醒刚到了点不算办完：详情要留着等用户点"看过了"，不自己关上（`DetailPolicy`）。
