import Foundation
import Combine

/// Keeps the island in sync with the sbxw daemon, merging two sources:
///
///  1. **Live sessions** — seeded from `GET /api/sessions`, then followed on
///     the `GET /api/events` SSE stream. Carry the rich state (agent, activity,
///     last input, pending question) of an attached PTY.
///  2. **Running sandboxes** — polled from `GET /api/sandboxes`, shown as an
///     `idle` baseline until a live session upgrades them in place.
@MainActor
final class SessionStore: ObservableObject {
    @Published private(set) var sessions: [SessionInfo] = []
    @Published private(set) var connected = false
    /// Latest account-wide subscription usage (polled from `/api/usage`).
    @Published private(set) var usage: UsageInfo?
    /// Sessions whose "waiting for input" the user has dismissed in the island,
    /// mapped to *what* they dismissed (see `ackKey`). Purely local — the daemon
    /// still shows the session waiting; this only silences the island.
    ///
    /// Keyed by content rather than just by session id because Claude Code's
    /// hooks make the state flicker while a prompt sits unanswered: an
    /// `AskUserQuestion` raises `attention`, then the turn ends and `Stop`
    /// drops the session to `idle`, then a `Notification` nudge raises
    /// `attention` again. Tying the dismissal to "is it still in attention"
    /// meant that blip re-armed the notification and the ✕ came back on a
    /// prompt the user had already dismissed. An acknowledgement now stands
    /// until the session asks something genuinely different.
    @Published private(set) var acknowledged: [String: Ack] = [:]

    /// One dismissal: what it was about, and which turn it belonged to.
    struct Ack: Equatable {
        /// `ackKey` at dismissal time — empty for a bare waiting turn.
        let key: String
        /// The session's `last_input` then. Once the user submits again the turn
        /// is new, so the dismissal no longer applies however quiet the session
        /// looks.
        let lastInput: String?
    }

    /// Fired when a live session genuinely changes state (not on the repeated
    /// "working" keep-alives). Used to pop notch toasts / prompts.
    var onTransition: ((SessionInfo) -> Void)?

    private var live: [String: SessionInfo] = [:]
    private var running: [SandboxItem] = []
    /// Names of sandboxes `sbx ls` currently reports as running — the authority
    /// on what still exists.
    private var runningNames: Set<String> = []
    /// Set once the first `/api/sandboxes` poll succeeds, so we don't prune live
    /// sessions before we know what's actually running.
    private var hasPolled = false
    /// When each live session first appeared, so a just-created session gets a
    /// grace window before the running-sandbox list is expected to include it.
    private var liveSince: [String: Date] = [:]
    /// A live session is pruned only after its sandbox has been absent from the
    /// running list for this long — covers a killed sandbox that never sent a
    /// `SessionEnd` hook, without flickering out brand-new sessions.
    private let staleGrace: TimeInterval = 8

    private var streamTask: Task<Void, Never>?
    private var pollTask: Task<Void, Never>?
    private var sseRawCount = 0

    var attentionCount: Int { sessions.filter { $0.state == .attention }.count }

    /// Any session waiting for input that the user hasn't dismissed — this is
    /// what drives the menu-bar bell badge and the notch's amber lead.
    var needsAttention: Bool {
        sessions.contains { $0.state == .attention && !isAcknowledged($0) }
    }

    /// What a dismissal is *about*, so it can outlive the state flicker around
    /// an unanswered prompt.
    ///
    /// Empty when the session isn't asking anything right now (idle, working, or
    /// a bare `attention` with no message): those carry nothing new, so they
    /// leave an existing acknowledgement standing rather than re-arming it.
    private func ackKey(_ s: SessionInfo) -> String {
        if !s.promptSteps.isEmpty {
            // Every step, not just the first: two prompts can share a question
            // and differ later on.
            return "q:" + s.promptSteps
                .map { $0.text + "\u{1F}" + $0.options.joined(separator: "\u{1E}") }
                .joined(separator: "\u{1D}")
        }
        // A permission prompt / idle nudge (`Notification`) has no structured
        // question — its message is what the user dismissed.
        if s.state == .attention, let a = s.activity, !a.isEmpty { return "n:" + a }
        return ""
    }

    /// Has the user dismissed what this session is currently asking?
    func isAcknowledged(_ s: SessionInfo) -> Bool {
        guard let ack = acknowledged[s.id] else { return false }
        // A new user turn retires the dismissal outright.
        guard ack.lastInput == s.last_input else { return false }
        let current = ackKey(s)
        return current.isEmpty || current == ack.key
    }

    /// Dismiss a session's current "waiting for input" notification in the island
    /// (the user checked it — via the row's ✕ or by opening the sandbox). No-op
    /// unless it's actually waiting.
    func acknowledge(_ session: SessionInfo) {
        guard session.state == .attention else { return }
        let ack = Ack(key: ackKey(session), lastInput: session.last_input)
        guard acknowledged[session.id] != ack else { return }
        acknowledged[session.id] = ack
        Log.log("acknowledge \(session.id) key=\(ack.key.prefix(60))")
    }

