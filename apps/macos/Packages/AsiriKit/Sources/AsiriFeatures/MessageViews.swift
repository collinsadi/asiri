import AppKit
import AsiriDesign
import AsiriDomain
import AsiriPlatform
import SwiftUI

/// Quick reactions offered on hover and in the context menu.
let quickReactions = ["❤️", "👍", "😂", "😮", "😢", "🙏"]

/// One message: avatar (groups), bubble, hover actions.
struct MessageRow: View {
    let message: ChatMessage
    let grouping: BubbleGrouping
    let model: ConversationModel
    let app: AppModel
    let openMedia: (ChatMessage) -> Void
    @State private var hovering = false
    @State private var showReactions = false

    private var outgoing: Bool { message.isOutgoing }
    private var isGroup: Bool { model.chat?.kind == .group }

    var body: some View {
        HStack(alignment: .bottom, spacing: 6) {
            if outgoing { Spacer(minLength: 64) }
            if isGroup, !outgoing {
                if grouping.showAvatar {
                    Avatar(id: message.sender, name: app.name(of: message.sender), image: app.contact(message.sender)?.avatar, size: 30)
                        .padding(.bottom, 1)
                } else {
                    Color.clear.frame(width: 30, height: 1)
                }
            }
            if outgoing { hoverActions }
            BubbleView(message: message, grouping: grouping, model: model, app: app, openMedia: openMedia)
                .frame(maxWidth: Metrics.bubbleMaxWidth, alignment: outgoing ? .trailing : .leading)
                .contextMenu { menu }
                .onTapGesture(count: 2) { model.reply(to: message) }
            if !outgoing { hoverActions }
            if !outgoing { Spacer(minLength: 64) }
        }
        .padding(.top, grouping.groupedAbove ? 2 : 8)
        .contentShape(Rectangle())
        .onHover { h in withAnimation(Motion.quick) { hovering = h } }
    }

    @ViewBuilder
    private var hoverActions: some View {
        if message.isBubble, !message.isDeleted, model.chat?.hasLeft != true {
            HStack(spacing: 2) {
                Button { showReactions.toggle() } label: {
                    Image(systemName: "face.smiling")
                        .font(.system(size: 13, weight: .medium))
                        .frame(width: 26, height: 26)
                        .background(Circle().fill(.regularMaterial))
                }
                .buttonStyle(PressableStyle())
                .popover(isPresented: $showReactions, arrowEdge: .top) {
                    ReactionPicker { emoji in
                        showReactions = false
                        model.toggleReaction(emoji, on: message)
                    }
                }
                Button { model.reply(to: message) } label: {
                    Image(systemName: "arrowshape.turn.up.left.fill")
                        .font(.system(size: 11, weight: .medium))
                        .frame(width: 26, height: 26)
                        .background(Circle().fill(.regularMaterial))
                }
                .buttonStyle(PressableStyle())
            }
            .foregroundStyle(.secondary)
            .opacity(hovering || showReactions ? 1 : 0)
            .scaleEffect(hovering || showReactions ? 1 : 0.8)
            .padding(.bottom, 4)
            .help("React or reply")
        }
    }

    @ViewBuilder
    private var menu: some View {
        if message.isBubble, !message.isDeleted {
            Button("Reply", systemImage: "arrowshape.turn.up.left") { model.reply(to: message) }
            Menu("React") {
                ForEach(quickReactions, id: \.self) { e in
                    Button(e) { model.toggleReaction(e, on: message) }
                }
            }
            if !message.body.isEmpty {
                Button("Copy", systemImage: "doc.on.doc") { model.copy(message) }
            }
            if message.attachment != nil, message.attachment?.state == .local {
                Button("Save As…", systemImage: "square.and.arrow.down") { model.save(message) }
            }
            if outgoing, message.kind == .text {
                Button("Edit", systemImage: "pencil") { model.beginEdit(message) }
            }
            if message.status == .failed {
                Button("Try Again", systemImage: "arrow.clockwise") { model.retry(message) }
            }
            Divider()
        }
        Button("Delete for Me", systemImage: "trash") { model.delete(message, forEveryone: false) }
        if outgoing, !message.isDeleted {
            Button("Delete for Everyone", systemImage: "trash.fill", role: .destructive) { model.delete(message, forEveryone: true) }
        }
    }
}

