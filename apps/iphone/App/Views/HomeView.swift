import AssembleCore
import SwiftUI

/// 登录之后的第一页：没读的通知、近七天的日程、还没到点与过了点的提醒。
struct HomeView: View {
    @Environment(AppModel.self) private var model
    @State private var showingAccount = false
    @State private var opened: Opened?

    enum Opened: Hashable {
        case schedule(Schedule)
        case reminder(Reminder)
    }

    private var now: Int64 {
        Int64(Date().timeIntervalSince1970 * 1000)
    }

    /// 近七天里有场次的日程，按最近的一场排；已经没有场次的排在后面。
    private var schedules: [Schedule] {
        let now = self.now
        let horizon = now + 7 * DAY_MS
        return (model.planner?.schedules ?? [])
            .filter { $0.status == .upcoming }
            .map { schedule -> (Schedule, Int64?) in
                let rule = Recurrence(schedule: schedule)
                let next = rule.upcoming(from: now - schedule.leadMinutes * 60_000, limit: 1).first
                let shown = next.flatMap { $0 <= horizon ? $0 : nil }
                return (schedule, shown)
            }
            .sorted { lhs, rhs in
                switch (lhs.1, rhs.1) {
                case let (l?, r?): return l != r ? l < r : lhs.0.title < rhs.0.title
                case (nil, _?): return false
                case (_?, nil): return true
                case (nil, nil): return lhs.0.startAt.millis < rhs.0.startAt.millis
                }
            }
            .map(\.0)
    }

    private var reminders: [Reminder] {
        (model.planner?.reminders ?? [])
            .filter { $0.status == .scheduled }
            .sorted { $0.dueAt.millis < $1.dueAt.millis }
    }

    var body: some View {
        List {
            if let problem = model.problem {
                Section {
                    Label(problem, systemImage: "exclamationmark.circle")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                        .accessibilityIdentifier("home.problem")
                }
            }

            if let planner = model.planner {
                if !planner.notifications.isEmpty {
                    Section {
                        ForEach(planner.notifications) { notification in
                            NotificationRow(notification: notification, now: now) {
                                Task { await model.markRead([notification.id]) }
                            }
                            .accessibilityIdentifier("notification.row.\(notification.id)")
                        }
                    } header: {
                        Text(Words.notifications)
                    } footer: {
                        Button(Words.markAllRead) {
                            Task { await model.markAllRead() }
                        }
                        .font(.footnote)
                        .accessibilityIdentifier("home.markAllRead")
                    }
                }

                Section(Words.schedules) {
                    if schedules.isEmpty {
                        Text(Words.nothingScheduled)
                            .foregroundStyle(.secondary)
                    }
                    ForEach(schedules) { schedule in
                        NavigationLink {
                            ScheduleDetailView(schedule: schedule)
                        } label: {
                            ScheduleRow(schedule: schedule, now: now)
                        }
                        .accessibilityIdentifier("schedule.row.\(schedule.id)")
                    }
                }

                Section(Words.reminders) {
                    if reminders.isEmpty {
                        Text(Words.nothingScheduled)
                            .foregroundStyle(.secondary)
                    }
                    ForEach(reminders) { reminder in
                        // 点一行进详情：备注看全，也有"看过了""不办了"两个按钮，不用非得左滑。
                        NavigationLink {
                            ReminderDetailView(reminder: reminder)
                        } label: {
                            ReminderRow(reminder: reminder, now: now)
                        }
                        .swipeActions(edge: .trailing, allowsFullSwipe: false) {
                            Button(role: .destructive) {
                                Task { await model.cancel(reminder) }
                            } label: {
                                Label(Words.cancelReminder, systemImage: "xmark.circle")
                            }
                            Button {
                                Task { await model.dismiss(reminder) }
                            } label: {
                                Label(Words.dismiss, systemImage: "checkmark.circle")
                            }
                            .tint(.green)
                        }
                        .accessibilityIdentifier("reminder.row.\(reminder.id)")
                    }
                }
            } else {
                Section {
                    HStack {
                        Spacer()
                        ProgressView {
                            Text(Words.loading)
                        }
                        Spacer()
                    }
                    .padding()
                }
            }
        }
        .listStyle(.insetGrouped)
        .navigationTitle(Words.app)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button {
                    showingAccount = true
                } label: {
                    Image(systemName: "person.crop.circle")
                }
                .accessibilityLabel(Words.account)
                .accessibilityIdentifier("home.account")
            }
        }
        .refreshable { await model.refresh() }
        .sheet(isPresented: $showingAccount) {
            AccountView()
        }
        .navigationDestination(item: $opened) { target in
            switch target {
            case let .schedule(schedule):
                ScheduleDetailView(schedule: schedule)
            case let .reminder(reminder):
                ReminderDetailView(reminder: reminder)
            }
        }
        .onChange(of: model.focus) { _, focus in
            guard let focus else { return }
            if let id = focus.scheduleID, let schedule = model.planner?.schedules.first(where: { $0.id == id }) {
                opened = .schedule(schedule)
            } else if let id = focus.reminderID, let reminder = model.planner?.reminders.first(where: { $0.id == id }) {
                opened = .reminder(reminder)
            }
            model.consumeFocus()
        }
        .onChange(of: model.planner) { _, planner in
            // 这一条办完了（这一页上的按钮，或者别的设备）：详情跟着收起来，名单里它已经不在了。
            guard let opened else { return }
            let stillListed: Bool
            switch opened {
            case let .schedule(schedule):
                stillListed = planner?.schedules.contains { $0.id == schedule.id && $0.status == .upcoming } ?? true
            case let .reminder(reminder):
                stillListed = planner?.reminders.contains { $0.id == reminder.id && $0.status == .scheduled } ?? true
            }
            if !stillListed {
                self.opened = nil
            }
        }
    }
}

