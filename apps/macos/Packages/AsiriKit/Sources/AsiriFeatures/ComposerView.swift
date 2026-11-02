import AppKit
import AsiriDesign
import AsiriDomain
import AsiriPlatform
import SwiftUI
import UniformTypeIdentifiers

/// The composer (§13.3): Return sends, Shift-Return adds a line, ↑ edits the last message,
/// paste or drop files, record voice notes.
struct ComposerView: View {
    @Bindable var model: ConversationModel
    let app: AppModel
    @FocusState private var focused: Bool
    @State private var importing = false
    @State private var recorder: VoiceRecorder?
    @State private var recording = false
    @State private var levels: [Float] = []
    @State private var recordStart = Date()

    private var canSend: Bool { !model.draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }

    var body: some View {
        VStack(spacing: 0) {
            if let target = model.replyingTo ?? model.editing {
                ComposeContextBar(message: target, isEdit: model.editing != nil, app: app) {
                    withAnimation(Motion.standard) { model.cancelCompose() }
                }
                .transition(.move(edge: .bottom).combined(with: .opacity))
            }
            if model.chat?.hasLeft == true {
                Text("You can't send messages to this chat because you're no longer a member.")
                    .font(.system(size: 12.5))
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity)
                    .padding(14)
            } else {
                HStack(alignment: .bottom, spacing: 8) {
                    if recording {
                        recordingBar
                            .transition(.move(edge: .leading).combined(with: .opacity))
                    } else {
                        CircleIconButton("paperclip", size: 34) { importing = true }
                            .help("Attach a file (⌘O)")
                            .keyboardShortcut("o")
                        field
                            .transition(.opacity)
                        CircleIconButton("face.smiling", size: 34) {
                            focused = true
                            NSApp.orderFrontCharacterPalette(nil)
                        }
                        .help("Emoji (⌃⌘Space)")
                    }
                    sendButton
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 9)
                .animation(Motion.standard, value: recording)
            }
        }
        .background(.bar)
        .overlay(alignment: .top) { Divider() }
        .animation(Motion.standard, value: model.replyingTo?.id)
        .animation(Motion.standard, value: model.editing?.id)
        .fileImporter(isPresented: $importing, allowedContentTypes: [.item], allowsMultipleSelection: true) { result in
            if case let .success(urls) = result {
                Task { await model.send(files: urls) }
            }
        }
        .onAppear { focused = true }
        .onChange(of: model.replyingTo?.id) { if model.replyingTo != nil { focused = true } }
        .onChange(of: model.editing?.id) { if model.editing != nil { focused = true } }
    }

    private var field: some View {
        TextField(model.editing != nil ? "Edit message" : "Message", text: $model.draft, axis: .vertical)
            .textFieldStyle(.plain)
            .font(.system(size: 14 * app.prefs.textScale))
            .lineLimit(1 ... 8)
            .focused($focused)
            .padding(.horizontal, 14)
            .padding(.vertical, 9)
            .frame(minHeight: Metrics.composerMinHeight)
            .background(RoundedRectangle(cornerRadius: 19, style: .continuous).fill(Palette.surface))
            .overlay(RoundedRectangle(cornerRadius: 19, style: .continuous).stroke(focused ? Palette.accent.opacity(0.55) : Palette.hairline, lineWidth: focused ? 1.5 : 1))
            .animation(Motion.quick, value: focused)
            .onKeyPress(.return, phases: .down) { press in
                if press.modifiers.contains(.shift) || press.modifiers.contains(.option) {
                    model.draft += "\n"
                    return .handled
                }
                withAnimation(Motion.standard) { _ = model.send() }
                return .handled
            }
            .onKeyPress(.upArrow) {
                model.editLast() ? .handled : .ignored
            }
            .onKeyPress(.escape) {
                guard model.replyingTo != nil || model.editing != nil else { return .ignored }
                withAnimation(Motion.standard) { model.cancelCompose() }
                return .handled
            }
            .onPasteCommand(of: [.fileURL, .png, .tiff, .jpeg]) { providers in
                paste(providers)
            }
    }

    private var sendButton: some View {
        Button {
            if recording {
                finishRecording(send: true)
            } else if canSend {
                withAnimation(Motion.standard) { _ = model.send() }
            } else {
                startRecording()
            }
        } label: {
            Image(systemName: recording ? "arrow.up" : (canSend ? (model.editing != nil ? "checkmark" : "arrow.up") : "mic.fill"))
                .font(.system(size: 15, weight: .bold))
                .foregroundStyle(canSend || recording ? Color.white : Palette.accent)
                .contentTransition(.symbolEffect(.replace))
                .frame(width: 38, height: 38)
                .background(
                    Circle().fill(canSend || recording
                        ? AnyShapeStyle(LinearGradient(colors: [Palette.outgoingTop, Palette.outgoingBottom], startPoint: .top, endPoint: .bottom))
                        : AnyShapeStyle(Palette.accent.opacity(0.12)))
                )
                .scaleEffect(canSend || recording ? 1 : 0.94)
                .shadow(color: Palette.accent.opacity(canSend ? 0.35 : 0), radius: 8, y: 3)
        }
        .buttonStyle(PressableStyle())
        .animation(Motion.bouncy, value: canSend)
        .help(canSend ? "Send (Return)" : "Record a voice message")
        .accessibilityLabel(canSend ? "Send" : "Record voice message")
    }

    private var recordingBar: some View {
        HStack(spacing: 10) {
            Button { finishRecording(send: false) } label: {
                Image(systemName: "trash")
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundStyle(Palette.danger)
                    .frame(width: 34, height: 34)
            }
            .buttonStyle(PressableStyle())
            .help("Discard")
            Circle()
                .fill(Palette.danger)
                .frame(width: 9, height: 9)
                .phaseAnimator([1.0, 0.3]) { dot, o in dot.opacity(o) } animation: { _ in .easeInOut(duration: 0.6) }
            TimelineView(.periodic(from: recordStart, by: 0.5)) { context in
                Text(Formatting.duration(ms: Int(context.date.timeIntervalSince(recordStart) * 1000)))
                    .font(.system(size: 13, weight: .medium).monospacedDigit())
            }
            LiveLevels(levels: levels)
                .frame(height: 26)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 6)
        .frame(minHeight: Metrics.composerMinHeight)
        .background(RoundedRectangle(cornerRadius: 19, style: .continuous).fill(Palette.surface))
    }

    private func startRecording() {
        Task {
            do {
                let recorder = self.recorder ?? VoiceRecorder()
                self.recorder = recorder
                recorder.onLevel = { level in
                    levels.append(level)
                    if levels.count > 64 { levels.removeFirst(levels.count - 64) }
                }
                levels = []
                try await recorder.start()
                recordStart = Date()
                withAnimation(Motion.standard) { recording = true }
            } catch {
                app.show(error)
            }
        }
    }

    private func finishRecording(send: Bool) {
        withAnimation(Motion.standard) { recording = false }
        if send, let file = recorder?.stop() {
            Task { await model.send(file: file) }
        } else {
            recorder?.cancel()
        }
    }

    private func paste(_ providers: [NSItemProvider]) {
        for provider in providers {
            if provider.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier) {
                _ = provider.loadObject(ofClass: URL.self) { url, _ in
                    guard let url else { return }
                    Task { @MainActor in await model.send(files: [url]) }
                }
            } else if provider.canLoadObject(ofClass: NSImage.self) {
                _ = provider.loadObject(ofClass: NSImage.self) { image, _ in
                    guard let image = image as? NSImage, let tiff = image.tiffRepresentation else { return }
                    Task { @MainActor in
                        do {
                            let file = try MediaPrep.prepare(data: tiff, type: .tiff, fileName: "Pasted image")
                            await model.send(file: file)
                        } catch {
                            app.show(error)
                        }
                    }
                }
            }
        }
    }
}

