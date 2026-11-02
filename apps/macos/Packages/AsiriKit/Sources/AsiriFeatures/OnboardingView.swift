import AppKit
import AsiriDesign
import AsiriDomain
import AsiriPlatform
import SwiftUI
import UniformTypeIdentifiers

/// First launch (architecture §11.1): welcome, name and photo, app lock, then joining a friend's
/// server or setting one up.
struct OnboardingView: View {
    let app: AppModel
    @State private var step: Step = .welcome
    @State private var name = ""
    @State private var avatar: Data?
    @State private var lockEnabled = false
    @State private var path: Path?

    enum Step: Int, Comparable {
        case welcome, profile, security, connect
        static func < (a: Step, b: Step) -> Bool { a.rawValue < b.rawValue }
    }

    enum Path { case join, setUp }

    var body: some View {
        ZStack {
            OnboardingBackground()
            VStack(spacing: 0) {
                Spacer(minLength: 20)
                Group {
                    switch step {
                    case .welcome: welcome
                    case .profile: profile
                    case .security: security
                    case .connect: connect
                    }
                }
                .transition(.asymmetric(insertion: .move(edge: .trailing).combined(with: .opacity), removal: .move(edge: .leading).combined(with: .opacity)))
                .id(step)
                Spacer(minLength: 20)
                StepDots(count: 4, current: step.rawValue)
                    .padding(.bottom, 22)
            }
            .frame(maxWidth: 560)
            .padding(.horizontal, 32)
        }
        .environment(\.colorScheme, .dark)
        .animation(Motion.standard, value: step)
    }

    private func go(_ s: Step) {
        withAnimation(Motion.standard) { step = s }
    }

    // MARK: Welcome

    private var welcome: some View {
        VStack(spacing: 22) {
            AsiriMark(size: 112)
                .shadow(color: Palette.indigoBright.opacity(0.55), radius: 30, y: 10)
                .phaseAnimator([0.0, -8.0]) { mark, y in mark.offset(y: y) } animation: { _ in .easeInOut(duration: 2.2) }
            VStack(spacing: 6) {
                Text("Àṣírí")
                    .font(.system(size: 44, weight: .heavy, design: .rounded))
                    .foregroundStyle(.white)
                Text("Between us.")
                    .font(.system(size: 18, weight: .medium, design: .rounded))
                    .foregroundStyle(.white.opacity(0.75))
            }
            Text("Private messages and calls with the people you trust. No phone number, no email: you add friends by meeting them in person.")
                .font(.system(size: 14))
                .foregroundStyle(.white.opacity(0.7))
                .multilineTextAlignment(.center)
                .frame(maxWidth: 420)
            Button("Get started") { go(.profile) }
                .buttonStyle(PrimaryButtonStyle(tint: Palette.indigoBright))
                .keyboardShortcut(.defaultAction)
                .padding(.top, 6)
        }
    }

    // MARK: Profile