/// The bubble and everything inside it.
struct BubbleView: View {
    let message: ChatMessage
    let grouping: BubbleGrouping
    let model: ConversationModel
    let app: AppModel
    let openMedia: (ChatMessage) -> Void
    @Environment(\.textScale) private var textScale

    private var outgoing: Bool { message.isOutgoing }
    private var highlighted: Bool { model.highlighted == message.id }

    var body: some View {
        Group {
            if message.isDeleted {
                textBubble { deletedBody }
            } else if message.kind == .call {
                textBubble { callBody }
            } else if message.kind == .text, Self.isEmojiOnly(message.body), message.replyTo == nil {
                bigEmoji
            } else if let a = message.attachment, a.isImage || a.isVideo {
                photoBubble(a)
            } else if let a = message.attachment, a.isVoiceNote {
                textBubble { VoiceNoteView(message: message, attachment: a, model: model) }
            } else if let a = message.attachment {
                textBubble { FileCard(message: message, attachment: a, model: model) }
            } else {
                textBubble { textBody }
            }
        }
        .overlay(alignment: outgoing ? .topLeading : .topTrailing) {
            ReactionBurst(trigger: burstTrigger)
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel(accessibilityText)
    }

    private var burstTrigger: String? {
        guard let reaction = model.lastReaction, reaction.0 == message.id else { return nil }
        return reaction.1
    }

    private var accessibilityText: String {
        let who = outgoing ? "You" : app.name(of: message.sender)
        let status = outgoing ? StatusTicks.label(message.status) : ""
        return "\(who), \(message.preview), \(Formatting.messageTime(message.sentAt)) \(status)"
    }

    // MARK: Bubble chrome

    private var fg: Color { outgoing ? Palette.outgoingText : Palette.incomingText }
    private var fg2: Color { outgoing ? Palette.outgoingSecondary : Palette.incomingSecondary }

    @ViewBuilder
    private func textBubble<Content: View>(@ViewBuilder _ content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            senderName
            if let reply = message.replyTo { ReplyQuote(messageID: reply, outgoing: outgoing, model: model, app: app) }
            content()
            if !message.reactions.isEmpty { reactionsRow.padding(.top, 2) }
        }
        .padding(.horizontal, Metrics.bubblePaddingH)
        .padding(.vertical, Metrics.bubblePaddingV)
        .padding(outgoing ? .trailing : .leading, BubbleShape.tailWidth)
        .background(BubbleBackground(outgoing: outgoing, tail: grouping.hasTail, groupedAbove: grouping.groupedAbove,
                                     groupedBelow: grouping.groupedBelow, highlighted: highlighted))
        .animation(Motion.standard, value: highlighted)
    }

    @ViewBuilder
    private var senderName: some View {
        if grouping.showName, !outgoing {
            Text(app.name(of: message.sender))
                .font(.system(size: 12.5 * textScale, weight: .semibold))
                .foregroundStyle(Palette.senderColors[Palette.index(for: message.sender, count: Palette.senderColors.count)])
                .lineLimit(1)
        }
    }

    // MARK: Text

    /// Text with the time tucked into the last line, Telegram-style: an invisible copy of the
    /// time reserves room, and the real one sits in the bottom-right corner.
    private var textBody: some View {
        (Self.linkified(message.body, color: fg, linkColor: outgoing ? .white : Palette.accent)
            + Text(verbatim: "  " + metaPlaceholder).font(Typo.meta).foregroundColor(.clear))
            .font(.system(size: 14 * textScale))
            .foregroundColor(fg)
            .fixedSize(horizontal: false, vertical: true)
            .overlay(alignment: .bottomTrailing) { meta.offset(y: 2) }
    }

