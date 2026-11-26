import SwiftUI
import UIKit

/// 账户页：是谁、这台手机叫什么、到点提醒的开关、退出登录。
struct AccountView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var confirmingSignOut = false
    @State private var switchingReminders = false

    var body: some View {
        NavigationStack {
            Form {
                if let account = model.account {
                    Section(Words.account) {
                        LabeledContent("账户", value: account.displayName)
                        if let email = account.email {
                            LabeledContent("邮箱", value: email)
                        }
                    }
                }
                if let device = model.device {
                    Section(Words.thisDevice) {
                        Text(device.name)
                    }
                }
                Section {
                    Toggle(isOn: remindersBinding) {
                        Text(Words.remindersToggle)
                    }
                    .disabled(switchingReminders)
                    .accessibilityIdentifier("account.reminders")
                } footer: {
                    if model.remindersDenied {
                        Text(Words.notificationsDenied)
                        Button(Words.openSettings) {
                            // 系统的"设置"里这一页才能重新允许通知；应用里没法替用户改。
                            if let url = URL(string: UIApplication.openSettingsURLString) {
                                UIApplication.shared.open(url)
                            }
                        }
                        .font(.footnote)
                    } else {
                        Text(Words.remindersHint)
                    }
                }
                Section(Words.service) {
                    Text(model.serviceLabel)
                }
                Section {
                    Button(role: .destructive) {
                        confirmingSignOut = true
                    } label: {
                        Text(Words.signOut)
                            .frame(maxWidth: .infinity)
                    }
                    .accessibilityIdentifier("account.signOut")
                }
            }
            .navigationTitle(Words.account)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("完成") { dismiss() }
                }
            }
            .confirmationDialog(Words.signOut, isPresented: $confirmingSignOut, titleVisibility: .visible) {
                Button(Words.signOut, role: .destructive) {
                    Task {
                        await model.signOut()
                        dismiss()
                    }
                }
            } message: {
                Text(Words.signOutConfirm)
            }
            .onChange(of: model.phase) { _, phase in
                // 在别处（比如令牌失效）退出了登录：这一页也收起来。
                if phase != .signedIn {
                    dismiss()
                }
            }
        }
    }

    private var remindersBinding: Binding<Bool> {
        Binding(
            get: { model.remindersEnabled },
            set: { value in
                Task {
                    switchingReminders = true
                    await model.setReminders(enabled: value)
                    switchingReminders = false
                }
            }
        )
    }
}