    private var profile: some View {
        VStack(spacing: 20) {
            OnboardingTitle("What should friends call you?", "Your name and photo are shared only with people you add, inside your encrypted chats.")
            Button(action: pickPhoto) {
                Avatar(id: Data(name.utf8), name: name.isEmpty ? "?" : name, image: avatar, size: 96)
                    .overlay(alignment: .bottomTrailing) {
                        Image(systemName: "camera.fill")
                            .font(.system(size: 12, weight: .bold))
                            .foregroundStyle(.white)
                            .frame(width: 28, height: 28)
                            .background(Circle().fill(Palette.accent))
                            .overlay(Circle().stroke(Palette.night, lineWidth: 3))
                    }
            }
            .buttonStyle(PressableStyle())
            .help("Choose a photo")
            TextField("Your name", text: $name)
                .textFieldStyle(.plain)
                .font(.system(size: 18, weight: .medium))
                .multilineTextAlignment(.center)
                .padding(.vertical, 10)
                .padding(.horizontal, 16)
                .frame(maxWidth: 320)
                .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color.white.opacity(0.1)))
                .foregroundStyle(.white)
                .onSubmit(saveProfile)
            Button("Continue", action: saveProfile)
                .buttonStyle(PrimaryButtonStyle(tint: Palette.indigoBright))
                .disabled(name.trimmingCharacters(in: .whitespaces).isEmpty)
                .keyboardShortcut(.defaultAction)
        }
    }

    private func pickPhoto() {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.image]
        panel.begin { response in
            guard response == .OK, let url = panel.url,
                  let file = try? MediaPrep.prepare(url: url),
                  let thumb = file.thumbnail
            else { return }
            Task { @MainActor in avatar = thumb }
        }
    }

    private func saveProfile() {
        do {
            try app.core.setProfile(name: name, avatar: avatar)
            go(.security)
        } catch {
            app.show(error)
        }
    }

    // MARK: Security

    private var security: some View {
        VStack(spacing: 20) {
            OnboardingTitle("Lock Asiri with Touch ID", "When it's on, Asiri asks for Touch ID or your Mac password before showing your chats. You can change this later in Settings.")
            Image(systemName: AppLock.hasBiometrics ? "touchid" : "lock.shield.fill")
                .font(.system(size: 64, weight: .light))
                .foregroundStyle(LinearGradient(colors: [Palette.amber, Color(hex: 0xF97316)], startPoint: .top, endPoint: .bottom))
                .symbolEffect(.pulse, isActive: lockEnabled)
            Toggle(isOn: $lockEnabled) {
                Text("Require Touch ID or password").foregroundStyle(.white)
            }
            .toggleStyle(.switch)
            .disabled(!AppLock.isAvailable)
            HStack(spacing: 12) {
                Button("Back") { go(.profile) }.buttonStyle(SecondaryButtonStyle())
                Button("Continue") {
                    Task {
                        if lockEnabled {
                            if await AppLock.authenticate(reason: "turn on app lock") {
                                try? KeyVault().setAppLock(true, reason: "turn on app lock")
                                app.prefs.appLockEnabled = true
                            }
                        }
                        go(.connect)
                    }
                }
                .buttonStyle(PrimaryButtonStyle(tint: Palette.indigoBright))
                .keyboardShortcut(.defaultAction)
            }
        }
    }

    // MARK: Connect

    @ViewBuilder
    private var connect: some View {
        switch path {
        case nil:
            VStack(spacing: 18) {
                OnboardingTitle("Connect to a server", "Asiri runs on a small server you or a friend control. Nobody else is involved.")
                HStack(spacing: 14) {
                    PathCard(icon: "person.2.fill", title: "Join a friend", text: "A friend who already uses Asiri invites you in person.") {
                        withAnimation(Motion.standard) { path = .join }
                    }
                    PathCard(icon: "server.rack", title: "Set up a server", text: "You run the server and invite your friends.") {
                        withAnimation(Motion.standard) { path = .setUp }
                    }
                }
                Button("Back") { go(.security) }.buttonStyle(SecondaryButtonStyle())
            }
        case .join:
            JoinFlow(app: app, onBack: { withAnimation(Motion.standard) { path = nil } })
        case .setUp:
            SetUpServerFlow(app: app, onBack: { withAnimation(Motion.standard) { path = nil } })
        }
    }
}

/// New Mac: show QR-A, then scan or paste QR-B (§11.2).
struct JoinFlow: View {
    let app: AppModel
    let onBack: () -> Void
    @State private var offer = ""
    @State private var scanning = false
    @State private var pasted = ""
    @State private var working = false
    @State private var result: PairingResult?

    var body: some View {
        VStack(spacing: 16) {
            if let result {
                EmojiConfirmation(app: app, result: result, onDone: { app.finishOnboarding() })
            } else {
                OnboardingTitle("Show this to your friend", "They choose “Add a friend” in Asiri and scan it. Then you scan the code they show back.")
                HStack(alignment: .center, spacing: 22) {
                    if !offer.isEmpty { QRCodeView(code: offer, size: 210) }
                    VStack(spacing: 10) {
                        if scanning {
                            ScannerPanel { code in redeem(code) }
                                .frame(width: 220, height: 165)
                        } else {
                            Button { withAnimation(Motion.standard) { scanning = true } } label: {
                                Label("Scan their code", systemImage: "qrcode.viewfinder")
                            }
                            .buttonStyle(PrimaryButtonStyle(tint: Palette.indigoBright))
                        }
                        PasteCodeField(text: $pasted, placeholder: "…or paste their code", onSubmit: { redeem(pasted) })
                            .frame(width: 220)
                    }
                }
                if working {
                    ProgressView("Connecting securely…")
                        .controlSize(.small)
                        .foregroundStyle(.white)
                }
                Button("Back", action: onBack).buttonStyle(SecondaryButtonStyle())
            }
        }
        .onAppear { offer = (try? app.core.deviceOfferCode()) ?? "" }
    }

    private func redeem(_ code: String) {
        let code = code.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !code.isEmpty, !working else { return }
        working = true
        scanning = false
        Task {
            defer { working = false }
            do {
                let r = try await app.core.redeemInvitation(code)
                withAnimation(Motion.bouncy) { result = r }
                if r.emoji.isEmpty { app.finishOnboarding() }
            } catch {
                app.show(error)
            }
        }
    }
}