    func start() {
        Log.log("start baseURL=\(Config.baseURL) api=\(Config.apiBaseURL)")
        if streamTask == nil { streamTask = Task { await self.streamLoop() } }
        if pollTask == nil { pollTask = Task { await self.pollLoop() } }
    }

    func stop() {
        streamTask?.cancel(); streamTask = nil
        pollTask?.cancel(); pollTask = nil
        connected = false
    }

    func restart() {
        stop()
        start()
    }

    // MARK: - Interaction (input back to the PTY)

    /// Answer a session's pending prompt: one 1-based option number per step,
    /// in tab order. The daemon replays them as the keystrokes a user would
    /// type, so a multi-question prompt is submitted in one go.
    func answer(_ session: SessionInfo, indices: [Int]) {
        Log.log("answer \(session.id) indices=\(indices)")
        post("/api/answer", body: [
            "sandbox": session.sandbox, "mode": session.mode, "indices": indices,
        ])
    }

    /// Write raw input into a session's PTY.
    func input(_ session: SessionInfo, data: String) {
        post("/api/input", body: [
            "sandbox": session.sandbox, "mode": session.mode, "data": data,
        ])
    }

    private func post(_ path: String, body: [String: Any]) {
        guard let url = Config.url(path) else { return }
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.httpBody = try? JSONSerialization.data(withJSONObject: body)
        Task { _ = try? await URLSession.shared.data(for: req) }
    }

    // MARK: - Merge

    private func rebuild() {
        // Drop live sessions whose sandbox no longer exists. `sbx ls` (via the
        // poll) is the authority; the daemon may still hold a status for a
        // sandbox that was stopped/removed without emitting a `SessionEnd` hook.
        // A startup + freshness grace avoids pruning a session before the poll
        // has confirmed the world, or a just-created one the poll hasn't seen yet.
        if hasPolled {
            let now = Date()
            let stale = live.filter { key, info in
                info.state != .exited
                    && !runningNames.contains(info.sandbox)
                    && now.timeIntervalSince(liveSince[key] ?? now) > staleGrace
            }.map(\.key)
            for key in stale {
                Log.log("prune stale session \(key) (sandbox gone)")
                live.removeValue(forKey: key)
                liveSince.removeValue(forKey: key)
            }
        }

        var out: [SessionInfo] = []
        var seen = Set<String>()
        for (key, info) in live {
            out.append(info)
            seen.insert(key)
        }
        for item in running {
            let key = "\(item.name)::claude"
            if !seen.contains(key) {
                out.append(SessionInfo(
                    sandbox: item.name, mode: "claude", state: .idle,
                    agent: item.agent, started_ms: 0,
                    activity: nil, last_input: nil, question: nil, steps: nil, ts: nil
                ))
            }
        }
        out.sort { ($0.sandbox, $0.mode) < ($1.sandbox, $1.mode) }
        sessions = out

        // Re-arm the notification only when a session asks something *different*
        // from what was dismissed — not merely because it stopped being in
        // `attention` for a tick. Sessions that vanished drop their entry too.
        var keptAcks: [String: Ack] = [:]
        for s in out where isAcknowledged(s) {
            keptAcks[s.id] = acknowledged[s.id]
        }
        if keptAcks != acknowledged { acknowledged = keptAcks }
        let summary = out.map { "\($0.sandbox):\($0.state.rawValue)" }.joined(separator: ", ")
        Log.log("rebuild: \(out.count) rows (live=\(live.count), running=\(running.count)) [\(summary)]")
    }

    private func apply(_ info: SessionInfo) {
        let key = info.id
        let previous = live[key]?.state
        if info.state == .exited {
            live.removeValue(forKey: key)
            liveSince.removeValue(forKey: key)
        } else {
            if live[key] == nil { liveSince[key] = Date() }
            live[key] = info
        }
        rebuild()
        if previous != info.state {
            onTransition?(info)
        }
    }

    // MARK: - HTTP

    /// Fetch a JSON endpoint, returning the body only on a healthy `200` with a
    /// non-empty payload. An empty or non-200 response is normal while the daemon
    /// is restarting or the connection was dropped mid-flight — return `nil` so
    /// callers skip that tick instead of treating it as data corruption (which
    /// is what produced the scary "snapshot DECODE FAIL … Unexpected end of file"
    /// on an empty body).
    private func fetchOK(_ path: String) async -> Data? {
        guard let url = Config.url(path) else { return nil }
        do {
            let (data, response) = try await URLSession.shared.data(from: url)
            if let http = response as? HTTPURLResponse, http.statusCode != 200 {
                Log.log("\(path): HTTP \(http.statusCode) — skipping")
                return nil
            }
            if data.isEmpty {
                Log.log("\(path): empty body — skipping (daemon restarting?)")
                return nil
            }
            return data
        } catch {
            Log.log("\(path): fetch failed — \(error.localizedDescription)")
            return nil
        }
    }