    private var metaPlaceholder: String {
        var s = ""
        if message.isEdited { s += "edited " }
        s += Formatting.messageTime(message.sentAt)
        if outgoing { s += "    " }
        if message.expiresAt != nil { s += "   " }
        return s
    }

    private var meta: some View {
        HStack(spacing: 3) {
            if message.expiresAt != nil {
                Image(systemName: "timer").font(.system(size: 9, weight: .semibold))
            }
            if message.isEdited { Text("edited") }
            Text(Formatting.messageTime(message.sentAt))
            if outgoing {
                StatusTicks(message.status, tint: fg2, readTint: outgoing ? .white : Palette.accent)
                    .onTapGesture { if message.status == .failed { model.retry(message) } }
            }
        }
        .font(Typo.meta)
        .foregroundStyle(fg2)
        .lineLimit(1)
        .fixedSize()
    }

    private var deletedBody: some View {
        HStack(spacing: 5) {
            Image(systemName: "nosign")
            Text(outgoing ? "You deleted this message" : "This message was deleted").italic()
            Text(Formatting.messageTime(message.sentAt)).font(Typo.meta)
        }
        .font(.system(size: 13 * textScale))
        .foregroundStyle(fg2)
    }

    private var callBody: some View {
        let missed = message.body.hasPrefix("Missed")
        return HStack(spacing: 8) {
            Image(systemName: message.body.contains("Video") || message.body.contains("video") ? "video.fill" : "phone.fill")
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(missed ? Palette.danger : (outgoing ? Color.white : Palette.success))
                .frame(width: 30, height: 30)
                .background(Circle().fill(outgoing ? Color.white.opacity(0.18) : Palette.success.opacity(0.12)))
            VStack(alignment: .leading, spacing: 1) {
                Text(message.body).font(.system(size: 13.5 * textScale, weight: .medium)).foregroundStyle(fg)
                Text(Formatting.messageTime(message.sentAt)).font(Typo.meta).foregroundStyle(fg2)
            }
        }
        .padding(.vertical, 2)
    }

    private var bigEmoji: some View {
        VStack(alignment: outgoing ? .trailing : .leading, spacing: 2) {
            Text(message.body)
                .font(.system(size: 46 * textScale))
                .phaseAnimator([1.0, 1.08], trigger: message.reactions.count) { v, s in v.scaleEffect(s) }
            meta
                .padding(.horizontal, 7)
                .padding(.vertical, 2)
                .background(Capsule().fill(Palette.pill))
                .foregroundStyle(.white)
            if !message.reactions.isEmpty { reactionsRow }
        }
    }

    // MARK: Photos

    private func photoBubble(_ a: Attachment) -> some View {
        let width: CGFloat = a.width > 0 ? 340 : 260
        let height: CGFloat = min(420, max(120, width / max(a.aspectRatio, 0.4)))
        return VStack(alignment: .leading, spacing: 0) {
            if grouping.showName, !outgoing {
                senderName.padding(.horizontal, 10).padding(.top, 6).padding(.bottom, 4)
            }
            if let reply = message.replyTo {
                ReplyQuote(messageID: reply, outgoing: outgoing, model: model, app: app).padding(6)
            }
            PhotoView(message: message, attachment: a, model: model)
                .frame(width: width, height: height)
                .clipShape(RoundedRectangle(cornerRadius: message.body.isEmpty ? 14 : 12, style: .continuous))
                .padding(3)
                .overlay(alignment: .bottomTrailing) {
                    if message.body.isEmpty {
                        meta
                            .padding(.horizontal, 7)
                            .padding(.vertical, 3)
                            .background(Capsule().fill(Color.black.opacity(0.45)))
                            .foregroundStyle(.white)
                            .padding(9)
                    }
                }
                .onTapGesture { if a.state == .local { openMedia(message) } else { model.download(message) } }
            if !message.body.isEmpty {
                textBody.padding(.horizontal, 10).padding(.vertical, 6)
            }
            if !message.reactions.isEmpty { reactionsRow.padding(.horizontal, 8).padding(.bottom, 6) }
        }
        .frame(width: width + 6)
        .padding(outgoing ? .trailing : .leading, BubbleShape.tailWidth)
        .background(BubbleBackground(outgoing: outgoing, tail: grouping.hasTail, groupedAbove: grouping.groupedAbove,
                                     groupedBelow: grouping.groupedBelow, highlighted: highlighted))
    }

