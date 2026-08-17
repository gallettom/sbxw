import Foundation
import Combine

/// Keeps the island in sync with the daemon's cross-sandbox information requests
/// (`/api/relay`), and carries the human's three decisions back: route it to a
/// sandbox, release the answer, or refuse.
///
/// Kept apart from `SessionStore` on purpose. That store is about sessions —
/// their state, what they are asking, whether sbxw can type into them — and it
/// merges two sources to say so. A relay request is not a session: it belongs to
/// two of them at once and outlives either. Folding it in would have meant a
/// second identity space inside a store whose whole job is reconciling the first.
///
/// Seeded from `GET /api/relay` and followed on `GET /api/relay/events`, with
/// the same reconnect backoff as the session stream. State lives in the daemon;
/// nothing here is authoritative, which is what makes a dropped connection cost
/// nothing but a re-seed.
@MainActor
final class RelayStore: ObservableObject {
    @Published private(set) var requests: [RelayRequest] = []

    /// Requests the user has waved off in the island, by id → the state they
    /// waved off.
    ///
    /// Keyed by *state* rather than just by id for the same reason
    /// `SessionStore.acknowledged` is keyed by content: dismissing a question
    /// means "not now", not "never". When the request moves on — the sandbox
    /// answered, or delivery failed and it fell back to `pending` — that is news
    /// the dismissal never covered, and the card is owed another appearance.
    @Published private(set) var dismissed: [String: RelayState] = [:]

    /// Requests with an action in flight, so a card can disable its buttons
    /// instead of letting a second click race the first.
    @Published private(set) var acting: Set<String> = []

    /// Fired when a request first needs the human — what the notch turns into a
    /// card. Only for a genuine arrival: a request that merely ticked (a note
    /// changed, a re-render) must not re-open a card the user closed.
    var onArrival: ((RelayRequest) -> Void)?

    private var streamTask: Task<Void, Never>?
    /// Every id seen so far in a state that wanted the human, so `onArrival`
    /// fires once per *turn* rather than on every event about the same request.
    private var announced: Set<String> = []

    // MARK: - Derived

    /// Open requests, oldest first — what the island's list banner counts.
    var open: [RelayRequest] { requests.filter(\.isOpen) }

    /// Requests waiting on the human and not waved off: what the notch surfaces,
    /// and what the banner offers a way back to.
    ///
    /// Excludes a request with an action in flight (`acting`), which matters at
    /// one precise moment: `route`/`approve`/`deny` fire the POST and mark the
    /// id busy *synchronously*, but the request's own `state` only changes once
    /// the daemon's SSE broadcast comes back — a round trip, not instant. A
    /// caller that acts on a request and then immediately asks "what needs me
    /// now?" (`NotchController.collapseAndSurfaceNext`, right after routing)
    /// would otherwise find the very request it just acted on, still reading
    /// `.pending` locally, and hand it straight back out — reopening the same
    /// card with live buttons a second click could fire for real. `acting`
    /// closes that window without waiting on the network.
    var pendingForYou: [RelayRequest] {
        requests.filter { $0.needsYou && !isDismissed($0) && !acting.contains($0.id) }
    }

    /// Has the user waved this request off in the state it is currently in?
    func isDismissed(_ req: RelayRequest) -> Bool {
        dismissed[req.id] == req.state
    }

    /// Take a request off the notch without answering it — the island's "later".
    /// The daemon still holds it, and the asking agent still waits; this only
    /// stops the island from occupying the menu bar with it.
    func dismiss(_ req: RelayRequest) {
        guard dismissed[req.id] != req.state else { return }
        dismissed[req.id] = req.state
        Log.log("relay dismiss \(req.id) state=\(req.state.rawValue)")
    }

    /// Sandboxes that could answer `req`: running, and not the one that asked.
    ///
    /// Taken from the session list rather than polled again — `SessionStore`
    /// already carries a row per running sandbox, and a second poll would be a
    /// second answer to the same question.
    func candidates(for req: RelayRequest, sessions: [SessionInfo]) -> [String] {
        var seen = Set<String>()
        var out: [String] = []
        for s in sessions where s.sandbox != req.from && s.state != .exited {
            if seen.insert(s.sandbox).inserted { out.append(s.sandbox) }
        }
        return out
    }

