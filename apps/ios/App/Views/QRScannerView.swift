import AVFoundation
import SwiftUI

/// 摄像头扫码：只认二维码，扫到一个就停。
struct QRScannerView: UIViewControllerRepresentable {
    let onCode: (String) -> Void
    let onUnavailable: () -> Void

    func makeUIViewController(context: Context) -> ScannerController {
        let controller = ScannerController()
        controller.onCode = onCode
        controller.onUnavailable = onUnavailable
        return controller
    }

    func updateUIViewController(_ controller: ScannerController, context: Context) {}

    @MainActor
    final class ScannerController: UIViewController {
        var onCode: ((String) -> Void)?
        var onUnavailable: (() -> Void)?

        // 摄像头的会话要在后台线程上启动；把它装进一个可以跨线程交出去的盒子。
        private let camera = CameraSession()
        private let reader = CodeReader()
        private var preview: AVCaptureVideoPreviewLayer?
        private var fired = false

        override func viewDidLoad() {
            super.viewDidLoad()
            view.backgroundColor = .black
            reader.controller = self
            let session = camera.session
            guard let device = AVCaptureDevice.default(for: .video),
                  let input = try? AVCaptureDeviceInput(device: device),
                  session.canAddInput(input)
            else {
                onUnavailable?()
                return
            }
            session.addInput(input)
            let output = AVCaptureMetadataOutput()
            guard session.canAddOutput(output) else {
                onUnavailable?()
                return
            }
            session.addOutput(output)
            // 回调交到主队列上：界面只在那里动。
            output.setMetadataObjectsDelegate(reader, queue: .main)
            output.metadataObjectTypes = [.qr]
            let preview = AVCaptureVideoPreviewLayer(session: session)
            preview.videoGravity = .resizeAspectFill
            view.layer.addSublayer(preview)
            self.preview = preview
        }

        override func viewDidLayoutSubviews() {
            super.viewDidLayoutSubviews()
            preview?.frame = view.bounds
        }

        override func viewWillAppear(_ animated: Bool) {
            super.viewWillAppear(animated)
            camera.start()
        }

        override func viewWillDisappear(_ animated: Bool) {
            super.viewWillDisappear(animated)
            camera.stop()
        }

        /// 扫到了一个码。只认第一个：之后的都是同一张二维码的重复读数。
        func didScan(_ code: String) {
            guard !fired else { return }
            fired = true
            camera.stop()
            onCode?(code)
        }
    }
}

/// 摄像头的委托。它的方法不隔离，但队列是主队列，所以进了门就声明自己在主线程上，
/// 再去动界面那边的控制器。
final class CodeReader: NSObject, AVCaptureMetadataOutputObjectsDelegate, @unchecked Sendable {
    nonisolated(unsafe) weak var controller: QRScannerView.ScannerController?

    func metadataOutput(
        _ output: AVCaptureMetadataOutput,
        didOutput metadataObjects: [AVMetadataObject],
        from connection: AVCaptureConnection
    ) {
        // 先在原地把那一串取出来：它只是文字，可以带进主线程。
        guard let object = metadataObjects.first as? AVMetadataMachineReadableCodeObject,
              let value = object.stringValue
        else { return }
        MainActor.assumeIsolated {
            controller?.didScan(value)
        }
    }
}

/// 摄像头会话。启动与停止都是阻塞的，放到后台去做。
final class CameraSession: @unchecked Sendable {
    let session = AVCaptureSession()

    func start() {
        DispatchQueue.global(qos: .userInitiated).async { [self] in
            if !session.isRunning {
                session.startRunning()
            }
        }
    }

    func stop() {
        DispatchQueue.global(qos: .userInitiated).async { [self] in
            if session.isRunning {
                session.stopRunning()
            }
        }
    }
}
