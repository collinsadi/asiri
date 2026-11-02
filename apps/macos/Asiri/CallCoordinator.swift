import AsiriCalls
import AsiriDomain
import AsiriFeatures
import Observation

/// Owns the media for the call on screen: LiveKit for real calls, a stand-in in the demo.
@MainActor
@Observable
final class CallCoordinator {
    private(set) var callID: CallID?
    private(set) var media: (any CallMedia)?

    func begin(_ session: CallSession, app: AppModel, demo: Bool) {
        guard callID != session.callID else { return }
        end()
        callID = session.callID
        if demo {
            let others = ((try? app.core.members(of: session.chatID)) ?? []).filter { !$0.isMe }.map(\.contact.displayName)
            media = PreviewCallMedia(names: others.isEmpty ? ["Ada Okafor"] : others)
        } else {
            let call = session.callID
            media = LiveKitCallMedia(displayName: { [weak app] alias in
                app?.participantName(alias: alias, in: call) ?? "Someone"
            })
        }
    }

    func end() {
        callID = nil
        guard let media else { return }
        self.media = nil
        Task { await media.disconnect() }
    }
}
