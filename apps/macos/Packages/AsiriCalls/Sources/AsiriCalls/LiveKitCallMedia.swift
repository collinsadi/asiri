import AsiriDomain
import AsiriFeatures
import CoreGraphics
import Foundation
import LiveKit
import Observation
import SwiftUI

/// Call media through LiveKit (architecture §10.5).
///
/// The room is joined through the core's loopback signaling bridge, which reaches LiveKit over
/// Tor; audio and video go over UDP to the server. Every frame is encrypted with AES-GCM under
/// the call key the core derives from the chat's MLS group, so LiveKit only ever forwards
/// ciphertext. LiveKit knows people by random aliases; `displayName` turns an alias into a name
/// using the call signals the core has seen (inside MLS).
@MainActor
@Observable
public final class LiveKitCallMedia: CallMedia {
    public private(set) var participants: [CallParticipant] = []
    public private(set) var connection: CallConnection = .connecting
    public private(set) var isMicOn = true
    public private(set) var isCameraOn = false
    public private(set) var isSharingScreen = false
    public private(set) var isEncrypted = false
    public private(set) var notice: String?

    private let displayName: (String) -> String
    @ObservationIgnored private var room: Room?
    @ObservationIgnored private var keys: BaseKeyProvider?
    @ObservationIgnored private var events: RoomEvents?
    @ObservationIgnored private var cryptors: [String: E2EEState] = [:]
    @ObservationIgnored private var videos: [String: VideoTrack] = [:]
    @ObservationIgnored private var noticeTask: Task<Void, Never>?
    @ObservationIgnored private var ended = false

    /// - Parameter displayName: The name to show for a LiveKit identity (a random alias).
    public init(displayName: @escaping (String) -> String) {
        self.displayName = displayName
    }

    // MARK: CallMedia

    public func connect(_ session: CallSession) async {
        guard room == nil, !ended else { return }
        guard let key = Data(base64Encoded: session.keyBase64), !key.isEmpty else {
            connection = .ended("This call has no key, so it can't be encrypted.")
            return
        }
        let keys = BaseKeyProvider(options: KeyProviderOptions(sharedKey: true))
        let index = Int32(truncatingIfNeeded: session.keyIndex)
        keys.setKey(keyData: key, index: index)
        keys.setCurrentKeyIndex(index)

        let events = RoomEvents(owner: self)
        let room = Room(
            delegate: events,
            connectOptions: ConnectOptions(
                // Signaling travels over Tor; give a cold circuit time to build.
                socketConnectTimeoutInterval: 30,
                primaryTransportConnectTimeout: 30,
                publisherTransportConnectTimeout: 30
            ),
            roomOptions: RoomOptions(
                adaptiveStream: true,
                dynacast: true,
                encryptionOptions: EncryptionOptions(keyProvider: keys, encryptionType: .gcm)
            )
        )
        self.keys = keys
        self.events = events
        self.room = room
        connection = .connecting
        isCameraOn = session.isVideo
        sync()

        do {
            try await room.connect(url: session.url, token: session.token)
            guard !ended else {
                await room.disconnect()
                return
            }
            try await room.localParticipant.setMicrophone(enabled: true)
            if session.isVideo {
                try await room.localParticipant.setCamera(enabled: true)
            }
        } catch {
            guard !ended else { return }
            if room.connectionState == .connected {
                show("Couldn't start your microphone or camera. Check Asiri in System Settings › Privacy & Security.")
            } else {
                connection = .ended("Couldn't join the call. \(Self.describe(error))")
            }
        }
        sync()
    }

    public func setMic(_ on: Bool) async {
        guard let room else { return }
        do {
            try await room.localParticipant.setMicrophone(enabled: on)
        } catch {
            show("Couldn't turn the microphone \(on ? "on" : "off"). Check Asiri in System Settings › Privacy & Security › Microphone.")
        }
        sync()
    }

