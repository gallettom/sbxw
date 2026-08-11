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
    /// Claude Code's own session id. Absent on an older daemon, and on a
    /// sandbox row synthesised from the running-sandboxes poll.
    var session_id: String? = nil
    /// The directory this session runs in — what tells two agents of the same
    /// sandbox apart on screen.
    var cwd: String? = nil
    /// Whose session this is — `tty` (the terminal sbxw attached), `remote` (a
    /// client driving it: Claude Desktop, an editor, a shell on `<name>.sbx`)
    /// or `unknown`. Established inside the sandbox, not guessed.
    var origin: String? = nil
    /// Whether the daemon can answer this session's prompt by typing into a
    /// terminal it owns. Absent on an older daemon, which only ever reported
    /// one session per sandbox and could always answer it.
    var answerable: Bool? = nil
    let ts: UInt64?

    /// One container can hold several agents — sbxw's own, plus any driven by
    /// an attached client (Claude Desktop, an editor). They are distinct
    /// sessions and get a row each, so the session id is part of the identity.
    var id: String { "\(sandbox)::\(mode)::\(session_id ?? "")" }

    /// Identity of the *sandbox pane*, without the session. What the running
    /// sandboxes poll and the focus/watch plumbing address.
    var sandboxKey: String { "\(sandbox)::\(mode)" }

    /// Can the island answer this prompt for you?
    ///
    /// Defaults to true so a daemon that predates the field behaves as it
    /// always did: it reported one session per sandbox, and that one was always
    /// sbxw's own. The daemon enforces this too — the island not offering the
    /// button is the courtesy, the refusal is the guarantee.
    var canAnswer: Bool { answerable ?? true }

    /// Is this session driven by a client rather than by sbxw — Claude Desktop,
    /// an editor, a shell on `<name>.sbx`? Only ever true on evidence: an origin
    /// the sandbox could not establish reads as "not remote" rather than as a
    /// suspicion.
    ///
    /// `ssh` is the name this carried before it was widened: Claude Desktop
    /// turned out not to use SSH at all, it runs its own server in the
    /// container. The app and the daemon ship as separate binaries and update
    /// independently, so a version of one that predates the rename must not
    /// leave the other showing nothing at all — which is exactly what an
    /// unrecognised value does, silently.
    var isRemote: Bool { origin == "remote" || origin == "ssh" }

    /// The badge a row wears when its sandbox holds more than one agent, so the
    /// same name is not printed twice with nothing to tell them apart.
    ///
    /// Says *whose session it is*, which is the question being asked — a
    /// directory name could not answer it, and a folder called `Desktop`
    /// actively answered the wrong one.
    /// The daemon's `remote` is deliberately broader than this label: it also
    /// covers a plain `ssh <name>.sbx` from a terminal or an editor. "Desktop"
    /// names the case that actually occurs, and reads as the thing you would
    /// switch to; the wire value stays accurate underneath.
    var originLabel: String? {
        switch origin {
        case "remote", "ssh": return "Desktop"
        case "tty": return "tty"
        // `unknown` and a missing origin genuinely have nothing to say. Any
        // *other* value comes from a daemon newer than this app: show it as it
        // came rather than swallowing the row's only distinguishing mark.
        case let other?: return other == "unknown" ? nil : other
        case nil: return nil
        }
    }

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

    /// Claude's reply, trimmed — nil when there isn't one. What a row's open
    /// accordion prints, in full: it is the only surface with room for the whole
    /// answer, and the row drops its one-line caption while it is open, so there
    /// is nothing to trim against.
    var replyText: String? {
        guard let reply else { return nil }
        let trimmed = reply.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }

    /// The prompt as a list of steps — one entry for a single question, empty
    /// when nothing is pending. Falls back to `question` so an older daemon
    /// still produces a (single-step) card.
    var promptSteps: [Question] {
        if let steps, !steps.isEmpty { return steps }
        return question.map { [$0] } ?? []
    }

    /// `cwd` shortened for a row: the sandbox's home collapses to `~`, and a
    /// long path keeps its last two components.
    ///
    /// A *path*, deliberately, and never the bare last component. A session
    /// sitting in `~/Desktop` would otherwise be badged "Desktop", which reads
    /// as the Claude Desktop client rather than as a directory — and on a
    /// shared sandbox those are exactly the two things a reader is trying to
    /// tell apart, so the one label would answer the wrong question. The badge
    /// carries `originLabel` now; this is the secondary detail, in a tooltip.
    var cwdLabel: String? {
        guard let cwd, !cwd.isEmpty else { return nil }
        // The container's home, not the Mac's: these paths are reported by a
        // hook running inside the sandbox, where the agent user is `agent`.
        var path = cwd
        for home in ["/home/agent", "/root"] {
            if path == home { return "~" }
            if path.hasPrefix(home + "/") {
                path = "~" + String(path.dropFirst(home.count))
                break
            }
        }
        let parts = path.split(separator: "/").map(String.init)
        guard parts.count > 2 else { return path }
        return "…/" + parts.suffix(2).joined(separator: "/")
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

    /// A JSON POST to the daemon, built the one way the island builds them.
    /// Returned as a `let` on purpose: a `Task` closure is `@Sendable`, and
    /// capturing a mutable local in one is an error, so every caller would
    /// otherwise wrap the construction in a closure of its own.
    static func jsonRequest(
        _ path: String,
        body: [String: Any],
        timeout: TimeInterval = 60
    ) -> URLRequest? {
        guard let url = url(path) else { return nil }
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.httpBody = try? JSONSerialization.data(withJSONObject: body)
        req.timeoutInterval = timeout
        return req
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
