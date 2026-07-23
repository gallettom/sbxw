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
    let question: Question?
    let ts: UInt64?

    var id: String { "\(sandbox)::\(mode)" }

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

    static var baseURL: String {
        get { UserDefaults.standard.string(forKey: key) ?? "http://sbxw.localhost:7681" }
        set { UserDefaults.standard.set(newValue, forKey: key) }
    }

    static func url(_ path: String) -> URL? {
        URL(string: baseURL + path)
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