    public func setCamera(_ on: Bool) async {
        guard let room else { return }
        do {
            try await room.localParticipant.setCamera(enabled: on)
        } catch {
            show("Couldn't turn the camera \(on ? "on" : "off"). Check Asiri in System Settings › Privacy & Security › Camera.")
        }
        sync()
    }

    public func setScreenShare(_ on: Bool) async {
        guard let room else { return }
        if on, !CGPreflightScreenCaptureAccess() {
            CGRequestScreenCaptureAccess()
            show("Allow Asiri in System Settings › Privacy & Security › Screen & System Audio Recording, then share again.")
            return
        }
        do {
            try await room.localParticipant.setScreenShare(enabled: on)
        } catch {
            show("Couldn't \(on ? "share" : "stop sharing") your screen. \(Self.describe(error))")
        }
        sync()
    }

    /// The chat's group changed mid-call: switch every frame to the next key (§7.8).
    public func updateKey(base64: String, index: UInt32) {
        guard let keys, let key = Data(base64Encoded: base64), !key.isEmpty else { return }
        let slot = Int32(truncatingIfNeeded: index)
        keys.setKey(keyData: key, index: slot)
        keys.setCurrentKeyIndex(slot)
    }

    public func disconnect() async {
        ended = true
        noticeTask?.cancel()
        guard let room else {
            connection = .ended(nil)
            return
        }
        self.room = nil
        await room.disconnect()
        connection = .ended(nil)
        participants = []
        videos = [:]
        cryptors = [:]
        keys = nil
        events = nil
        isEncrypted = false
    }

    public func video(for participant: CallParticipant) -> AnyView? {
        guard participant.hasVideo, let track = videos[participant.id] else { return nil }
        return AnyView(SwiftUIVideoView(track, layoutMode: participant.isScreenShare ? .fit : .fill))
    }

    // MARK: Room events (on the main actor)

    fileprivate func connectionChanged(_ state: ConnectionState) {
        guard !ended else { return }
        switch state {
        case .connecting: connection = .connecting
        case .connected: connection = .connected
        case .reconnecting: connection = .reconnecting
        default: break // didDisconnect says why
        }
        sync()
    }

    fileprivate func disconnected(_ reason: String?) {
        guard !ended else { return }
        connection = .ended(reason.map { "The call dropped. \($0)" } ?? "Call ended")
        sync()
    }

    fileprivate func cryptorChanged(_ track: String, _ state: E2EEState) {
        cryptors[track] = state
        sync()
    }

    fileprivate func forget(_ tracks: [String]) {
        for track in tracks { cryptors[track] = nil }
        sync()
    }

    /// Rebuilds the tiles, the controls' state and the "Encrypted" badge from the room.
    fileprivate func sync() {
        guard let room else { return }
        var people: [CallParticipant] = []
        var tracks: [String: VideoTrack] = [:]
        var present: [String: E2EEState] = [:]

        func add(_ p: Participant, id: String, name: String, isLocal: Bool) {
            var keyProblem = false
            for publication in p.trackPublications.values {
                let key = publication.sid.stringValue
                guard let state = cryptors[key] else { continue }
                present[key] = state
                if Self.isFailure(state) { keyProblem = true }
            }
            let camera = p.videoTracks.first { $0.source == .camera && !$0.isMuted }?.track as? VideoTrack
            let screen = p.videoTracks.first { $0.source == .screenShareVideo && !$0.isMuted }?.track as? VideoTrack
            if let camera { tracks[id] = camera }
            people.append(CallParticipant(
                id: id, name: name, isLocal: isLocal, isSpeaking: p.isSpeaking, isMuted: !p.isMicrophoneEnabled(),
                hasVideo: camera != nil, hasKeyProblem: keyProblem && !isLocal
            ))
            if let screen {
                let shareID = id + "/screen"
                tracks[shareID] = screen
                people.append(CallParticipant(
                    id: shareID, name: isLocal ? "Your screen" : "\(name)'s screen", isLocal: isLocal,
                    hasVideo: true, isScreenShare: true, hasKeyProblem: keyProblem && !isLocal
                ))
            }
        }

        let local = room.localParticipant
        add(local, id: "local", name: "You", isLocal: true)
        let remotes = room.remoteParticipants.values.sorted {
            ($0.joinedAt ?? .distantPast, $0.identity?.stringValue ?? "") < ($1.joinedAt ?? .distantPast, $1.identity?.stringValue ?? "")
        }
        for remote in remotes {
            let alias = remote.identity?.stringValue ?? ""
            add(remote, id: "p:" + alias, name: displayName(alias), isLocal: false)
        }

        participants = people
        videos = tracks
        isMicOn = local.isMicrophoneEnabled()
        isCameraOn = local.isCameraEnabled()
        isSharingScreen = local.isScreenShareEnabled()
        // "Encrypted" only once a frame cryptor reports in and none is failing (§10.5).
        isEncrypted = room.connectionState == .connected && !present.isEmpty && !present.values.contains(where: Self.isFailure)
    }