    // MARK: Reactions

    private var reactionsRow: some View {
        HStack(spacing: 4) {
            ForEach(message.reactions, id: \.emoji) { r in
                Button {
                    model.toggleReaction(r.emoji, on: message)
                } label: {
                    HStack(spacing: 3) {
                        Text(r.emoji).font(.system(size: 13))
                        if r.count > 1 || isGroupChat {
                            Text("\(r.count)")
                                .font(.system(size: 11.5, weight: .semibold))
                                .contentTransition(.numericText(value: Double(r.count)))
                        }
                    }
                    .padding(.horizontal, 7)
                    .padding(.vertical, 3)
                    .background(Capsule().fill(reactionFill(r.mine)))
                    .overlay(Capsule().stroke(r.mine ? (outgoing ? Color.white : Palette.accent) : Color.clear, lineWidth: 1.2))
                    .foregroundStyle(outgoing ? Color.white : Palette.accent)
                }
                .buttonStyle(PressableStyle())
                .transition(.scale(scale: 0.3).combined(with: .opacity))
            }
        }
        .animation(Motion.bouncy, value: message.reactions)
    }

    private var isGroupChat: Bool { model.chat?.kind == .group }

    private func reactionFill(_ mine: Bool) -> Color {
        if outgoing { return Color.white.opacity(mine ? 0.32 : 0.18) }
        return Palette.accent.opacity(mine ? 0.2 : 0.1)
    }

    // MARK: Helpers

    /// One to three emoji and nothing else: shown large, without a bubble.
    static func isEmojiOnly(_ text: String) -> Bool {
        let trimmed = text.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty, trimmed.count <= 3 else { return false }
        return trimmed.allSatisfy { c in
            c.unicodeScalars.contains { $0.properties.isEmojiPresentation || ($0.properties.isEmoji && $0.value > 0x238C) }
        }
    }

    /// Plain text with tappable links (found locally; nothing is fetched).
    static func linkified(_ text: String, color: Color, linkColor: Color) -> Text {
        var attributed = AttributedString(text)
        if let detector = try? NSDataDetector(types: NSTextCheckingResult.CheckingType.link.rawValue) {
            let ns = text as NSString
            for match in detector.matches(in: text, range: NSRange(location: 0, length: ns.length)) {
                guard let url = match.url, let range = Range(match.range, in: text),
                      let lower = AttributedString.Index(range.lowerBound, within: attributed),
                      let upper = AttributedString.Index(range.upperBound, within: attributed)
                else { continue }
                attributed[lower ..< upper].link = url
                attributed[lower ..< upper].foregroundColor = linkColor
            }
        }
        return Text(attributed)
    }
}

// MARK: - Reply quote

struct ReplyQuote: View {
    let messageID: MessageID
    let outgoing: Bool
    let model: ConversationModel
    let app: AppModel

    var body: some View {
        let quoted = model.quoted(messageID)
        let color = quoted.map { Palette.senderColors[Palette.index(for: $0.sender, count: Palette.senderColors.count)] } ?? Palette.accent
        Button {
            model.highlighted = messageID
        } label: {
            HStack(spacing: 7) {
                RoundedRectangle(cornerRadius: 2)
                    .fill(outgoing ? Color.white : color)
                    .frame(width: 3)
                VStack(alignment: .leading, spacing: 1) {
                    Text(quoted.map { app.name(of: $0.sender) } ?? "Message")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(outgoing ? Color.white : color)
                    Text(quoted?.preview ?? "Not available")
                        .font(.system(size: 12))
                        .foregroundStyle(outgoing ? Palette.outgoingSecondary : Palette.incomingSecondary)
                        .lineLimit(1)
                }
                Spacer(minLength: 0)
            }
            .padding(.vertical, 4)
            .padding(.trailing, 8)
            .background(RoundedRectangle(cornerRadius: 6, style: .continuous).fill((outgoing ? Color.white : color).opacity(0.14)))
        }
        .buttonStyle(.plain)
        .frame(minWidth: 140, alignment: .leading)
    }
}

