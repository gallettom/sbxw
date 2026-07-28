import Foundation

/// Mirrors sbxw's `SessionState` (see `src/web.rs`).
enum SessionState: String, Codable, Equatable {
    case working
    case idle
    case attention
    case exited
}

/// A pending numbered menu the agent is waiting on (parsed server-side).
struct Question: Codable, Equatable {
    let text: String
    let options: [String]
    /// On-screen context above the question (a diff, a decision table…).
    var context: [String] = []
}

/// Rich description of a session — the payload for both `/api/events` and the
/// `/api/sessions` snapshot. JSON keys are snake_case, matched here directly.
struct SessionInfo: Codable, Equatable, Identifiable {
    let sandbox: String
    let mode: String
    let state: SessionState
    let agent: String
    let started_ms: UInt64
    let activity: String?
    let last_input: String?
    /// First step of the pending prompt (what the row's subtitle shows).
    let question: Question?
    /// Every step of the pending prompt, in the order the terminal lays them
    /// out as tabs. Absent from a daemon that predates multi-step prompts.
    let steps: [Question]?
    /// What Claude last said, read off the transcript when its turn ended.
    /// Absent on an older daemon, and while a turn is in flight.
    let reply: String?
    let ts: UInt64?

    var id: String { "\(sandbox)::\(mode)" }

    /// Opening sentence of Claude's reply — the row's one-line caption.
    ///
    /// Split on sentence punctuation rather than on the first line: Claude's
    /// answers often open with a long paragraph, and a whole line of it would
    /// be truncated mid-word anyway.
    var replyLead: String? {
        guard let reply, !reply.isEmpty else { return nil }
        let flat = reply.replacingOccurrences(of: "\n", with: " ")
        var sentence = flat
        if let end = flat.firstIndex(where: { ".!?".contains($0) }) {
            let upTo = flat.index(after: end)
            // A trailing "…" or a decimal point isn't the end of a thought.
            if flat.distance(from: flat.startIndex, to: upTo) > 12 {
                sentence = String(flat[..<upTo])
            }
        }
        let trimmed = sentence.trimmingCharacters(in: .whitespaces)
        return trimmed.isEmpty ? nil : trimmed
    }

    /// The whole reply, for the accordion a hovered row opens — nil when the
    /// lead sentence already *is* the whole reply, so there is nothing to
    /// unfold. Splitting on newlines instead would strand the common case: a
    /// two-sentence answer arrives as one line, and everything past the first
    /// sentence would be unreachable.
    var replyFull: String? {
        guard let reply else { return nil }
        let trimmed = reply.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, trimmed != replyLead else { return nil }
        return trimmed
    }

    /// The prompt as a list of steps — one entry for a single question, empty
    /// when nothing is pending. Falls back to `question` so an older daemon
    /// still produces a (single-step) card.
    var promptSteps: [Question] {
        if let steps, !steps.isEmpty { return steps }
        return question.map { [$0] } ?? []
    }

    /// Seconds since the session's PTY started (0 if unknown).
    var elapsed: TimeInterval {
        guard started_ms > 0 else { return 0 }
        return max(0, Date().timeIntervalSince1970 - Double(started_ms) / 1000.0)
    }
}

/// Account-wide Claude subscription usage (mirrors sbxw's `UsageInfo`), from
/// `GET /api/usage`. Percentages are 0–100; nil when unknown (API-key auth, or
/// before a session's first API response).
struct UsageInfo: Codable, Equatable {
    let five_hour_pct: Double?
    let seven_day_pct: Double?
    let five_hour_resets_at: Int?
    let seven_day_resets_at: Int?
    let updated_ms: UInt64

    /// True once at least one window has been reported.
    var hasData: Bool { five_hour_pct != nil || seven_day_pct != nil }
}

/// One entry of the `GET /api/sandboxes` list (mirrors sbxw's `SandboxItem`).
/// Lets running sandboxes appear in the island before a live session exists.
struct SandboxItem: Codable {
    let name: String
    let agent: String
    let status: String

    var isRunning: Bool { status.lowercased().contains("running") }
}

/// Where the sbxw daemon lives. Persisted in `UserDefaults`.
enum Config {
    private static let key = "sbxwBaseURL"

    /// The daemon URL as the *user* knows it — the same address their browser
    /// tab is on. Used for browser-facing work (deep links, matching the open
    /// tab in `focusExistingTab`), never for our own HTTP calls: see `apiBaseURL`.
    static var baseURL: String {
        get { UserDefaults.standard.string(forKey: key) ?? "http://sbxw.localhost:7681" }
        set { UserDefaults.standard.set(newValue, forKey: key) }
    }

    /// `baseURL` with a `*.localhost` host swapped for the loopback literal.
    ///
    /// App Transport Security blocks plain-HTTP loads to a *dotted* hostname
    /// like `sbxw.localhost`, and does so even with `NSAllowsArbitraryLoads` in
    /// the bundle's Info.plist — every request fails with `NSURLErrorDomain`
    /// -1022 and the island sits forever on "Waiting for sbxw…". Numeric
    /// loopback addresses are outside ATS's remit, so we dial 127.0.0.1
    /// instead. Safe by definition: RFC 6761 reserves `.localhost` to resolve
    /// to loopback, which is exactly what sbxw's /etc/hosts entry does.
    ///
    /// Only the *transport* changes — `baseURL` still names the host for
    /// anything the browser sees, so tab matching keeps working.
    static var apiBaseURL: String {
        guard var parts = URLComponents(string: baseURL),
              let host = parts.host, host.hasSuffix("localhost")
        else { return baseURL }
        parts.host = "127.0.0.1"
        return parts.string ?? baseURL
    }

    static func url(_ path: String) -> URL? {
        URL(string: apiBaseURL + path)
    }

    /// Deep-link that focuses `sandbox` in the browser UI (see the
    /// `#sandbox=` handler in `assets/index.html`).
    static func deepLink(sandbox: String) -> URL? {
        let escaped = sandbox.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? sandbox
        return URL(string: baseURL + "/#sandbox=" + escaped)
    }
}

/// Compact elapsed label like "3s", "27m", "5h", "2d".
func elapsedLabel(_ seconds: TimeInterval) -> String {
    let s = Int(seconds)
    if s < 60 { return "\(s)s" }
    if s < 3600 { return "\(s / 60)m" }
    if s < 86400 { return "\(s / 3600)h" }
    return "\(s / 86400)d"
}
