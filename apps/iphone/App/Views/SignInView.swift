import AssembleCore
import SwiftUI

/// 登录页：扫电脑上的二维码，或者把它显示的那一串粘贴进来。密码从不在这里出现。
struct SignInView: View {
    @Environment(AppModel.self) private var model
    @State private var sheet: Sheet?

    enum Sheet: String, Identifiable {
        case scan
        case paste

        var id: String { rawValue }
    }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(spacing: 24) {
                    Image(systemName: "square.grid.2x2")
                        .font(.system(size: 56, weight: .light))
                        .foregroundStyle(.tint)
                        .padding(.top, 32)
                        .accessibilityHidden(true)
                    Text(Words.signInTitle)
                        .font(.largeTitle.bold())
                    Text(Words.signInBody)
                        .multilineTextAlignment(.center)
                        .foregroundStyle(.secondary)

                    progress

                    VStack(spacing: 12) {
                        Button(Words.scan) { sheet = .scan }
                            .buttonStyle(.borderedProminent)
                            .controlSize(.large)
                            .accessibilityIdentifier("signin.scan")
                        Button(Words.paste) { sheet = .paste }
                            .buttonStyle(.bordered)
                            .controlSize(.large)
                            .accessibilityIdentifier("signin.paste")
                    }
                    .disabled(isPairing)

                    if let problem = model.problem {
                        Text(problem)
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                            .multilineTextAlignment(.center)
                            .accessibilityIdentifier("signin.problem")
                    }
                }
                .frame(maxWidth: 420)
                .frame(maxWidth: .infinity)
                .padding(24)
            }
            .navigationTitle(Words.app)
            .navigationBarTitleDisplayMode(.inline)
            .sheet(item: $sheet) { sheet in
                switch sheet {
                case .scan:
                    ScannerSheet()
                case .paste:
                    PasteSheet()
                }
            }
        }
    }

    private var isPairing: Bool {
        model.pairing != .idle && model.pairing != .failed("")
    }

    /// 配对走到了哪一步。
    @ViewBuilder
    private var progress: some View {
        switch model.pairing {
        case .idle:
            EmptyView()
        case .claiming:
            VStack(spacing: 12) {
                ProgressView()
                Text(Words.claiming)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                Button(Words.cancel) { model.cancelPairing() }
                    .buttonStyle(.bordered)
            }
            .padding(.top, 8)
            .accessibilityIdentifier("pairing.claiming")
        case let .confirm(code, _):
            VStack(spacing: 12) {
                Text(Words.confirmTitle)
                    .font(.title3.bold())
                Text(code)
                    .font(.system(size: 40, weight: .semibold, design: .monospaced))
                    .kerning(6)
                    .accessibilityIdentifier("pairing.code")
                Text(Words.confirmBody)
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                ProgressView()
            }
            .padding(.top, 8)
        case let .failed(message):
            VStack(spacing: 12) {
                Image(systemName: "exclamationmark.triangle")
                    .font(.title2)
                    .foregroundStyle(.orange)
                    .accessibilityHidden(true)
                Text(message)
                    .font(.callout)
                    .multilineTextAlignment(.center)
                    .accessibilityIdentifier("pairing.failed")
                Button(Words.startOver) { model.cancelPairing() }
                    .buttonStyle(.bordered)
            }
            .padding(.top, 8)
        }
    }
}

/// 扫不了码的时候：把电脑上显示的那一串粘贴进来。
struct PasteSheet: View {
    @Environment(\.dismiss) private var dismiss
    @Environment(AppModel.self) private var model
    @State private var address = ""

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField(Words.pastePrompt, text: $address, axis: .vertical)
                        .lineLimit(2...4)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .keyboardType(.URL)
                        .textInputAutocapitalization(.never)
                        .accessibilityIdentifier("pairing.address")
                        .onSubmit(begin)
                } footer: {
                    Text("形如 assemble://pair?ticket=…")
                }
            }
            .navigationTitle(Words.paste)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(Words.cancel) { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button(Words.continueWord, action: begin)
                        .disabled(address.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                        .accessibilityIdentifier("pairing.continue")
                }
            }
        }
    }

    private func begin() {
        let text = address
        guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        dismiss()
        model.beginPairing(with: text)
    }
}

/// 用摄像头扫二维码；这台设备没有摄像头（比如模拟器）时明说，并请用户改用粘贴。
struct ScannerSheet: View {
    @Environment(\.dismiss) private var dismiss
    @Environment(AppModel.self) private var model
    @State private var unavailable = false

    var body: some View {
        NavigationStack {
            Group {
                if unavailable {
                    ContentUnavailableView(
                        Words.noCamera,
                        systemImage: "camera.metering.unknown"
                    )
                } else {
                    QRScannerView(
                        onCode: { code in
                            dismiss()
                            model.beginPairing(with: code)
                        },
                        onUnavailable: { unavailable = true }
                    )
                    .ignoresSafeArea()
                }
            }
            .navigationTitle(Words.scan)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(Words.cancel) { dismiss() }
                }
            }
        }
    }
}
