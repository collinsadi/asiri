import Foundation

/// Dates, sizes and durations the way the UI shows them.
public enum Formatting {
    private static func formatter(_ template: String) -> DateFormatter {
        let f = DateFormatter()
        f.locale = .current
        f.setLocalizedDateFormatFromTemplate(template)
        return f
    }

    private static let time = formatter("jmm")
    private static let weekday = formatter("EEE")
    private static let weekdayLong = formatter("EEEE")
    private static let dayMonth = formatter("d MMMM")
    private static let dayMonthYear = formatter("d MMMM yyyy")
    private static let short = formatter("d/M/yy")

    /// "9:41" today, "Tue" this week, "12/9/26" before.
    public static func chatListTime(_ date: Date, now: Date = Date(), calendar: Calendar = .current) -> String {
        if calendar.isDate(date, inSameDayAs: now) { return time.string(from: date) }
        if let days = calendar.dateComponents([.day], from: calendar.startOfDay(for: date), to: calendar.startOfDay(for: now)).day, days < 7 {
            return weekday.string(from: date)
        }
        return short.string(from: date)
    }

    /// The time under a bubble.
    public static func messageTime(_ date: Date) -> String {
        time.string(from: date)
    }

    /// "Today", "Yesterday", "Monday", "12 September", "12 September 2025".
    public static func daySeparator(_ date: Date, now: Date = Date(), calendar: Calendar = .current) -> String {
        if calendar.isDate(date, inSameDayAs: now) { return "Today" }
        if let yesterday = calendar.date(byAdding: .day, value: -1, to: now), calendar.isDate(date, inSameDayAs: yesterday) {
            return "Yesterday"
        }
        if let days = calendar.dateComponents([.day], from: calendar.startOfDay(for: date), to: calendar.startOfDay(for: now)).day, days < 7 {
            return weekdayLong.string(from: date)
        }
        if calendar.component(.year, from: date) == calendar.component(.year, from: now) {
            return dayMonth.string(from: date)
        }
        return dayMonthYear.string(from: date)
    }

    /// "4.2 MB".
    public static func bytes(_ count: UInt64) -> String {
        ByteCountFormatter.string(fromByteCount: Int64(clamping: count), countStyle: .file)
    }

    /// "0:14", "12:03", "1:02:45".
    public static func duration(ms: Int) -> String {
        let total = max(0, ms / 1000)
        let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60)
        return h > 0 ? String(format: "%d:%02d:%02d", h, m, s) : String(format: "%d:%02d", m, s)
    }

    /// "last seen" style relative phrase for call history: "2 minutes ago".
    public static func relative(_ date: Date, now: Date = Date()) -> String {
        let f = RelativeDateTimeFormatter()
        f.unitsStyle = .full
        return f.localizedString(for: date, relativeTo: now)
    }

    /// Groups a fingerprint or safety number for reading aloud.
    public static func grouped(_ text: String, every n: Int = 4) -> String {
        let clean = text.filter { !$0.isWhitespace }
        var out = ""
        for (i, c) in clean.enumerated() {
            if i > 0, i % n == 0 { out.append(" ") }
            out.append(c)
        }
        return out
    }
}
