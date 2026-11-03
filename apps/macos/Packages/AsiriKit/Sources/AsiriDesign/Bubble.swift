import SwiftUI

/// A message bubble with a tail on the sender's side (ADR-0005).
///
/// Consecutive messages from the same person are grouped: only the last one has a tail, and the
/// corners where bubbles meet are tighter. Space for the tail is always reserved, so grouped
/// bubbles line up.
public struct BubbleShape: Shape {
    public static let tailWidth: CGFloat = 6

    public var outgoing: Bool
    public var tail: Bool
    public var groupedAbove: Bool
    public var groupedBelow: Bool

    public init(outgoing: Bool, tail: Bool, groupedAbove: Bool = false, groupedBelow: Bool = false) {
        self.outgoing = outgoing
        self.tail = tail
        self.groupedAbove = groupedAbove
        self.groupedBelow = groupedBelow
    }

    public func path(in rect: CGRect) -> Path {
        let w = rect.width
        let h = rect.height
        let bodyMaxX = w - Self.tailWidth
        let large = min(Metrics.bubbleRadius, h / 2, bodyMaxX / 2)
        let small = min(Metrics.bubbleTightRadius, h / 2)
        let rTL = large
        let rTR = groupedAbove ? small : large
        let rBR = groupedBelow ? small : large
        let rBL = large

        var p = Path()
        p.move(to: CGPoint(x: rTL, y: 0))
        p.addLine(to: CGPoint(x: bodyMaxX - rTR, y: 0))
        p.addArc(tangent1End: CGPoint(x: bodyMaxX, y: 0), tangent2End: CGPoint(x: bodyMaxX, y: rTR), radius: rTR)
        if tail {
            let start = max(rTR, h - 12)
            p.addLine(to: CGPoint(x: bodyMaxX, y: start))
            // Sweep out to the tip, then run back along the bottom edge.
            p.addQuadCurve(to: CGPoint(x: w, y: h), control: CGPoint(x: bodyMaxX, y: h - 1))
            p.addQuadCurve(to: CGPoint(x: max(rBL, bodyMaxX - 14), y: h), control: CGPoint(x: bodyMaxX - 3, y: h))
        } else {
            p.addLine(to: CGPoint(x: bodyMaxX, y: h - rBR))
            p.addArc(tangent1End: CGPoint(x: bodyMaxX, y: h), tangent2End: CGPoint(x: bodyMaxX - rBR, y: h), radius: rBR)
        }
        p.addLine(to: CGPoint(x: rBL, y: h))
        p.addArc(tangent1End: CGPoint(x: 0, y: h), tangent2End: CGPoint(x: 0, y: h - rBL), radius: rBL)
        p.addLine(to: CGPoint(x: 0, y: rTL))
        p.addArc(tangent1End: CGPoint(x: 0, y: 0), tangent2End: CGPoint(x: rTL, y: 0), radius: rTL)
        p.closeSubpath()

        let transform = outgoing
            ? CGAffineTransform(translationX: rect.minX, y: rect.minY)
            : CGAffineTransform(translationX: rect.maxX, y: rect.minY).scaledBy(x: -1, y: 1)
        return p.applying(transform)
    }
}

/// The fill for a bubble: an indigo gradient for the user's own messages.
public struct BubbleBackground: View {
    let outgoing: Bool
    let tail: Bool
    let groupedAbove: Bool
    let groupedBelow: Bool
    let highlighted: Bool

    public init(outgoing: Bool, tail: Bool, groupedAbove: Bool, groupedBelow: Bool, highlighted: Bool = false) {
        self.outgoing = outgoing
        self.tail = tail
        self.groupedAbove = groupedAbove
        self.groupedBelow = groupedBelow
        self.highlighted = highlighted
    }

    public var body: some View {
        let shape = BubbleShape(outgoing: outgoing, tail: tail, groupedAbove: groupedAbove, groupedBelow: groupedBelow)
        Group {
            if outgoing {
                shape.fill(LinearGradient(colors: [Palette.outgoingTop, Palette.outgoingBottom], startPoint: .top, endPoint: .bottom))
            } else {
                shape.fill(Palette.incoming)
            }
        }
        .overlay {
            shape.fill(Color.white.opacity(highlighted ? 0.16 : 0))
        }
        .shadow(color: .black.opacity(outgoing ? 0.12 : 0.07), radius: 0.8, y: 0.6)
    }
}
