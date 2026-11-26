import AssembleCore
import Foundation

/// 应用里给人看的文字。界面默认简体中文。
enum Words {
    static let app = "Assemble"

    // ---- 登录 ----
    static let signInTitle = "登录 Assemble"
    static let signInBody = "在已登录的电脑上打开账户页，点“添加手机”，再用这里扫它显示的二维码。"
    static let scan = "扫描二维码"
    static let paste = "粘贴配对地址"
    static let pastePrompt = "把电脑上显示的那一串粘贴到这里"
    static let continueWord = "继续"
    static let claiming = "正在认领这张二维码…"
    static let confirmTitle = "回到电脑上核对"
    static let confirmBody = "手机上显示的六位数字和电脑上的一样时，在电脑上点“允许”。这里会自己接着往下走。"
    static let cancel = "取消"
    static let startOver = "重新开始"
    static let badPairingAddress = "这不是配对地址。电脑上“扫不了码？”下面那一串才是。"
    static let pairingDenied = "电脑上点了“拒绝”。确认是你本人在操作后，重新扫一次。"
    static let pairingExpired = "这次配对已经过期。在电脑上重新生成一张二维码。"
    static let noCamera = "这台设备没有可用的摄像头。把电脑上显示的那一串粘贴进来也一样。"

    // ---- 名单 ----
    static let notifications = "没读的通知"
    static let schedules = "日程"
    static let reminders = "提醒"
    static let nothingScheduled = "近七天没有安排。"
    static let nothingUnread = "没有没读的通知。"
    static let markAllRead = "全部标成已读"
    static let loading = "正在读取名单…"
    static let refreshing = "正在同步…"

    // ---- 一条 ----
    static func remindBefore(_ minutes: Int64) -> String {
        switch minutes {
        case 1: "提前 1 分钟提醒"
        case ..<60: "提前 \(minutes) 分钟提醒"
        case 60: "提前 1 小时提醒"
        case ..<1440 where minutes % 60 == 0: "提前 \(minutes / 60) 小时提醒"
        case 1440: "提前 1 天提醒"
        case ..<1440: "提前 \(minutes / 60) 小时 \(minutes % 60) 分钟提醒"
        default: "提前 \(minutes / 1440) 天提醒"
        }
    }
    static func repeatRule(_ rule: Repeat, until: WireTime?) -> String {
        let word: String
        switch rule {
        case .none: word = "不重复"
        case .daily: word = "每天"
        case .weekly: word = "每周"
        case .monthly: word = "每月"
        }
        if let until {
            return "\(word)，到 \(on(until)) 为止"
        }
        return word
    }
    static let complete = "完成"
    static let cancelSchedule = "取消这条日程"
    static let dismiss = "看过了"
    static let cancelReminder = "不办了"
    static let createdBy = "记下它的"

    // ---- 账户 ----
    static let account = "账户"
    static let thisDevice = "这台手机"
    static let remindersToggle = "到点提醒"
    static let remindersHint = "系统会提前排好近三十天的提醒；关掉就只剩名单。"
    static let signOut = "退出登录"
    static let signOutConfirm = "退出后这台手机不再是账户里的设备，要重新扫码才能进来。"
    static let notificationsDenied = "系统没有允许通知。到“设置 → 通知”里为 Assemble 打开，再回来打开这个开关。"
    static let openSettings = "打开设置"
    static let service = "服务"

    // ---- 出错 ----
    static func describe(_ error: Error) -> String {
        guard let error = error as? APIError else {
            return "出了点问题：\(error.localizedDescription)"
        }
        switch error {
        case .network: return "连不上服务。看看网络，或者服务是不是在维护。"
        case .timeout: return "服务太久没有应答。稍后再试。"
        case .cancelled: return "已取消。"
        case .badResponse, .tooLarge: return "服务给的回应读不出来。应用可能太旧，更新一下。"
        case .redirected: return "服务想把这台手机送去别的地址，已拦下。"
        case .unauthorized: return "登录已经失效。"
        case .notFound: return "那一条已经不在了。"
        case .invalidCode: return pairingExpired
        case .tooManyAttempts: return "试的次数太多，过一分钟再来。"
        case .versionConflict: return "那一条刚被别处改过。名单已重新读取，看一眼再操作。"
        case .invalidState: return "那一条现在的状态不能这么做。名单已重新读取。"
        case .invalidRequest: return "这个操作服务不接受。"
        case .forbidden: return "这台设备没有权限这么做。"
        case .http(let status, _): return "服务出了状况（\(status)）。稍后再试。"
        }
    }
    static let sessionExpired = "登录已经失效：这台手机可能已从账户里被移除。重新扫码就能进来。"
    static let signOutUnreachable = "暂时连不上服务，还没有退出。连上后再试一次。"
    static let storedSessionUnreadable = "保存的登录读不出来。重新扫码就能进来。"

    // ---- 时间 ----
    private static let zone = TimeZone.current
    private static var calendar: Calendar {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = zone
        return calendar
    }

    static func on(_ time: WireTime) -> String {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "zh_CN")
        formatter.calendar = calendar
        formatter.timeZone = zone
        formatter.dateFormat = "yyyy年M月d日 HH:mm"
        return formatter.string(from: time.date)
    }

    static func short(_ time: WireTime) -> String {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "zh_CN")
        formatter.calendar = calendar
        formatter.timeZone = zone
        formatter.setLocalizedDateFormatFromTemplate("MdHHmm")
        return formatter.string(from: time.date)
    }

    static func time(_ millis: Int64) -> String {
        on(WireTime(millis: millis))
    }

    static func clock(_ millis: Int64) -> String {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "zh_CN")
        formatter.calendar = calendar
        formatter.timeZone = zone
        formatter.dateFormat = "HH:mm"
        return formatter.string(from: Date(timeIntervalSince1970: TimeInterval(millis) / 1000))
    }

    static func day(_ millis: Int64) -> String {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "zh_CN")
        formatter.calendar = calendar
        formatter.timeZone = zone
        formatter.dateFormat = "M月d日"
        return formatter.string(from: Date(timeIntervalSince1970: TimeInterval(millis) / 1000))
    }

    /// 离某个时刻还有多久，或者已经过了多久。
    static func relative(to millis: Int64, now: Int64) -> String {
        let gap = millis - now
        let magnitude = abs(gap)
        // 向上取整：一天多说明天到，不说"还有 1 天"。
        func rounded(_ unit: Int64) -> Int64 {
            (magnitude + unit - 1) / unit
        }
        let word: String
        if magnitude < 60_000 {
            word = "不到 1 分钟"
        } else if magnitude < 3_600_000 {
            word = "\(rounded(60_000)) 分钟"
        } else if magnitude < 86_400_000 {
            word = "\(rounded(3_600_000)) 小时"
        } else {
            word = "\(rounded(86_400_000)) 天"
        }
        return gap >= 0 ? "还有 \(word)" : "过了 \(word)"
    }
}