    // MARK: Helpers

    private static func isFailure(_ state: E2EEState) -> Bool {
        !(state == .ok || state == .key_ratcheted || state == .new)
    }

    private static func describe(_ error: Error) -> String {
        let text = (error as NSError).localizedDescription
        return text.isEmpty ? "Check your connection and try again." : text
    }

    private func show(_ text: String) {
        notice = text
        noticeTask?.cancel()
        noticeTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(6))
            guard !Task.isCancelled else { return }
            self?.notice = nil
        }
    }
}

// MARK: - Delegate

/// LiveKit calls its delegates on its own queues; everything is handed to the main actor.
private final class RoomEvents: NSObject, RoomDelegate, @unchecked Sendable {
    weak var owner: LiveKitCallMedia?

    init(owner: LiveKitCallMedia) {
        self.owner = owner
    }

    private func update(_ body: @escaping @MainActor (LiveKitCallMedia) -> Void) {
        Task { @MainActor [weak self] in
            guard let owner = self?.owner else { return }
            body(owner)
        }
    }

    func room(_ room: Room, didUpdateConnectionState connectionState: ConnectionState, from oldConnectionState: ConnectionState) {
        update { $0.connectionChanged(connectionState) }
    }

    func room(_ room: Room, didDisconnectWithError error: LiveKitError?) {
        let reason = error?.localizedDescription
        update { $0.disconnected(reason) }
    }

    func room(_ room: Room, participantDidConnect participant: RemoteParticipant) {
        update { $0.sync() }
    }

    func room(_ room: Room, participantDidDisconnect participant: RemoteParticipant) {
        let gone = participant.trackPublications.values.map(\.sid.stringValue)
        update { $0.forget(gone) }
    }

    func room(_ room: Room, didUpdateSpeakingParticipants participants: [Participant]) {
        update { $0.sync() }
    }

    func room(_ room: Room, participant: RemoteParticipant, didSubscribeTrack publication: RemoteTrackPublication) {
        update { $0.sync() }
    }

    func room(_ room: Room, participant: RemoteParticipant, didUnsubscribeTrack publication: RemoteTrackPublication) {
        update { $0.sync() }
    }

    func room(_ room: Room, participant: LocalParticipant, didPublishTrack publication: LocalTrackPublication) {
        update { $0.sync() }
    }

    func room(_ room: Room, participant: LocalParticipant, didUnpublishTrack publication: LocalTrackPublication) {
        update { $0.sync() }
    }

    func room(_ room: Room, participant: Participant, trackPublication: TrackPublication, didUpdateIsMuted isMuted: Bool) {
        update { $0.sync() }
    }

    func room(_ room: Room, trackPublication: TrackPublication, didUpdateE2EEState state: E2EEState) {
        let track = trackPublication.sid.stringValue
        update { $0.cryptorChanged(track, state) }
    }
}
