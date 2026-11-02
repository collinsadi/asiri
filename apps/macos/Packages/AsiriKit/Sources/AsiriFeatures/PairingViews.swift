import AppKit
import AsiriDesign
import AsiriDomain
import SwiftUI

/// "Add a friend": the inviter's side of in-person pairing (architecture §11.2).
struct AddFriendSheet: View {
    let app: AppModel
    @Environment(\.dismiss) private var dismiss
    @State private var mode = Mode.newToAsiri
    @State private var pasted = ""
    @State private var working = false
    @State private var invitation: PairingResult?
    @State private var confirming: PairingResult?

    enum Mode: String, CaseIterable, Identifiable {
        case newToAsiri = "New to Asiri"
        case alreadyUses = "Already uses Asiri"
        var id: String { rawValue }
    }

    var body: some View {
        VStack(spacing: 18) {
            HStack {
                Text("Add a friend").font(.system(size: 18, weight: .bold))
                Spacer()
                Button("Close") { dismiss() }.keyboardShortcut(.cancelAction)
            }
            if let confirming {
                EmojiConfirmation(app: app, result: confirming) {
                    if let chat = confirming.chatID ?? app.pairingCompleted?.chat { app.select(chat) }
                    dismiss()
                }
                .transition(.scale(scale: 0.95).combined(with: .opacity))
            } else if let invitation {
                waiting(invitation)
                    .transition(.move(edge: .trailing).combined(with: .opacity))
            } else {
                Picker("", selection: $mode) {
                    ForEach(Mode.allCases) { Text($0.rawValue).tag($0) }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .frame(maxWidth: 360)
                switch mode {
                case .newToAsiri: scanOffer
                case .alreadyUses: exchangeCards
                }
            }
        }
        .padding(24)
        .frame(width: 560, height: 560)
        .animation(Motion.standard, value: invitation)
        .animation(Motion.standard, value: confirming)
        .onChange(of: app.pairingCompleted?.user) {
            guard let invitation, app.pairingCompleted?.user == invitation.peerID else { return }
            withAnimation(Motion.bouncy) { confirming = invitation }
        }
    }

    private var scanOffer: some View {
        VStack(spacing: 14) {
            Text("On your friend's Mac, Asiri shows a code after “Join a friend”. Scan it, or paste it if they sent it to you some other way you trust.")
                .font(.system(size: 13))
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 420)
            ScannerPanel { code in invite(code) }
                .frame(width: 360, height: 250)
            PasteCodeField(text: $pasted, placeholder: "Paste their code (ASIRI1:…)", onSubmit: { invite(pasted) })
                .frame(width: 360)
            if working { ProgressView().controlSize(.small) }
        }
    }

    private func waiting(_ result: PairingResult) -> some View {
        VStack(spacing: 14) {
            Text("Now \(result.peerName.isEmpty ? "your friend" : result.peerName) scans this")
                .font(.system(size: 15, weight: .semibold))
            if let code = result.replyCode {
                QRCodeView(code: code, size: 230)
            }
            EmojiSequence(result.emoji, size: 26)
            HStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text("Waiting for \(result.peerName.isEmpty ? "them" : result.peerName)…")
                    .font(.system(size: 12.5))
                    .foregroundStyle(.secondary)
            }
            Button("Copy code") {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(result.replyCode ?? "", forType: .string)
                app.show(.info, "Copied. Only send it through a channel you trust.")
            }
            .buttonStyle(SecondaryButtonStyle())
        }
    }

    @State private var card = ""

    private var exchangeCards: some View {
        HStack(alignment: .top, spacing: 22) {
            VStack(spacing: 8) {
                Text("Your code").font(.system(size: 13, weight: .semibold))
                if !card.isEmpty { QRCodeView(code: card, size: 200) }
            }
            VStack(spacing: 10) {
                Text("Their code").font(.system(size: 13, weight: .semibold))
                ScannerPanel { code in addContact(code) }
                    .frame(width: 220, height: 165)
                PasteCodeField(text: $pasted, placeholder: "…or paste it", onSubmit: { addContact(pasted) })
                    .frame(width: 220)
                if working { ProgressView().controlSize(.small) }
            }
        }
        .onAppear { card = (try? app.core.contactCode()) ?? "" }
    }

    private func invite(_ code: String) {
        let code = code.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !code.isEmpty, !working else { return }
        working = true
        Task {
            defer { working = false }
            do {
                let result = try await app.core.inviteDevice(offer: code)
                withAnimation(Motion.standard) { invitation = result }
            } catch {
                app.show(error)
            }
        }
    }

    private func addContact(_ code: String) {
        let code = code.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !code.isEmpty, !working else { return }
        working = true
        Task {
            defer { working = false }
            do {
                let result = try await app.core.addContact(card: code)
                withAnimation(Motion.bouncy) { confirming = result }
            } catch {
                app.show(error)
            }
        }
    }
}

