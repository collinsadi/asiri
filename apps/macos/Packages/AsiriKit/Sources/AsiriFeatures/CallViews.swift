import AppKit
import AsiriDesign
import AsiriDomain
import Observation
import SwiftUI

// MARK: - Media abstraction

/// Someone in a call.
public struct CallParticipant: Identifiable, Hashable, Sendable {
    public var id: String
    public var name: String
    public var isLocal: Bool
    public var isSpeaking: Bool
    public var isMuted: Bool
    public var hasVideo: Bool
    public var isScreenShare: Bool
    /// Their frames couldn't be decrypted (a missing key, §10.5).
    public var hasKeyProblem: Bool

    public init(id: String, name: String, isLocal: Bool, isSpeaking: Bool = false, isMuted: Bool = false, hasVideo: Bool = false,
                isScreenShare: Bool = false, hasKeyProblem: Bool = false) {
        self.id = id
        self.name = name
        self.isLocal = isLocal
        self.isSpeaking = isSpeaking
        self.isMuted = isMuted
        self.hasVideo = hasVideo
        self.isScreenShare = isScreenShare
        self.hasKeyProblem = hasKeyProblem
    }
}

public enum CallConnection: Sendable, Equatable {
    case connecting
    case connected
    case reconnecting
    case ended(String?)
}

/// The media side of a call: LiveKit in the app (with the shared end-to-end key from the core),
/// a stand-in in previews and tests.
@MainActor
public protocol CallMedia: AnyObject {
    var participants: [CallParticipant] { get }
    var connection: CallConnection { get }
    var isMicOn: Bool { get }
    var isCameraOn: Bool { get }
    var isSharingScreen: Bool { get }
    /// True only once the room reports encryption as active (§10.5).
    var isEncrypted: Bool { get }

    func connect(_ session: CallSession) async
    func setMic(_ on: Bool) async
    func setCamera(_ on: Bool) async
    func setScreenShare(_ on: Bool) async
    func updateKey(base64: String, index: UInt32)
    func disconnect() async
    /// The video for a participant, when they have one.
    func video(for participant: CallParticipant) -> AnyView?
}

/// A stand-in that behaves like a call without any media (previews, snapshots, builds without
/// a call stack).
@MainActor
@Observable
public final class PreviewCallMedia: CallMedia {
    public var participants: [CallParticipant] = []
    public var connection: CallConnection = .connecting
    public var isMicOn = true
    public var isCameraOn = false
    public var isSharingScreen = false
    public var isEncrypted = false
    private let names: [String]

    public init(names: [String] = ["Ada Okafor"]) {
        self.names = names
    }

    public func connect(_ session: CallSession) async {
        isCameraOn = session.isVideo
        participants = [CallParticipant(id: "me", name: "You", isLocal: true, hasVideo: session.isVideo)]
            + names.enumerated().map { CallParticipant(id: "p\($0.offset)", name: $0.element, isLocal: false, isSpeaking: $0.offset == 0) }
        try? await Task.sleep(for: .milliseconds(600))
        connection = .connected
        isEncrypted = true
    }

    public func setMic(_ on: Bool) async {
        isMicOn = on
        if let i = participants.firstIndex(where: \.isLocal) { participants[i].isMuted = !on }
    }

    public func setCamera(_ on: Bool) async {
        isCameraOn = on
        if let i = participants.firstIndex(where: \.isLocal) { participants[i].hasVideo = on }
    }

    public func setScreenShare(_ on: Bool) async { isSharingScreen = on }
    public func updateKey(base64: String, index: UInt32) {}
    public func disconnect() async { connection = .ended(nil) }
    public func video(for participant: CallParticipant) -> AnyView? { nil }
}

// MARK: - Incoming call

/// Slides down over the main window while a call rings (with a notification as well).
struct IncomingCallBanner: View {
    let app: AppModel
    let call: IncomingCall

