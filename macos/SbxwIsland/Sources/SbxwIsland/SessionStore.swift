import Foundation
import Combine

/// Where an island composer's message goes.
///
/// The island has more than one composer now — one per open row accordion, plus
/// the "New chat" strip at the bottom — and they mean different things, so the
/// target is explicit rather than "the chat sandbox".
enum ChatTarget: Equatable, Hashable {
    /// An existing sandbox: type into the agent already living there.
    case sandbox(String)
    /// A brand-new throwaway chat sandbox, named by the daemon
    /// (`ephemeral-chat`, then `ephemeral-chat-2`, `-3`, …).
    case newChat

    /// Key this target's progress is filed under in `SessionStore.chatPush`.
    /// The empty string is safe for `newChat`: a sandbox name never is (the
    /// daemon rejects it), so the two can't collide.
    var key: String {
        switch self {
        case .sandbox(let name): return name
        case .newChat: return ""
        }
    }

    /// How the target reads in a log line.
    var label: String {
        switch self {
        case .sandbox(let name): return name
        case .newChat: return "a new chat"
        }
    }
}

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

    /// Progress of a composer's last chat push.
    enum ChatPushState: Equatable {
        case idle
        case sending
        case failed(String)
    }

    /// Push progress per target (see `ChatTarget.key`). Keyed rather than
    /// single-valued because every open row carries its own composer: one shared
    /// state would put another row's spinner — or another row's error — in yours.
    @Published private(set) var chatPush: [String: ChatPushState] = [:]

    /// Fired when a live session genuinely changes state (not on the repeated
    /// "working" keep-alives). Used to pop notch toasts / prompts.
    var onTransition: ((SessionInfo) -> Void)?

    /// Fired when a browser tab reports it is now showing a sandbox's terminal
    /// (see `/api/watch-events`). The notch uses it to take down a card the
    /// user walked away from the island to go and read.
    var onWatched: ((String) -> Void)?

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
    private var watchTask: Task<Void, Never>?
    private var sseRawCount = 0

    var attentionCount: Int { sessions.filter { $0.state == .attention }.count }

    /// Any session waiting for input that the user hasn't dismissed — this is
    /// what drives the menu-bar bell badge and the notch's amber lead.
    var needsAttention: Bool {
        sessions.contains { $0.state == .attention && !isAcknowledged($0) }
    }

    /// The session the collapsed notch bubble speaks for, by how much it wants
    /// you: an explicit prompt first, then a turn awaiting your reply, then a
    /// working one. Without the middle case the collapsed notch showed nothing at
    /// all while Claude sat waiting on an inline question.
    ///
    /// Nil means the bubble draws nothing (a hairline hover strip), which is also
    /// what tells the notch panel whether there is anything up there to click —
    /// hence a store property rather than the pill's own business (see
    /// `SummaryPill` and `NotchController.updateClickThrough`).
    var summaryLead: SessionInfo? {
        sessions.first { $0.state == .attention && !isAcknowledged($0) }
            // Also skipped once dismissed: a prompt the user waved off ends its
            // turn (`Stop` → idle with the last input still set), which reads as
            // "waiting for your reply" and put the very session they'd just
            // dismissed straight back on the notch in teal.
            ?? sessions.first { $0.awaitingReply && !isAcknowledged($0) }
            ?? sessions.first { $0.state == .working }
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
    ///
    /// "Waiting" covers a turn that simply *ended* as well as an explicit
    /// `attention`: a session that answered keeps the collapsed notch on screen
    /// (`SummaryPill.lead`), and while this guard was `attention`-only there was
    /// no way to retire it — the pill outlived every reply for good. An idle
    /// session's `ackKey` is empty, so the dismissal stands until you submit
    /// again, which retires it through `isAcknowledged`.
    /// Sessions a dismissal would actually do something to — what "dismiss all"
    /// acts on, and what decides whether offering it makes sense at all.
    var dismissable: [SessionInfo] {
        sessions.filter { ($0.state == .attention || $0.awaitingReply) && !isAcknowledged($0) }
    }

    /// Dismiss everything that is currently asking for something. Clearing the
    /// notch in one gesture instead of one ✕ per row.
    func acknowledgeAll() {
        let targets = dismissable
        guard !targets.isEmpty else { return }
        Log.log("acknowledge all (\(targets.count))")
        for s in targets { acknowledge(s) }
    }

    func acknowledge(_ session: SessionInfo) {
        guard session.state == .attention || session.awaitingReply else { return }
        let ack = Ack(key: ackKey(session), lastInput: session.last_input)
        guard acknowledged[session.id] != ack else { return }
        acknowledged[session.id] = ack
        Log.log("acknowledge \(session.id) key=\(ack.key.prefix(60))")
    }

    /// A browser tab is now showing this sandbox's terminal (`/api/watch-events`).
    ///
    /// Going to the tty to see what is happening *is* checking the notification,
    /// so it retires exactly as the row's ✕ would — the island should not still
    /// be pointing at a question the user is already reading in full. Every
    /// session of that sandbox, since the sandbox is what the user opened, and
    /// `acknowledge` no-ops on the ones that aren't waiting for anything.
    ///
    /// Nothing about this is permanent: the acknowledgement is keyed on what was
    /// being asked, so a genuinely new question raises the island again even
    /// though the tab never stopped watching.
    func acknowledgeWatched(_ sandbox: String) {
        let targets = sessions.filter { $0.sandbox == sandbox }
        if !targets.isEmpty {
            Log.log("watched \(sandbox) — acknowledging \(targets.count) session(s)")
            for s in targets { acknowledge(s) }
        }
        onWatched?(sandbox)
    }

    func start() {
        Log.log("start baseURL=\(Config.baseURL) api=\(Config.apiBaseURL)")
        if streamTask == nil { streamTask = Task { await self.streamLoop() } }
        if pollTask == nil { pollTask = Task { await self.pollLoop() } }
        if watchTask == nil { watchTask = Task { await self.watchLoop() } }
    }

    func stop() {
        streamTask?.cancel(); streamTask = nil
        pollTask?.cancel(); pollTask = nil
        watchTask?.cancel(); watchTask = nil
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

    /// Progress of `target`'s last push — `.idle` for one never pushed to.
    func chatState(_ target: ChatTarget) -> ChatPushState {
        chatPush[target.key] ?? .idle
    }

    /// Submit a message to a chat agent.
    ///
    /// One call whether or not anything exists yet: the daemon creates the
    /// sandbox if needed, attaches the agent and waits for its TUI before typing
    /// (see `/api/chat/push`). Typing into a sandbox that is already up returns
    /// in a blink; a `.newChat` has to boot one first, hence `chatPush` for the
    /// composer to show progress, and a timeout far past URLSession's default
    /// patience.
    func pushChat(_ text: String, to target: ChatTarget) {
        let message = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !message.isEmpty, chatState(target) != .sending else { return }
        guard let url = Config.url("/api/chat/push") else {
            chatPush[target.key] = .failed("bad daemon URL")
            return
        }
        var body: [String: Any] = ["text": message]
        switch target {
        case .sandbox(let name): body["name"] = name
        // The daemon picks the name — it is the one that knows which
        // `ephemeral-chat-N` is free.
        case .newChat: body["fresh"] = true
        }
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.httpBody = try? JSONSerialization.data(withJSONObject: body)
        req.timeoutInterval = 180

        let key = target.key
        chatPush[key] = .sending
        Log.log("chat push → \(target.label): \(message.prefix(60))")
        Task { [weak self] in
            do {
                let (data, _) = try await URLSession.shared.data(for: req)
                let obj = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
                if obj?["ok"] as? Bool == true {
                    Log.log("chat push ok → \(obj?["name"] as? String ?? "?")")
                    self?.chatPush[key] = .idle
                } else {
                    let msg = obj?["error"] as? String ?? "chat push failed"
                    Log.log("chat push failed: \(msg)")
                    self?.chatPush[key] = .failed(msg)
                }
            } catch {
                Log.log("chat push error: \(error.localizedDescription)")
                self?.chatPush[key] = .failed(error.localizedDescription)
            }
        }
    }

    /// Clear a failed push so the composer stops showing the error.
    func clearChatPushError(_ target: ChatTarget) {
        if case .failed = chatState(target) { chatPush[target.key] = .idle }
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
                    activity: nil, last_input: nil, question: nil, steps: nil,
                    reply: nil, ts: nil
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

    // MARK: - "The user went to look" stream

    /// Follow `/api/watch-events` for as long as the store is running, with the
    /// same backoff as the session stream. Kept apart from `streamLoop` on
    /// purpose: this one must not touch `connected`, which reports whether the
    /// island is receiving *session* state — the thing the UI falls back on when
    /// the daemon goes away.
    private func watchLoop() async {
        var backoffSeconds: UInt64 = 1
        while !Task.isCancelled {
            do {
                try await watchStream()
                backoffSeconds = 1
            } catch {
                Log.log("watch SSE error: \(error)")
            }
            if Task.isCancelled { break }
            try? await Task.sleep(nanoseconds: backoffSeconds * 1_000_000_000)
            backoffSeconds = min(backoffSeconds * 2, 15)
        }
    }

    /// Each event's payload is a bare sandbox name, not JSON — the same shape as
    /// `/api/focus-events`, travelling the other way.
    private func watchStream() async throws {
        guard let url = Config.url("/api/watch-events") else { return }
        let client = SSEClient()

        defer { client.disconnect() }
        try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
                client.onEvent = { name in
                    let sandbox = name.trimmingCharacters(in: .whitespacesAndNewlines)
                    guard !sandbox.isEmpty else { return }
                    DispatchQueue.main.async {
                        MainActor.assumeIsolated { self.acknowledgeWatched(sandbox) }
                    }
                }
                client.onClose = { error in
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
        Log.log("watch stream ended")
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
