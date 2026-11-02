import AsiriDesign
import AsiriDomain
import SwiftUI

/// "Update Asiri" at the bottom of the chat list (§16.2).
struct UpdateBanner: View {
    let app: AppModel
    let update: UpdateInfo

    var body: some View {
        Button {
            Task { await app.installUpdate() }
        } label: {
            HStack(spacing: 8) {
                if app.installingUpdate {
                    ProgressView()
                        .controlSize(.small)
                        .tint(.white)
                } else {
                    Image(systemName: "arrow.down.circle.fill")
                        .font(.system(size: 15, weight: .semibold))
                }
                Text(app.installingUpdate ? "Downloading securely…" : "Update Asiri to \(update.version)")
                    .font(.system(size: 13, weight: .semibold))
                Spacer(minLength: 0)
            }
            .foregroundStyle(.white)
            .padding(.horizontal, 14)
            .frame(height: 36)
            .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Palette.accent))
            .contentShape(Rectangle())
        }
        .buttonStyle(PressableStyle())
        .disabled(app.installingUpdate)
        .help(update.notes.isEmpty ? "A new version of Asiri is ready" : update.notes)
        .padding(.horizontal, 10)
        .padding(.bottom, 8)
        .transition(.move(edge: .bottom).combined(with: .opacity))
    }
}

/// Builds older than the server's minimum stop here until they update (§16.2).
struct RequiredUpdateView: View {
    let app: AppModel
    let update: UpdateInfo

    var body: some View {
        ZStack {
            OnboardingBackground()
            VStack(spacing: 16) {
                AsiriMark(size: 80)
                    .shadow(color: Palette.indigoBright.opacity(0.5), radius: 24)
                Text("Please update Asiri")
                    .font(.system(size: 22, weight: .bold, design: .rounded))
                    .foregroundStyle(.white)
                Text("This version is too old to keep your chats safe. Asiri \(update.version) is ready, checked against the release key built into this app.")
                    .font(.system(size: 13))
                    .foregroundStyle(.white.opacity(0.75))
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 440)
                if !update.notes.isEmpty {
                    Text(update.notes)
                        .font(.system(size: 12.5))
                        .foregroundStyle(.white.opacity(0.6))
                        .multilineTextAlignment(.center)
                        .frame(maxWidth: 440)
                }
                Button {
                    Task { await app.installUpdate() }
                } label: {
                    HStack(spacing: 8) {
                        if app.installingUpdate { ProgressView().controlSize(.small).tint(.white) }
                        Text(app.installingUpdate ? "Downloading securely…" : "Download and Install")
                    }
                }
                .buttonStyle(PrimaryButtonStyle(tint: Palette.indigoBright))
                .keyboardShortcut(.defaultAction)
                .disabled(app.installingUpdate)
                .padding(.top, 4)
            }
            .padding(32)
        }
        .environment(\.colorScheme, .dark)
    }
}