    var body: some View {
        let chat = app.chats.first { $0.id == call.chatID }
        HStack(spacing: 14) {
            ZStack {
                ForEach(0 ..< 2, id: \.self) { i in
                    Circle()
                        .stroke(Palette.success.opacity(0.5), lineWidth: 2)
                        .frame(width: 46, height: 46)
                        .phaseAnimator([false, true]) { ring, grow in
                            ring.scaleEffect(grow ? 1.7 : 1).opacity(grow ? 0 : 1)
                        } animation: { _ in .easeOut(duration: 1.4).delay(Double(i) * 0.7) }
                }
                Avatar(id: call.caller, name: app.name(of: call.caller), image: app.contact(call.caller)?.avatar, size: 46)
            }
            VStack(alignment: .leading, spacing: 2) {
                Text(app.name(of: call.caller)).font(.system(size: 14, weight: .semibold))
                Text(call.isVideo ? "Incoming video call" : "Incoming call")
                    .font(.system(size: 12))
                    .foregroundStyle(.secondary)
                if let chat, chat.kind == .group {
                    Text("in \(chat.title)").font(.system(size: 11)).foregroundStyle(.secondary)
                }
            }
            Spacer(minLength: 20)
            Button { app.decline(call) } label: {
                Image(systemName: "phone.down.fill")
                    .font(.system(size: 15, weight: .bold))
                    .foregroundStyle(.white)
                    .frame(width: 42, height: 42)
                    .background(Circle().fill(Palette.danger))
            }
            .buttonStyle(PressableStyle())
            .help("Decline")
            Button { Task { await app.answer(call) } } label: {
                Image(systemName: call.isVideo ? "video.fill" : "phone.fill")
                    .font(.system(size: 15, weight: .bold))
                    .foregroundStyle(.white)
                    .frame(width: 42, height: 42)
                    .background(Circle().fill(Palette.success))
                    .repeatingBounce(speed: 0.6)
            }
            .buttonStyle(PressableStyle())
            .keyboardShortcut(.defaultAction)
            .help("Answer")
        }
        .padding(14)
        .frame(width: 420)
        .background(RoundedRectangle(cornerRadius: 20, style: .continuous).fill(.regularMaterial))
        .overlay(RoundedRectangle(cornerRadius: 20, style: .continuous).stroke(Palette.hairline))
        .shadow(color: .black.opacity(0.2), radius: 20, y: 8)
    }
}

// MARK: - Call window

/// The call window (§13.3): tiles, controls, the "Encrypted" lock and the four-emoji check.
public struct CallView: View {
    let app: AppModel
    let session: CallSession
    let media: any CallMedia
    @State private var showEmoji = false
    @State private var started = Date()

    public init(app: AppModel, session: CallSession, media: any CallMedia) {
        self.app = app
        self.session = session
        self.media = media
    }

    public var body: some View {
        ZStack {
            Palette.night.ignoresSafeArea()
            RadialGradient(colors: [Palette.indigo.opacity(0.35), .clear], center: .top, startRadius: 10, endRadius: 600).ignoresSafeArea()
            VStack(spacing: 0) {
                topBar
                tiles
                    .padding(14)
                controls
                    .padding(.bottom, 20)
            }
        }
        .environment(\.colorScheme, .dark)
        .frame(minWidth: 420, minHeight: 360)
        .task {
            started = Date()
            await media.connect(session)
        }
        .onChange(of: app.callKeys[session.callID]?.1) {
            if let rotated = app.callKeys[session.callID] { media.updateKey(base64: rotated.0, index: rotated.1) }
        }
        .onChange(of: app.activeCall) {
            if app.activeCall == nil { Task { await media.disconnect() } }
        }
    }

    private var title: String {
        app.chats.first { $0.id == session.chatID }?.title ?? "Call"
    }