    // MARK: - Running sandboxes poll

    private func pollLoop() async {
        var lastNames = ""
        while !Task.isCancelled {
            if let data = await fetchOK("/api/sandboxes"),
                let items = try? JSONDecoder().decode([SandboxItem].self, from: data) {
                running = items.filter { $0.isRunning }
                runningNames = Set(running.map { $0.name })
                hasPolled = true
                let names = running.map { $0.name }.joined(separator: ",")
                if names != lastNames {
                    Log.log("poll: \(items.count) sandboxes, running=[\(names)]")
                    lastNames = names
                }
                rebuild()
            }
            // Subscription usage (5h / weekly %). Slow-changing; the same 3 s
            // cadence is plenty. Keep the last value if a poll fails.
            if let data = await fetchOK("/api/usage"),
                let u = try? JSONDecoder().decode(UsageInfo.self, from: data) {
                if u.hasData { usage = u }
            }
            try? await Task.sleep(nanoseconds: 3 * 1_000_000_000)
        }
    }

    // MARK: - Live event stream

    private func streamLoop() async {
        var backoffSeconds: UInt64 = 1
        while !Task.isCancelled {
            await loadSnapshot()
            do {
                try await stream()
                backoffSeconds = 1
            } catch {
                connected = false
                Log.log("SSE error: \(error)")
            }
            if Task.isCancelled { break }
            try? await Task.sleep(nanoseconds: backoffSeconds * 1_000_000_000)
            backoffSeconds = min(backoffSeconds * 2, 15)
        }
    }

    private func loadSnapshot() async {
        guard let data = await fetchOK("/api/sessions") else { return }
        do {
            let snaps = try JSONDecoder().decode([SessionInfo].self, from: data)
            live = Dictionary(snaps.map { ($0.id, $0) }, uniquingKeysWith: { _, new in new })
            // Track first-seen times for the stale-prune grace: keep existing
            // ones, stamp new keys now, forget keys no longer present.
            let now = Date()
            for key in live.keys where liveSince[key] == nil { liveSince[key] = now }
            liveSince = liveSince.filter { live[$0.key] != nil }
            Log.log("snapshot ok: \(snaps.count) sessions")
            rebuild()
        } catch {
            let raw = String(data: data, encoding: .utf8) ?? "<binary>"
            Log.log("snapshot DECODE FAIL: \(error) — raw: \(raw.prefix(500))")
        }
    }

    /// Follow `/api/events` via a delegate-based SSE client (reliable streaming),
    /// suspending until the connection closes or the task is cancelled.
    private func stream() async throws {
        guard let url = Config.url("/api/events") else { return }
        let client = SSEClient()
        sseRawCount = 0

        defer { client.disconnect() }
        try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
                client.onOpen = {
                    DispatchQueue.main.async {
                        MainActor.assumeIsolated {
                            self.connected = true
                            Log.log("SSE connected")
                        }
                    }
                }
                client.onRawLine = { line in
                    DispatchQueue.main.async {
                        MainActor.assumeIsolated { self.logRaw(line) }
                    }
                }
                client.onEvent = { json in
                    DispatchQueue.main.async {
                        MainActor.assumeIsolated { self.applyRawEvent(json) }
                    }
                }
                client.onClose = { error in
                    DispatchQueue.main.async {
                        MainActor.assumeIsolated { self.connected = false }
                    }
                    if let error, (error as NSError).code != NSURLErrorCancelled {
                        cont.resume(throwing: error)
                    } else {
                        cont.resume()
                    }
                }
                client.connect(url: url)
            }
        } onCancel: {
            client.disconnect()
        }
        Log.log("SSE stream ended")
    }

    private func logRaw(_ line: String) {
        sseRawCount += 1
        if sseRawCount <= 30 || line.isEmpty || line.hasPrefix("data:") {
            // Long enough that a prompt payload's `steps` array is visible: at
            // 160 chars it was cut off right where the interesting part starts.
            Log.log("SSE raw#\(sseRawCount): \(line.isEmpty ? "<blank>" : String(line.prefix(500)))")
        }
    }

    private func applyRawEvent(_ json: String) {
        guard let data = json.data(using: .utf8) else { return }
        if let info = try? JSONDecoder().decode(SessionInfo.self, from: data) {
            // `nil` distinguishes a daemon that predates multi-step prompts
            // (no `steps` key at all, so the island falls back to `question`)
            // from one that really did send a single-question prompt.
            let steps = info.steps.map { "\($0.count)" } ?? "nil"
            Log.log("event \(info.id) state=\(info.state.rawValue) steps=\(steps)")
            apply(info)
        } else {
            Log.log("SSE decode FAIL: \(json.prefix(300))")
        }
    }
}