// MARK: - Reactions

struct ReactionPicker: View {
    let pick: (String) -> Void
    @State private var shown = false

    var body: some View {
        HStack(spacing: 6) {
            ForEach(Array(quickReactions.enumerated()), id: \.offset) { i, e in
                Button { pick(e) } label: {
                    Text(e).font(.system(size: 24))
                }
                .buttonStyle(ReactionButtonStyle())
                .scaleEffect(shown ? 1 : 0.2)
                .opacity(shown ? 1 : 0)
                .animation(Motion.bouncy.delay(Double(i) * 0.03), value: shown)
            }
            Button {
                NSApp.orderFrontCharacterPalette(nil)
            } label: {
                Image(systemName: "plus").font(.system(size: 13, weight: .bold)).frame(width: 30, height: 30)
            }
            .buttonStyle(ReactionButtonStyle())
            .help("More emoji")
        }
        .padding(8)
        .onAppear { shown = true }
    }
}

struct ReactionButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        ReactionButtonBody(configuration: configuration)
    }
}

/// Grows on hover, shrinks on press.
struct ReactionButtonBody: View {
    let configuration: ButtonStyleConfiguration
    @State private var hovering = false

    var body: some View {
        configuration.label
            .frame(width: 36, height: 36)
            .background(Circle().fill(Palette.accent.opacity(hovering ? 0.14 : 0)))
            .scaleEffect(configuration.isPressed ? 0.85 : (hovering ? 1.18 : 1))
            .animation(Motion.bouncy, value: hovering)
            .animation(Motion.snappy, value: configuration.isPressed)
            .onHover { hovering = $0 }
    }
}

/// A little burst of the emoji just added, flying up and fading.
struct ReactionBurst: View {
    let trigger: String?
    @State private var progress: CGFloat = 1
    @State private var emoji = ""

    var body: some View {
        ZStack {
            ForEach(0 ..< 7, id: \.self) { i in
                let angle = CGFloat(i) / 7 * 2 * .pi + 0.4
                Text(emoji)
                    .font(.system(size: 14 + CGFloat(i % 3) * 3))
                    .offset(x: cos(angle) * 34 * progress, y: sin(angle) * 26 * progress - 26 * progress)
                    .opacity(Double(1 - progress))
                    .scaleEffect(0.6 + progress * 0.6)
            }
        }
        .allowsHitTesting(false)
        .onChange(of: trigger) {
            guard let trigger else { return }
            emoji = trigger
            progress = 0
            withAnimation(.easeOut(duration: 0.75)) { progress = 1 }
        }
    }
}

// MARK: - Attachments

/// Decrypted images, kept in memory only.
@MainActor
final class ImageCache {
    static let shared = ImageCache()
    private let cache = NSCache<NSData, NSImage>()

    func image(_ id: MessageID) -> NSImage? { cache.object(forKey: id as NSData) }
    func set(_ image: NSImage, for id: MessageID) { cache.setObject(image, forKey: id as NSData) }
}

struct PhotoView: View {
    let message: ChatMessage
    let attachment: Attachment
    let model: ConversationModel
    @State private var full: NSImage?

    var body: some View {
        ZStack {
            if let full {
                Image(nsImage: full).resizable().scaledToFill().transition(.opacity)
            } else if let thumb = attachment.thumbnail.flatMap(NSImage.init(data:)) {
                Image(nsImage: thumb).resizable().scaledToFill().blur(radius: attachment.state == .local ? 0 : 6)
            } else if let blur = BlurHash.image(attachment.blurhash) {
                Image(nsImage: blur).resizable().interpolation(.high).scaledToFill()
            } else {
                LinearGradient(colors: [Palette.accent.opacity(0.4), Palette.violet.opacity(0.4)], startPoint: .topLeading, endPoint: .bottomTrailing)
            }
            if attachment.state != .local {
                TransferRing(state: attachment.state, progress: attachment.progress, size: attachment.size)
            }
        }
        .animation(Motion.standard, value: full != nil)
        .task(id: attachment.state) { await load() }
    }