    // MARK: - Lifecycle

    func start() {
        if streamTask == nil { streamTask = Task { await self.streamLoop() } }
    }

    func stop() {
        streamTask?.cancel()
        streamTask = nil
    }

    func restart() {
        stop()
        start()
    }

    // MARK: - The human's three decisions

    /// Send the question to `sandbox`. The daemon types it into that sandbox's
    /// agent; nothing comes back to the asker until it is approved.
    func route(_ req: RelayRequest, to sandbox: String) {
        act(req, path: "/api/relay/\(escaped(req.id))/route", body: ["to": sandbox],
            what: "route → \(sandbox)")
    }

    /// Release the held answer to the sandbox that asked.
    ///
    /// Sent as it stands: the island has no editor, so a card only offers this
    /// when it could show the answer in full (see `RelayCard`). Approving text
    /// you have not read is the one thing this must not make easy.
    func approve(_ req: RelayRequest) {
        act(req, path: "/api/relay/\(escaped(req.id))/approve", body: [:], what: "approve")
    }

    /// Refuse: nothing is released, now or later, and the asking agent is told
    /// not to re-send it.
    func deny(_ req: RelayRequest) {
        act(req, path: "/api/relay/\(escaped(req.id))/deny", body: [:], what: "deny")
    }

    private func escaped(_ id: String) -> String {
        id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? id
    }

    /// One shape for all three: mark the request busy, POST, and let the SSE
    /// stream deliver the outcome. The response body is only read to log a
    /// refusal — the daemon's broadcast is what the UI actually follows, so a
    /// success needs nothing done to it here.
    private func act(_ req: RelayRequest, path: String, body: [String: Any], what: String) {
        guard !acting.contains(req.id) else { return }
        guard let request = Config.jsonRequest(path, body: body) else {
            Log.log("relay \(what) \(req.id): bad daemon URL")
            return
        }
        acting.insert(req.id)
        Log.log("relay \(what) \(req.id)")
        Task { [weak self] in
            do {
                let (data, _) = try await URLSession.shared.data(for: request)
                let obj = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
                if obj?["ok"] as? Bool != true {
                    let msg = obj?["error"] as? String ?? "refused"
                    Log.log("relay \(what) \(req.id) failed: \(msg)")
                }
            } catch {
                Log.log("relay \(what) \(req.id) error: \(error.localizedDescription)")
            }
            // Released on both paths — there is no early return above, so this
            // one line is the whole cleanup. The card re-enables either way: a
            // refusal that left the request busy forever would be a dead card.
            self?.acting.remove(req.id)
        }
    }

    // MARK: - Ingest

    private func apply(_ req: RelayRequest) {
        var next = requests.filter { $0.id != req.id }
        // Settled requests leave the list: the island has no history view, and
        // one that lingered would keep padding the banner's count.
        if req.isOpen {
            next.append(req)
            next.sort { ($0.created_ms, $0.id) < ($1.created_ms, $1.id) }
        }
        requests = next

        if !req.isOpen {
            dismissed.removeValue(forKey: req.id)
            announced.remove(req.id)
            return
        }
        // A dismissal only covered the state it was made in, so a request that
        // moved on is new again — to `announced` as well, which is what lets the
        // answer to a routed question raise its own card.
        if dismissed[req.id] != nil && dismissed[req.id] != req.state {
            dismissed.removeValue(forKey: req.id)
        }
        if req.needsYou {
            if announced.insert(req.id).inserted {
                onArrival?(req)
            }
        } else {
            // `routed`: out with a sandbox. Clearing this here is what makes the
            // answer's arrival count as news rather than as an update to
            // something already announced.
            announced.remove(req.id)
        }
    }

