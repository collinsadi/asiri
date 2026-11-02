import AsiriDesign
import AsiriPlatform
import SwiftUI

/// Where the app is before the encrypted database is open.
public enum StartupState: Equatable, Sendable {
    /// Unwrapping the database key and opening the store.
    case opening
    /// The user cancelled Touch ID or the password prompt at launch.
    case locked
    /// The store couldn't be opened; the text says why, in plain words.
    case failed(String)
}

/// The main window until the database is open: the glowing mark, the locked screen, or what
/// went wrong with a way forward.
public struct StartupView: View {
    let state: StartupState
    let retry: () -> Void
    let startOver: () -> Void
    @State private var confirmStartOver = false

    public init(state: StartupState, retry: @escaping () -> Void, startOver: @escaping () -> Void) {
        self.state = state
        self.retry = retry
        self.startOver = startOver
    }

    public var body: some View {
        ZStack {
            switch state {
            case .opening:
                LaunchView()
                    .transition(.opacity)
            case .locked:
                locked
                    .transition(.opacity)
            case let .failed(message):
                failed(message)
                    .transition(.opacity)
            }
        }
        .animation(Motion.standard, value: state)
        .frame(minWidth: 780, minHeight: 520)
    }

    private var locked: some View {
        ZStack {
            OnboardingBackground()
            VStack(spacing: 18) {
                AsiriMark(size: 84)
                    .shadow(color: Palette.indigoBright.opacity(0.5), radius: 24)
                Text("Asiri is locked")
                    .font(.system(size: 22, weight: .bold, design: .rounded))
                    .foregroundStyle(.white)
                Text("Unlock to read your chats.")
                    .font(.system(size: 13))
                    .foregroundStyle(.white.opacity(0.7))
                Button(action: retry) {
                    Label("Unlock", systemImage: AppLockGlyph.name)
                }
                .buttonStyle(PrimaryButtonStyle(tint: Palette.indigoBright))
                .keyboardShortcut(.defaultAction)
            }
        }
        .environment(\.colorScheme, .dark)
    }

    private func failed(_ message: String) -> some View {
        ZStack {
            OnboardingBackground()
            VStack(spacing: 16) {
                Image(systemName: "exclamationmark.lock.fill")
                    .font(.system(size: 44))
                    .foregroundStyle(Palette.amber)
                    .symbolEffect(.pulse)
                Text("Asiri couldn't open your chats")
                    .font(.system(size: 20, weight: .bold, design: .rounded))
                    .foregroundStyle(.white)
                Text(message)
                    .font(.system(size: 13))
                    .foregroundStyle(.white.opacity(0.75))
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 420)
                HStack(spacing: 12) {
                    Button("Try Again", action: retry)
                        .buttonStyle(PrimaryButtonStyle(tint: Palette.indigoBright))
                        .keyboardShortcut(.defaultAction)
                    Button("Start Over…") { confirmStartOver = true }
                        .buttonStyle(SecondaryButtonStyle())
                }
                .padding(.top, 6)
            }
            .padding(32)
        }
        .environment(\.colorScheme, .dark)
        .confirmationDialog("Erase Asiri on this Mac?", isPresented: $confirmStartOver) {
            Button("Erase and Start Over", role: .destructive, action: startOver)
        } message: {
            Text("Your chats on this Mac are deleted. Your friends keep theirs, and you can pair this Mac again.")
        }
    }
}

/// The unlock symbol: Touch ID where the Mac has it, a lock otherwise.
enum AppLockGlyph {
    static var name: String {
        AppLock.hasBiometrics ? "touchid" : "lock.open.fill"
    }
}
