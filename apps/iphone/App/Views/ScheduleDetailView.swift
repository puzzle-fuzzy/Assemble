import AssembleCore
import SwiftUI

/// 一条日程：什么时候、重复不重复、提前多久提醒、地点、备注；能完成、能取消。
struct ScheduleDetailView: View {
    @Environment(AppModel.self) private var model
    let schedule: Schedule
    @State private var confirmingCancel = false

    private var now: Int64 {
        Int64(Date().timeIntervalSince1970 * 1000)
    }

    /// 接下来的几场。
    private var upcoming: [Int64] {
        Recurrence(schedule: schedule).upcoming(from: now, limit: 5)
    }

    var body: some View {
        List {
            Section {
                Text(schedule.title)
                    .font(.title2.weight(.semibold))
                if let location = schedule.location {
                    VStack(alignment: .leading, spacing: 2) {
                        Label(location.label, systemImage: "mappin.and.ellipse")
                        Text(location.address)
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                    }
                }
            }

            Section {
                row("开始", Words.on(schedule.startAt))
                if let end = schedule.endAt {
                    row("结束", Words.on(end))
                }
                row("重复", Words.repeatRule(schedule.repeatRule, until: schedule.repeatUntil))
                if let remind = schedule.remindBefore, remind > 0 {
                    row("提醒", Words.remindBefore(Int64(remind)))
                }
            }

            if !upcoming.isEmpty {
                Section("接下来的场次") {
                    ForEach(upcoming, id: \.self) { occurrence in
                        Text(Words.on(WireTime(millis: occurrence)))
                    }
                }
            }

            if let notes = schedule.notes {
                Section("备注") {
                    Text(notes)
                }
            }

            Section {
                Text("\(Words.createdBy) \(schedule.createdBy)")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            }

            if schedule.status == .upcoming {
                Section {
                    if schedule.repeatRule == .none {
                        Button {
                            Task { await model.complete(schedule) }
                        } label: {
                            Label(Words.complete, systemImage: "checkmark.circle")
                        }
                        .accessibilityIdentifier("detail.complete")
                    }
                    Button(role: .destructive) {
                        confirmingCancel = true
                    } label: {
                        Label(Words.cancelSchedule, systemImage: "xmark.circle")
                    }
                    .accessibilityIdentifier("detail.cancel")
                }
            }
        }
        .listStyle(.insetGrouped)
        .navigationTitle(schedule.title)
        .navigationBarTitleDisplayMode(.inline)
        .confirmationDialog(Words.cancelSchedule, isPresented: $confirmingCancel, titleVisibility: .visible) {
            Button(Words.cancelSchedule, role: .destructive) {
                Task { await model.cancel(schedule) }
            }
        }
    }

    private func row(_ title: String, _ value: String) -> some View {
        LabeledContent(title) {
            Text(value)
                .multilineTextAlignment(.trailing)
        }
    }
}

/// 一条提醒：什么时候到点、备注；能标成看过了，能不办了。
struct ReminderDetailView: View {
    @Environment(AppModel.self) private var model
    let reminder: Reminder

    var body: some View {
        List {
            Section {
                Text(reminder.title)
                    .font(.title2.weight(.semibold))
                LabeledContent("到点") {
                    Text(Words.on(reminder.dueAt))
                }
            }
            if let notes = reminder.notes {
                Section("备注") {
                    Text(notes)
                }
            }
            Section {
                Text("\(Words.createdBy) \(reminder.createdBy)")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            }
            if reminder.status == .scheduled {
                Section {
                    Button {
                        Task { await model.dismiss(reminder) }
                    } label: {
                        Label(Words.dismiss, systemImage: "checkmark.circle")
                    }
                    Button(role: .destructive) {
                        Task { await model.cancel(reminder) }
                    } label: {
                        Label(Words.cancelReminder, systemImage: "xmark.circle")
                    }
                }
            }
        }
        .listStyle(.insetGrouped)
        .navigationTitle(reminder.title)
        .navigationBarTitleDisplayMode(.inline)
    }
}