    private func load() async {
        guard attachment.state == .local else { return }
        if let cached = ImageCache.shared.image(message.id) {
            full = cached
            return
        }
        let id = message.id
        let core = model.app.core
        // Reading and decrypting happen off the main actor. Only the bytes cross back:
        // NSImage isn't Sendable, and it decodes lazily anyway.
        let data = await Task.detached(priority: .userInitiated) { () -> Data? in
            guard let data = try? core.attachmentData(id), !data.isEmpty else { return nil }
            return data
        }.value
        if let data, let image = NSImage(data: data) {
            ImageCache.shared.set(image, for: id)
            full = image
        }
    }
}

struct TransferRing: View {
    let state: TransferState
    let progress: Double
    let size: UInt64

    var body: some View {
        VStack(spacing: 6) {
            ZStack {
                Circle().fill(Color.black.opacity(0.45))
                if state == .uploading || state == .downloading {
                    Circle()
                        .trim(from: 0, to: max(0.04, progress))
                        .stroke(Color.white, style: StrokeStyle(lineWidth: 2.5, lineCap: .round))
                        .rotationEffect(.degrees(-90))
                        .padding(5)
                        .animation(Motion.standard, value: progress)
                }
                Image(systemName: icon)
                    .font(.system(size: 15, weight: .bold))
                    .foregroundStyle(.white)
            }
            .frame(width: 44, height: 44)
            if state == .remote {
                Text(Formatting.bytes(size))
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(.white)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 2)
                    .background(Capsule().fill(Color.black.opacity(0.45)))
            }
        }
    }

    private var icon: String {
        switch state {
        case .remote: "arrow.down"
        case .downloading, .uploading: "xmark"
        case .failed: "arrow.clockwise"
        case .local: "checkmark"
        }
    }
}

struct FileCard: View {
    let message: ChatMessage
    let attachment: Attachment
    let model: ConversationModel

    private var outgoing: Bool { message.isOutgoing }

    var body: some View {
        HStack(spacing: 10) {
            Button {
                switch attachment.state {
                case .local: model.save(message)
                case .remote, .failed: model.download(message)
                default: break
                }
            } label: {
                ZStack {
                    Circle().fill(outgoing ? Color.white.opacity(0.22) : Palette.accent)
                    if attachment.state == .downloading || attachment.state == .uploading {
                        Circle()
                            .trim(from: 0, to: max(0.04, attachment.progress))
                            .stroke(Color.white, style: StrokeStyle(lineWidth: 2.2, lineCap: .round))
                            .rotationEffect(.degrees(-90))
                            .padding(4)
                    }
                    Image(systemName: attachment.state == .local ? "doc.fill" : (attachment.state == .remote ? "arrow.down" : "xmark"))
                        .font(.system(size: 15, weight: .bold))
                        .foregroundStyle(.white)
                }
                .frame(width: 42, height: 42)
            }
            .buttonStyle(PressableStyle())
            VStack(alignment: .leading, spacing: 2) {
                Text(attachment.fileName)
                    .font(.system(size: 13.5, weight: .semibold))
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .foregroundStyle(outgoing ? Palette.outgoingText : Palette.incomingText)
                HStack(spacing: 4) {
                    Text(Formatting.bytes(attachment.size))
                    Text("·")
                    Text(Formatting.messageTime(message.sentAt))
                    if outgoing { StatusTicks(message.status, tint: Palette.outgoingSecondary, readTint: .white) }
                }
                .font(Typo.meta)
                .foregroundStyle(outgoing ? Palette.outgoingSecondary : Palette.incomingSecondary)
            }
            .frame(minWidth: 120, alignment: .leading)
        }
        .padding(.vertical, 2)
    }
}

struct VoiceNoteView: View {
    let message: ChatMessage
    let attachment: Attachment
    let model: ConversationModel
    @State private var player = VoicePlayer.shared
    @State private var tick = 0