/// First admin: show the onion client key, then redeem the server's code (§11.1, §9.4).
struct SetUpServerFlow: View {
    let app: AppModel
    let onBack: () -> Void
    @State private var pasted = ""
    @State private var scanning = false
    @State private var working = false
    @State private var copied = false

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            OnboardingTitle("Set up your server", "Asiri's server only stores encrypted messages until your friends' Macs collect them.")
            StepCard(number: 1, title: "Install the server on your VPS") {
                Text("From the Asiri folder on this Mac:").font(.system(size: 12)).foregroundStyle(.white.opacity(0.7))
                CodeLine("scripts/deploy-vps.sh ubuntu@your-server your-key.pem")
            }
            StepCard(number: 2, title: "Make this Mac its first admin") {
                Text("Run this with the key below; it prints your server's code.").font(.system(size: 12)).foregroundStyle(.white.opacity(0.7))
                let key = (try? app.core.profile().onionClientKey) ?? ""
                CodeLine("scripts/deploy-vps.sh ubuntu@your-server your-key.pem --bootstrap '\(key)'")
                Button {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(key, forType: .string)
                    withAnimation(Motion.bouncy) { copied = true }
                } label: {
                    Label(copied ? "Copied" : "Copy this Mac's key", systemImage: copied ? "checkmark" : "doc.on.doc")
                }
                .buttonStyle(SecondaryButtonStyle())
            }
            StepCard(number: 3, title: "Enter the server's code") {
                HStack(spacing: 10) {
                    PasteCodeField(text: $pasted, placeholder: "Paste the code (ASIRI1:…)", onSubmit: { redeem(pasted) })
                    Button { scanning.toggle() } label: { Image(systemName: "qrcode.viewfinder") }
                        .buttonStyle(SecondaryButtonStyle())
                        .help("Scan it")
                }
                if scanning {
                    ScannerPanel { code in redeem(code) }.frame(height: 160)
                }
            }
            HStack {
                Button("Back", action: onBack).buttonStyle(SecondaryButtonStyle())
                Spacer()
                if working { ProgressView().controlSize(.small) }
                Button("Connect") { redeem(pasted) }
                    .buttonStyle(PrimaryButtonStyle(tint: Palette.indigoBright))
                    .disabled(pasted.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || working)
            }
        }
    }

    private func redeem(_ code: String) {
        let code = code.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !code.isEmpty, !working else { return }
        working = true
        scanning = false
        Task {
            defer { working = false }
            do {
                _ = try await app.core.redeemInvitation(code)
                app.show(.success, "You're the admin of your server")
                app.finishOnboarding()
            } catch {
                app.show(error)
            }
        }
    }
}

// MARK: - Pieces

struct OnboardingBackground: View {
    var body: some View {
        ZStack {
            Palette.night
            RadialGradient(colors: [Palette.indigo.opacity(0.55), .clear], center: .top, startRadius: 10, endRadius: 520)
            RadialGradient(colors: [Palette.violet.opacity(0.35), .clear], center: .bottomTrailing, startRadius: 10, endRadius: 480)
        }
        .ignoresSafeArea()
        .environment(\.colorScheme, .dark)
    }
}

struct OnboardingTitle: View {
    let title: String
    let subtitle: String

    init(_ title: String, _ subtitle: String) {
        self.title = title
        self.subtitle = subtitle
    }

    var body: some View {
        VStack(spacing: 8) {
            Text(title)
                .font(.system(size: 24, weight: .bold, design: .rounded))
                .foregroundStyle(.white)
                .multilineTextAlignment(.center)
            Text(subtitle)
                .font(.system(size: 13.5))
                .foregroundStyle(.white.opacity(0.7))
                .multilineTextAlignment(.center)
                .frame(maxWidth: 440)
        }
        .frame(maxWidth: .infinity)
    }
}

struct StepDots: View {
    let count: Int
    let current: Int

    var body: some View {
        HStack(spacing: 6) {
            ForEach(0 ..< count, id: \.self) { i in
                Capsule()
                    .fill(i == current ? Color.white : Color.white.opacity(0.25))
                    .frame(width: i == current ? 20 : 6, height: 6)
            }
        }
        .animation(Motion.bouncy, value: current)
    }
}