extension NotificationTarget {
    var scheduleID: String? {
        if case let .schedule(id, _) = self { id }
        else { nil }
    }

    var reminderID: String? {
        if case let .reminder(id) = self { id }
        else { nil }
    }
}

/// 一条还没读的通知。
struct NotificationRow: View {
    let notification: PlannerNotification
    let now: Int64
    let onRead: () -> Void

    var body: some View {
        HStack(alignment: .firstTextBaseline) {
            Image(systemName: notification.kind == .schedule ? "calendar" : "bell")
                .foregroundStyle(.tint)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 4) {
                Text(notification.title)
                    .font(.body.weight(.medium))
                if let body = notification.body {
                    Text(body)
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                        .lineLimit(3)
                }
                Text(Words.relative(to: notification.raisedAt.millis, now: now))
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }
            Spacer(minLength: 0)
            Button(Words.dismiss, action: onRead)
                .buttonStyle(.bordered)
                .controlSize(.small)
                .accessibilityLabel("\(Words.dismiss)：\(notification.title)")
        }
        .padding(.vertical, 2)
    }
}

/// 名单里的一条日程：最近的一场是什么时候，重复不重复。
struct ScheduleRow: View {
    let schedule: Schedule
    let now: Int64

    private var next: Int64? {
        Recurrence(schedule: schedule)
            .upcoming(from: now - schedule.leadMinutes * 60_000, limit: 1)
            .first
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(schedule.title)
                .font(.body.weight(.medium))
            if let next {
                Text("\(Words.relative(to: next, now: now)) · \(Words.on(WireTime(millis: next)))")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            } else {
                Text("已经没有接下来的场次")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            }
            if schedule.repeatRule != .none || schedule.remindBefore != nil {
                Text(
                    [
                        schedule.repeatRule != .none ? Words.repeatRule(schedule.repeatRule, until: schedule.repeatUntil) : nil,
                        schedule.remindBefore.map { Words.remindBefore(Int64($0)) },
                    ].compactMap { $0 }.joined(separator: " · ")
                )
                .font(.caption)
                .foregroundStyle(.tertiary)
            }
        }
        .padding(.vertical, 2)
    }
}

/// 名单里的一条提醒。
struct ReminderRow: View {
    let reminder: Reminder
    let now: Int64

    private var overdue: Bool {
        reminder.dueAt.millis < now
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(reminder.title)
                .font(.body.weight(.medium))
            Text("\(Words.relative(to: reminder.dueAt.millis, now: now)) · \(Words.on(reminder.dueAt))")
                .font(.footnote)
                .foregroundStyle(overdue ? .orange : .secondary)
            if let notes = reminder.notes {
                Text(notes)
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
            }
        }
        .padding(.vertical, 2)
    }
}