/// Reply or edit context above the composer.
struct ComposeContextBar: View {
    let message: ChatMessage
    let isEdit: Bool
    let app: AppModel
    let close: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: isEdit ? "pencil" : "arrowshape.turn.up.left.fill")
                .font(.system(size: 14, weight: .semibold))
                .foregroundStyle(Palette.accent)
                .frame(width: 28)
            RoundedRectangle(cornerRadius: 1.5)
                .fill(Palette.accent)
                .frame(width: 3, height: 32)
            VStack(alignment: .leading, spacing: 1) {
                Text(isEdit ? "Edit message" : "Reply to \(app.name(of: message.sender))")
                    .font(.system(size: 12.5, weight: .semibold))
                    .foregroundStyle(Palette.accent)
                Text(message.preview)
                    .font(.system(size: 12.5))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            Spacer()
            Button(action: close) {
                Image(systemName: "xmark")
                    .font(.system(size: 11, weight: .bold))
                    .foregroundStyle(.secondary)
                    .frame(width: 24, height: 24)
                    .background(Circle().fill(Palette.hairline))
            }
            .buttonStyle(PressableStyle())
            .keyboardShortcut(.cancelAction)
        }
        .padding(.horizontal, 14)
        .padding(.top, 8)
        .padding(.bottom, 2)
    }
}

/// The live microphone level while recording.
struct LiveLevels: View {
    let levels: [Float]

    var body: some View {
        HStack(alignment: .center, spacing: 2) {
            ForEach(Array(levels.suffix(48).enumerated()), id: \.offset) { _, level in
                Capsule()
                    .fill(Palette.accent)
                    .frame(width: 2.5, height: max(3, 26 * CGFloat(level)))
            }
        }
        .animation(.linear(duration: 0.05), value: levels.count)
    }
}