struct PathCard: View {
    let icon: String
    let title: String
    let text: String
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            VStack(alignment: .leading, spacing: 10) {
                Image(systemName: icon)
                    .font(.system(size: 24, weight: .semibold))
                    .foregroundStyle(Palette.amber)
                Text(title).font(.system(size: 15, weight: .semibold)).foregroundStyle(.white)
                Text(text).font(.system(size: 12.5)).foregroundStyle(.white.opacity(0.65)).multilineTextAlignment(.leading)
                Spacer(minLength: 0)
            }
            .padding(18)
            .frame(width: 220, height: 150, alignment: .topLeading)
            .background(RoundedRectangle(cornerRadius: 18, style: .continuous).fill(Color.white.opacity(hovering ? 0.14 : 0.08)))
            .overlay(RoundedRectangle(cornerRadius: 18, style: .continuous).stroke(Color.white.opacity(hovering ? 0.35 : 0.12)))
            .scaleEffect(hovering ? 1.03 : 1)
        }
        .buttonStyle(.plain)
        .onHover { h in withAnimation(Motion.snappy) { hovering = h } }
    }
}

struct StepCard<Content: View>: View {
    let number: Int
    let title: String
    @ViewBuilder let content: () -> Content

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            Text("\(number)")
                .font(.system(size: 13, weight: .bold, design: .rounded))
                .foregroundStyle(Palette.night)
                .frame(width: 24, height: 24)
                .background(Circle().fill(Palette.amber))
            VStack(alignment: .leading, spacing: 7) {
                Text(title).font(.system(size: 14, weight: .semibold)).foregroundStyle(.white)
                content()
            }
            Spacer(minLength: 0)
        }
        .padding(14)
        .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Color.white.opacity(0.07)))
    }
}

struct CodeLine: View {
    let text: String

    init(_ text: String) {
        self.text = text
    }

    var body: some View {
        Text(text)
            .font(.system(size: 11.5, design: .monospaced))
            .foregroundStyle(.white.opacity(0.9))
            .textSelection(.enabled)
            .padding(8)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: 8).fill(Color.black.opacity(0.35)))
    }
}

struct PasteCodeField: View {
    @Binding var text: String
    let placeholder: String
    let onSubmit: () -> Void

    var body: some View {
        HStack(spacing: 6) {
            TextField(placeholder, text: $text)
                .textFieldStyle(.plain)
                .font(.system(size: 12, design: .monospaced))
                .onSubmit(onSubmit)
            Button {
                if let s = NSPasteboard.general.string(forType: .string) {
                    text = s.trimmingCharacters(in: .whitespacesAndNewlines)
                    onSubmit()
                }
            } label: {
                Image(systemName: "doc.on.clipboard")
            }
            .buttonStyle(.plain)
            .help("Paste")
        }
        .padding(.horizontal, 10)
        .frame(height: 32)
        .background(RoundedRectangle(cornerRadius: 9, style: .continuous).fill(Palette.hairline))
    }
}

/// The camera, ready to read a pairing code.
struct ScannerPanel: View {
    let onCode: (String) -> Void
    @State private var state: QRScannerView.ScanState = .starting

    var body: some View {
        ZStack {
            CameraRepresentable(onCode: onCode, onState: { state = $0 })
            switch state {
            case .denied:
                ScannerMessage(icon: "video.slash.fill", text: "Allow the camera in System Settings → Privacy & Security, or paste the code instead.")
            case .noCamera:
                ScannerMessage(icon: "video.slash", text: "No camera found. Paste the code instead, or use Continuity Camera.")
            case .starting:
                ProgressView().controlSize(.small)
            case .scanning:
                ScanFrame()
            }
        }
        .clipShape(RoundedRectangle(cornerRadius: 18, style: .continuous))
    }
}

struct ScanFrame: View {
    var body: some View {
        RoundedRectangle(cornerRadius: 14, style: .continuous)
            .stroke(Palette.amber, style: StrokeStyle(lineWidth: 3, dash: [22, 12]))
            .padding(22)
            .phaseAnimator([0.6, 1.0]) { v, o in v.opacity(o) } animation: { _ in .easeInOut(duration: 0.9) }
            .allowsHitTesting(false)
    }
}

struct ScannerMessage: View {
    let icon: String
    let text: String

    var body: some View {
        VStack(spacing: 8) {
            Image(systemName: icon).font(.system(size: 22))
            Text(text).font(.system(size: 11.5)).multilineTextAlignment(.center)
        }
        .foregroundStyle(.white)
        .padding(14)
    }
}

struct CameraRepresentable: NSViewRepresentable {
    let onCode: (String) -> Void
    let onState: (QRScannerView.ScanState) -> Void

    func makeNSView(context: Context) -> QRScannerView {
        let view = QRScannerView(frame: .zero)
        view.onCode = onCode
        view.onState = onState
        return view
    }

    func updateNSView(_ view: QRScannerView, context: Context) {
        view.onCode = onCode
        view.onState = onState
    }

    static func dismantleNSView(_ view: QRScannerView, coordinator: ()) {
        view.stop()
    }
}