    private var outgoing: Bool { message.isOutgoing }
    private var playing: Bool { player.playing == message.id }

    var body: some View {
        HStack(spacing: 10) {
            Button {
                if attachment.state == .local, let data = model.data(of: message) {
                    player.onChange = { tick += 1 }
                    player.toggle(message.id, data: data)
                    tick += 1
                } else {
                    model.download(message)
                }
            } label: {
                Image(systemName: attachment.state != .local ? "arrow.down" : (playing ? "pause.fill" : "play.fill"))
                    .font(.system(size: 15, weight: .bold))
                    .foregroundStyle(outgoing ? Palette.outgoingBottom : Color.white)
                    .contentTransition(.symbolEffect(.replace))
                    .frame(width: 40, height: 40)
                    .background(Circle().fill(outgoing ? Color.white : Palette.accent))
            }
            .buttonStyle(PressableStyle())
            VStack(alignment: .leading, spacing: 4) {
                TimelineView(.animation(minimumInterval: 1 / 20, paused: !playing)) { _ in
                    Waveform(levels: attachment.waveform, progress: playing ? player.progress : 0,
                             active: outgoing ? Color.white : Palette.accent,
                             inactive: outgoing ? Color.white.opacity(0.45) : Palette.accent.opacity(0.3))
                }
                .frame(width: 170, height: 24)
                HStack(spacing: 4) {
                    Text(Formatting.duration(ms: attachment.durationMs))
                    Spacer()
                    Text(Formatting.messageTime(message.sentAt))
                    if outgoing { StatusTicks(message.status, tint: Palette.outgoingSecondary, readTint: .white) }
                }
                .font(Typo.meta)
                .foregroundStyle(outgoing ? Palette.outgoingSecondary : Palette.incomingSecondary)
            }
            // A fixed width: the Spacer in the meta row would otherwise stretch the bubble.
            .frame(width: 170)
        }
        .id(tick)
        .padding(.vertical, 2)
    }
}

struct Waveform: View {
    let levels: [UInt8]
    let progress: Double
    let active: Color
    let inactive: Color

    var body: some View {
        GeometryReader { geo in
            let bars = levels.isEmpty ? Array(repeating: UInt8(40), count: 40) : levels
            let w = geo.size.width / CGFloat(bars.count)
            HStack(alignment: .center, spacing: w * 0.35) {
                ForEach(Array(bars.enumerated()), id: \.offset) { i, level in
                    Capsule()
                        .fill(Double(i) / Double(bars.count) < progress ? active : inactive)
                        .frame(width: max(1.5, w * 0.65), height: max(3, geo.size.height * CGFloat(level) / 255))
                }
            }
            .frame(maxHeight: .infinity)
        }
    }
}

// MARK: - Viewer

struct MediaViewer: View {
    let message: ChatMessage
    let model: ConversationModel
    @Environment(\.dismiss) private var dismiss
    @State private var image: NSImage?
    @State private var zoom: CGFloat = 1

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text(message.attachment?.fileName ?? "")
                    .font(.system(size: 13, weight: .semibold))
                Spacer()
                Button("Save As…") { model.save(message) }
                Button("Done") { dismiss() }.keyboardShortcut(.cancelAction)
            }
            .padding(12)
            ZStack {
                Color.black
                if let image {
                    Image(nsImage: image)
                        .resizable()
                        .scaledToFit()
                        .scaleEffect(zoom)
                        .gesture(MagnifyGesture().onChanged { zoom = max(1, min(5, $0.magnification)) }.onEnded { _ in
                            withAnimation(Motion.standard) { if zoom < 1.1 { zoom = 1 } }
                        })
                        .onTapGesture(count: 2) { withAnimation(Motion.standard) { zoom = zoom > 1 ? 1 : 2.5 } }
                } else {
                    ProgressView()
                }
            }
        }
        .frame(minWidth: 640, minHeight: 480)
        .task {
            image = ImageCache.shared.image(message.id) ?? model.data(of: message).flatMap(NSImage.init(data:))
        }
    }
}