    private var topBar: some View {
        HStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(.system(size: 15, weight: .semibold)).foregroundStyle(.white)
                HStack(spacing: 6) {
                    switch media.connection {
                    case .connecting: Text("Connecting securely…")
                    case .reconnecting: Text("Reconnecting…")
                    case let .ended(why): Text(why ?? "Call ended")
                    case .connected:
                        TimelineView(.periodic(from: started, by: 1)) { ctx in
                            Text(Formatting.duration(ms: Int(ctx.date.timeIntervalSince(started) * 1000)))
                                .monospacedDigit()
                        }
                    }
                }
                .font(.system(size: 12))
                .foregroundStyle(.white.opacity(0.65))
            }
            Spacer()
            Button { withAnimation(Motion.bouncy) { showEmoji.toggle() } } label: {
                HStack(spacing: 5) {
                    Image(systemName: media.isEncrypted ? "lock.fill" : "lock.open")
                    Text(media.isEncrypted ? "Encrypted" : "Securing…")
                    if showEmoji {
                        Text(session.emoji.map(\.glyph).joined(separator: " "))
                            .transition(.scale.combined(with: .opacity))
                    }
                }
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(media.isEncrypted ? Palette.amber : Color.white.opacity(0.6))
                .padding(.horizontal, 10)
                .padding(.vertical, 5)
                .background(Capsule().fill(Color.white.opacity(0.1)))
            }
            .buttonStyle(.plain)
            .help("Everyone in the call sees the same four emoji")
        }
        .padding(.horizontal, 18)
        .padding(.top, 14)
    }

    private var tiles: some View {
        let people = media.participants
        let columns = people.count <= 1 ? 1 : (people.count <= 4 ? 2 : 3)
        return LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 10), count: columns), spacing: 10) {
            ForEach(people) { p in
                ParticipantTile(participant: p, video: media.video(for: p), app: app, session: session)
                    .aspectRatio(16 / 10, contentMode: .fit)
                    .transition(.scale(scale: 0.9).combined(with: .opacity))
            }
        }
        .animation(Motion.standard, value: people)
        .frame(maxHeight: .infinity)
    }

    private var controls: some View {
        HStack(spacing: 18) {
            CallControl(icon: media.isMicOn ? "mic.fill" : "mic.slash.fill", label: media.isMicOn ? "Mute" : "Unmute", active: !media.isMicOn) {
                Task { await media.setMic(!media.isMicOn) }
            }
            .keyboardShortcut("a", modifiers: [.command, .shift])
            CallControl(icon: media.isCameraOn ? "video.fill" : "video.slash.fill", label: "Camera", active: !media.isCameraOn) {
                Task { await media.setCamera(!media.isCameraOn) }
            }
            .keyboardShortcut("v", modifiers: [.command, .shift])
            CallControl(icon: "rectangle.on.rectangle", label: media.isSharingScreen ? "Stop sharing" : "Share screen", active: media.isSharingScreen) {
                Task { await media.setScreenShare(!media.isSharingScreen) }
            }
            .keyboardShortcut("s", modifiers: [.command, .shift])
            Button {
                Task { await media.disconnect() }
                app.hangUp()
            } label: {
                Image(systemName: "phone.down.fill")
                    .font(.system(size: 20, weight: .bold))
                    .foregroundStyle(.white)
                    .frame(width: 60, height: 60)
                    .background(Circle().fill(Palette.danger))
                    .shadow(color: Palette.danger.opacity(0.5), radius: 12, y: 4)
            }
            .buttonStyle(PressableStyle())
            .help("Hang up")
        }
    }
}

struct ParticipantTile: View {
    let participant: CallParticipant
    let video: AnyView?
    let app: AppModel
    let session: CallSession

    var body: some View {
        ZStack(alignment: .bottomLeading) {
            RoundedRectangle(cornerRadius: 18, style: .continuous)
                .fill(LinearGradient(colors: [Color(hex: 0x2A2750), Color(hex: 0x1B1938)], startPoint: .top, endPoint: .bottom))
            if let video {
                video.clipShape(RoundedRectangle(cornerRadius: 18, style: .continuous))
            } else {
                Avatar(id: Data(participant.name.utf8), name: participant.name, size: 72)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .scaleEffect(participant.isSpeaking ? 1.06 : 1)
                    .animation(Motion.bouncy, value: participant.isSpeaking)
            }
            HStack(spacing: 5) {
                if participant.isMuted { Image(systemName: "mic.slash.fill").foregroundStyle(Palette.danger) }
                if participant.hasKeyProblem { Image(systemName: "exclamationmark.lock.fill").foregroundStyle(Palette.amber) }
                Text(participant.isLocal ? "You" : participant.name)
            }
            .font(.system(size: 12, weight: .semibold))
            .foregroundStyle(.white)
            .padding(.horizontal, 9)
            .padding(.vertical, 4)
            .background(Capsule().fill(Color.black.opacity(0.45)))
            .padding(10)
        }
        .overlay(
            RoundedRectangle(cornerRadius: 18, style: .continuous)
                .stroke(participant.isSpeaking ? Palette.success : Color.clear, lineWidth: 3)
                .animation(Motion.standard, value: participant.isSpeaking)
        )
    }
}

struct CallControl: View {
    let icon: String
    let label: String
    let active: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            VStack(spacing: 6) {
                Image(systemName: icon)
                    .font(.system(size: 18, weight: .semibold))
                    .foregroundStyle(active ? Palette.night : Color.white)
                    .contentTransition(.symbolEffect(.replace))
                    .frame(width: 52, height: 52)
                    .background(Circle().fill(active ? Color.white : Color.white.opacity(0.14)))
                Text(label).font(.system(size: 11)).foregroundStyle(.white.opacity(0.75))
            }
        }
        .buttonStyle(PressableStyle())
        .animation(Motion.snappy, value: active)
    }
}