/// "Do these emoji match?" — the moment that makes Asiri trustworthy.
struct EmojiConfirmation: View {
    let app: AppModel
    let result: PairingResult
    let onDone: () -> Void
    @State private var mismatch = false

    var body: some View {
        VStack(spacing: 18) {
            Image(systemName: mismatch ? "exclamationmark.triangle.fill" : "checkmark.shield.fill")
                .font(.system(size: 42))
                .foregroundStyle(mismatch ? Palette.danger : Palette.amber)
                .symbolEffect(.bounce, value: mismatch)
            Text(mismatch ? "Don't use this chat yet" : "Compare with \(result.peerName.isEmpty ? "their" : result.peerName + "'s") screen")
                .font(.system(size: 18, weight: .bold))
            Text(mismatch
                ? "The codes didn't match, so someone may be in the middle. Delete the chat and pair again, face to face."
                : "Both screens should show the same six emoji, in the same order.")
                .font(.system(size: 13))
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 400)
            if !mismatch {
                EmojiSequence(result.emoji, size: 40)
                    .padding(.vertical, 6)
                HStack(spacing: 12) {
                    Button("They don't match") { withAnimation(Motion.bouncy) { mismatch = true } }
                        .buttonStyle(SecondaryButtonStyle())
                    Button("They match") {
                        app.perform { try app.core.confirmVerification(of: result.peerID, inPerson: true) }
                        app.show(.success, "\(result.peerName.isEmpty ? "Your friend" : result.peerName) is verified")
                        onDone()
                    }
                    .buttonStyle(PrimaryButtonStyle())
                    .keyboardShortcut(.defaultAction)
                }
            } else {
                Button("Close", action: onDone).buttonStyle(SecondaryButtonStyle())
            }
        }
        .padding(.vertical, 10)
    }
}

/// Comparing safety numbers remotely (§7.6).
struct VerifySheet: View {
    let app: AppModel
    let user: UserID
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        let contact = app.contact(user)
        let number = (try? app.core.safetyNumber(for: user)) ?? ""
        let emoji = (try? app.core.pairingEmoji(for: user)) ?? []
        VStack(spacing: 16) {
            Avatar(id: user, name: contact?.displayName ?? "", image: contact?.avatar, size: 64)
            Text("Verify \(contact?.displayName ?? "your friend")")
                .font(.system(size: 18, weight: .bold))
            Text("Compare these with \(contact?.displayName ?? "them") in person or on a call you trust. If they match, nobody is in the middle.")
                .font(.system(size: 13))
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 380)
            EmojiSequence(emoji, size: 30)
            Text(number)
                .font(Typo.mono)
                .multilineTextAlignment(.center)
                .textSelection(.enabled)
                .padding(12)
                .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Palette.hairline))
            HStack(spacing: 12) {
                Button("Not now") { dismiss() }
                    .buttonStyle(SecondaryButtonStyle())
                    .keyboardShortcut(.cancelAction)
                Button("Mark as verified") {
                    app.perform { try app.core.confirmVerification(of: user, inPerson: false) }
                    dismiss()
                }
                .buttonStyle(PrimaryButtonStyle())
            }
        }
        .padding(28)
        .frame(width: 480)
    }
}