    private func seed(_ list: [RelayRequest]) {
        let live = list.filter(\.isOpen)
        requests = live.sorted { ($0.created_ms, $0.id) < ($1.created_ms, $1.id) }
        // Forget bookkeeping for requests the daemon no longer has.
        let ids = Set(live.map(\.id))
        dismissed = dismissed.filter { ids.contains($0.key) }
        announced.formIntersection(ids)

        // A seed announces what it has never announced before — and nothing
        // else. That distinction is the whole design: `announced` survives
        // reconnects, so a blink re-seeds the same ids and stays quiet, while a
        // request that appeared *while the stream was down* still gets its card.
        //
        // Which is not a hypothetical. A daemon predating `/api/relay/events`
        // serves `/api/relay` perfectly well, so the reconnect loop re-seeds
        // every few seconds and the island degrades onto this path: cards keep
        // arriving, just a little later. Announcing only from the event stream
        // left that daemon with a populated banner and a notch that never once
        // opened — silence that looked exactly like a broken feature.
        let fresh = live.filter { $0.needsYou && !announced.contains($0.id) && !isDismissed($0) }
        for req in fresh { announced.insert(req.id) }
        Log.log("relay seed: \(live.count) open, \(fresh.count) new to announce")
        for req in fresh { onArrival?(req) }
    }

    // MARK: - HTTP

    private func streamLoop() async {
        var backoffSeconds: UInt64 = 1
        // Logged once rather than every retry: a daemon without the endpoint
        // never grows one, and a line per second would bury everything else.
        var reportedMissing = false
        while !Task.isCancelled {
            await loadSnapshot()
            do {
                let opened = try await stream()
                if opened {
                    backoffSeconds = 1
                    reportedMissing = false
                } else {
                    // The response was never a 200 — on a daemon older than the
                    // island, that is `/api/relay/events` simply not existing.
                    // Backing off matters here: without it this retries once a
                    // second forever against a route that will never appear.
                    if !reportedMissing {
                        Log.log(
                            "relay: /api/relay/events did not open — the daemon may predate it. "
                                + "Restart `sbxw web` to get live relay cards; falling back to "
                                + "re-reading /api/relay on each retry."
                        )
                        reportedMissing = true
                    }
                    backoffSeconds = min(max(backoffSeconds * 2, 2), 15)
                }
            } catch {
                Log.log("relay SSE error: \(error)")
                backoffSeconds = min(backoffSeconds * 2, 15)
            }
            if Task.isCancelled { break }
            try? await Task.sleep(nanoseconds: backoffSeconds * 1_000_000_000)
        }
    }

    private func loadSnapshot() async {
        guard let url = Config.url("/api/relay") else { return }
        do {
            let (data, response) = try await URLSession.shared.data(from: url)
            if let http = response as? HTTPURLResponse, http.statusCode != 200 {
                // A daemon predating the relay simply has no such route. Not an
                // error worth retrying loudly: the island keeps working, it just
                // has no requests to show.
                Log.log("/api/relay: HTTP \(http.statusCode) — no relay on this daemon?")
                return
            }
            guard !data.isEmpty else { return }
            let payload = try JSONDecoder().decode(RelayList.self, from: data)
            seed(payload.requests)
        } catch {
            Log.log("/api/relay: \(error.localizedDescription)")
        }
    }

    /// Envelope of `GET /api/relay`.
    private struct RelayList: Codable {
        let requests: [RelayRequest]
    }

    /// Follow `/api/relay/events`. Each payload is one whole request — the
    /// daemon sends the full record on every transition rather than a delta, so
    /// a client that missed one still ends up right.
    ///
    /// Returns whether the stream ever opened. `SSEClient` cancels anything that
    /// isn't a 200, and a cancellation is indistinguishable from a clean close
    /// at the continuation — so "did we get a 200" is the only way to tell a
    /// dropped connection from a route that isn't there.
    @discardableResult
    private func stream() async throws -> Bool {
        guard let url = Config.url("/api/relay/events") else { return false }
        let client = SSEClient()
        var opened = false

        defer { client.disconnect() }
        try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
                client.onOpen = { opened = true }
                client.onEvent = { json in
                    guard let data = json.data(using: .utf8) else { return }
                    guard let req = try? JSONDecoder().decode(RelayRequest.self, from: data) else {
                        Log.log("relay SSE decode FAIL: \(json.prefix(300))")
                        return
                    }
                    DispatchQueue.main.async {
                        MainActor.assumeIsolated {
                            Log.log("relay event \(req.id) state=\(req.state.rawValue)")
                            self.apply(req)
                        }
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
        Log.log("relay stream ended (opened=\(opened))")
        return opened
    }
}
