import SwiftUI
import AppKit

// MARK: - Cursor

/// Shows the pointing-hand cursor while hovering a control (SwiftUI buttons
/// don't do this automatically on macOS). Balances push/pop with a flag and
/// pops on disappear so a vanishing island never leaves the cursor stuck.
private struct PointerCursor: ViewModifier {
    @State private var pushed = false
    func body(content: Content) -> some View {
        content
            .onHover { inside in
                if inside {
                    if !pushed { NSCursor.pointingHand.push(); pushed = true }
                } else if pushed {
                    NSCursor.pop(); pushed = false
                }
            }
            .onDisappear {
                if pushed { NSCursor.pop(); pushed = false }
            }
    }
}

extension View {
    func pointerCursor() -> some View { modifier(PointerCursor()) }
}

// MARK: - State presentation

/// The calm accent for "it's your turn to reply" — distinct from the amber of
/// an explicit `attention` prompt, so a finished conversational turn reads as
/// an invitation, not an alarm.
let awaitingReplyColor = Color(red: 0.36, green: 0.80, blue: 0.72)

extension SessionState {
    var dotColor: Color {
        switch self {
        case .working: return .blue
        case .attention: return .orange
        case .idle: return .gray
        case .exited: return .gray.opacity(0.4)
        }
    }

    var label: String {
        switch self {
        case .working: return "working…"
        case .attention: return "waiting for input"
        case .idle: return "idle"
        case .exited: return "ended"
        }
    }
}

extension SessionInfo {
    /// Claude finished its turn and the ball is in your court — an idle *live*
    /// session that has already been talked to. This is the only cue for a
    /// free-text question Claude asks inline (e.g. "Question 1: …?"): it fires
    /// no `AskUserQuestion` hook, so the daemon just sees the turn end and the
    /// session go idle. A brand-new sandbox that has never been prompted stays
    /// plainly idle (no `last_input`), so it isn't mislabelled.
    var awaitingReply: Bool {
        state == .idle && started_ms > 0 && (last_input?.isEmpty == false)
    }

    /// Whether this session should be captioned with what Claude *said* rather
    /// than with a status or with your own prompt.
    ///
    /// Having a reply at all is the whole condition — no state test. A stale
    /// answer can't leak through, because the daemon clears `reply` on
    /// `UserPromptSubmit`: a session only carries one if a turn has *ended*
    /// since you last spoke. So an `attention` session showing a reply is one
    /// that answered and is now nudging you about it (Claude Code's idle
    /// notification), which is precisely when the answer is what you want to
    /// read — while a permission prompt, raised mid-turn, has no reply to show
    /// and still surfaces its `activity`.
    ///
    /// A structured question outranks this and is handled ahead of it, since
    /// that text is what you have to act on. Every surface that captions a
    /// session shares this rule — the row, the pill under the notch, the mini
    /// toast — so they can't disagree about the same session.
    var showsReply: Bool {
        replyLead != nil
    }

    /// Dot/accent for a session that wants the human: amber when it explicitly
    /// asked (`attention`), calm teal when it's simply your turn to reply.
    var accentColor: Color {
        if state == .attention { return .orange }
        if awaitingReply { return awaitingReplyColor }
        return state.dotColor
    }
}

/// Small rounded tag, e.g. the agent name.
struct Tag: View {
    let text: String
    var body: some View {
        Text(text)
            .font(.system(size: 9, weight: .medium))
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(Color.white.opacity(0.12))
            .clipShape(Capsule())
            .foregroundStyle(.white.opacity(0.85))
    }
}

// MARK: - Rich session row

/// Escape a string for embedding inside an AppleScript double-quoted literal.
private func escapeAppleScript(_ s: String) -> String {
    s.replacingOccurrences(of: "\\", with: "\\\\")
        .replacingOccurrences(of: "\"", with: "\\\"")
}

/// Find and foreground the browser tab already showing the sbxw UI (its URL
/// starts with the base URL). Returns `true` if such a tab was found and
/// activated. Browsers block programmatic tab focus from the page itself, so
/// this is the only reliable way to bring the *right* tab forward — it does mean
/// the app needs Automation permission for the browser (macOS prompts once).
///
/// Chromium browsers (Chrome, Brave, Edge, …) and Safari use different scripting
/// vocabularies, so we branch on the default browser's bundle id. Call on the
/// main thread (it drives NSWorkspace and NSAppleScript).
func focusExistingTab() -> Bool {
    guard let probe = URL(string: Config.baseURL),
          let appURL = NSWorkspace.shared.urlForApplication(toOpen: probe) else { return false }
    let appName = escapeAppleScript(appURL.deletingPathExtension().lastPathComponent)
    let bundleID = Bundle(url: appURL)?.bundleIdentifier ?? ""
    let base = escapeAppleScript(Config.baseURL)

    let script: String
    if bundleID.hasPrefix("com.apple.Safari") {
        script = """
        tell application "\(appName)"
            activate
            repeat with w in every window
                repeat with t in every tab of w
                    try
                        if (URL of t) starts with "\(base)" then
                            set current tab of w to t
                            set index of w to 1
                            return "ok"
                        end if
                    end try
                end repeat
            end repeat
        end tell
        return "notfound"
        """
    } else {
        script = """
        tell application "\(appName)"
            activate
            repeat with w in every window
                set n to count of tabs of w
                repeat with i from 1 to n
                    try
                        if (URL of tab i of w) starts with "\(base)" then
                            set active tab index of w to i
                            set index of w to 1
                            return "ok"
                        end if
                    end try
                end repeat
            end repeat
        end tell
        return "notfound"
        """
    }

    guard let osa = NSAppleScript(source: script) else { return false }
    var err: NSDictionary?
    let result = osa.executeAndReturnError(&err)
    if let err {
        Log.log("focusExistingTab AppleScript failed: \(err)")
        return false
    }
    return result.stringValue == "ok"
}

/// Bring a sandbox's terminal to the foreground in the browser.
///
/// First we ask every open sbxw tab, over `/api/focus`, to switch to this
/// sandbox in place (SSE) — no navigation, no flicker. Then we bring the
/// browser's existing sbxw tab forward via AppleScript, reusing it instead of
/// spawning a new page. Only if no such tab exists do we cold-start one with the
/// full `#sandbox=` deep link.
func openInBrowser(_ sandbox: String) {
    // Fire-and-forget: switch the pane in any already-open tab.
    if let focusURL = Config.url("/api/focus") {
        let req: URLRequest = {
            var r = URLRequest(url: focusURL)
            r.httpMethod = "POST"
            r.setValue("application/json", forHTTPHeaderField: "Content-Type")
            r.httpBody = try? JSONSerialization.data(withJSONObject: ["sandbox": sandbox])
            r.timeoutInterval = 2
            return r
        }()
        Task { _ = try? await URLSession.shared.data(for: req) }
    }
    // Bring the right tab forward (or open one if none exists). Small delay so
    // the SSE switch above has landed before the tab is revealed.
    DispatchQueue.main.asyncAfter(deadline: .now() + 0.15) {
        if !focusExistingTab(), let url = Config.deepLink(sandbox: sandbox) {
            NSWorkspace.shared.open(url)
        }
    }
}

struct SessionRow: View {
    let session: SessionInfo
    /// Needed by the drawer's composer, which writes into this row's sandbox.
    @ObservedObject var store: SessionStore
    /// Whether the user has already dismissed this session's waiting notification.
    var acknowledged: Bool = false
    /// Whether this row's accordion is open. Owned by the list rather than by
    /// the row (see `IslandView.expanded`), so it survives the store's rebuilds
    /// and the panel can be told that something is open.
    var expanded: Bool = false
    /// Called when the row is tapped (e.g. show the prompt card, or jump).
    var onSelect: (SessionInfo) -> Void = { openInBrowser($0.sandbox) }
    /// Called when the user taps the row's ✕ to dismiss its waiting state. Absent
    /// (no ✕) where dismissal doesn't apply.
    var onDismiss: ((SessionInfo) -> Void)? = nil
    /// Called when the accordion chevron is clicked.
    var onToggle: () -> Void = {}
    /// Told when the drawer's composer takes or gives up keyboard focus — same
    /// contract as `IslandView.onComposerFocus`, and for the same reason: the
    /// notch panel must not retract mid-sentence.
    var onComposerFocus: (Bool) -> Void = { _ in }

    /// Wrapped lines the open drawer gives Claude's reply. Past this the answer
    /// is a document, not a notification — read it in the browser.
    private static let replyLineLimit = 10

    /// Still-waiting *and* not yet dismissed: the row reads as needing you.
    private var waiting: Bool {
        session.state == .attention && !acknowledged
    }

    /// What the open drawer prints above its composer: the reply entire.
    ///
    /// Not "the rest of it after the lead sentence" — the header gives its
    /// caption up while this is on screen (see `showsCaption`), so the answer
    /// lives in exactly one place instead of opening with a truncated echo of
    /// itself.
    private var drawerReply: String? { session.replyText }

    /// Whether the header keeps its one-line caption.
    ///
    /// It doesn't when the drawer below is printing the very reply that caption
    /// summarises: the same sentence twice, once cut off at a word, reads as a
    /// stutter. A caption that is *not* the reply — a pending question, an
    /// activity — is not repeated by the drawer and stays.
    private var showsCaption: Bool {
        !(expanded && canOpen && showsReply)
    }

    private var hasQuestion: Bool {
        session.state == .attention && !session.promptSteps.isEmpty
    }

    /// Whether this row has an accordion at all.
    ///
    /// A Bash pane is a terminal handle, not a conversation: no hooks fire for
    /// it, so it never carries a reply — and `/api/chat/push` always types into
    /// the sandbox's *agent*, so a field here would quietly send your message to
    /// a different session than the row you opened. Nothing to show and nothing
    /// safe to send: no handle.
    private var canOpen: Bool { session.mode != "bash" }

    /// Room the header leaves on the right for the controls overlaid on it, at
    /// 20 pt apiece.
    private var controlsInset: CGFloat {
        8 + (canOpen ? 20 : 0) + (showDismiss ? 20 : 0)
    }

    /// Show a dismiss ✕ while the row wants something from you — an explicit
    /// `attention`, or a turn that ended and is holding the collapsed notch
    /// open. Both are dismissible, so both need the affordance.
    private var showDismiss: Bool {
        (waiting || (session.awaitingReply && !acknowledged)) && onDismiss != nil
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            if expanded && canOpen {
                drawer
            }
        }
        // A faint wash marks which row the open drawer belongs to; the list can
        // have several open at once.
        .background(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .fill(Color.white.opacity(expanded ? 0.05 : 0))
        )
        .padding(.horizontal, 2)
    }

    private var header: some View {
        Button {
            onSelect(session)
        } label: {
            HStack(alignment: .top, spacing: 9) {
                Circle()
                    .fill(dotColor)
                    .frame(width: 8, height: 8)
                    .padding(.top, 4)
                VStack(alignment: .leading, spacing: 2) {
                    Text(session.sandbox)
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundStyle(.white)
                    if let input = session.last_input, !input.isEmpty {
                        Text("You: \(input)")
                            .font(.system(size: 10))
                            .foregroundStyle(.white.opacity(0.55))
                            .lineLimit(1)
                    }
                    if showsCaption {
                        Text(subtitle)
                            .font(.system(size: 10))
                            .foregroundStyle(subtitleColor)
                            .lineLimit(1)
                    }
                }
                Spacer(minLength: 6)
                VStack(alignment: .trailing, spacing: 3) {
                    if !session.agent.isEmpty {
                        Tag(text: session.mode == "bash" ? "bash" : session.agent)
                    }
                    if waiting, !session.promptSteps.isEmpty {
                        Text("answer ›")
                            .font(.system(size: 9, weight: .semibold))
                            .foregroundStyle(.orange)
                    } else if session.elapsed > 0 {
                        Text(elapsedLabel(session.elapsed))
                            .font(.system(size: 9))
                            .foregroundStyle(.white.opacity(0.4))
                    }
                }
            }
            .padding(.vertical, 5)
            .padding(.leading, 8)
            // Leave room on the right for the controls overlaid below.
            .padding(.trailing, controlsInset)
            .frame(maxWidth: .infinity, alignment: .leading)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .pointerCursor()
        .help(hasQuestion ? "Answer \(session.sandbox)" : "Open \(session.sandbox) in the browser")
        // Separate buttons layered above the row: tapping one must not trigger
        // the row's own tap.
        //
        // Pinned to the top, not centred: the row grows downwards when the
        // drawer opens, and centred controls would slide down with it — away
        // from the pointer that just clicked. They stay level with the sandbox
        // name instead, the one line whose position never changes. The top inset
        // mirrors the status dot's, for the same reason: line up with the first
        // row of text rather than the top of the padding box.
        //
        // The chevron is the outermost of the two so that the ✕ coming and going
        // never moves it.
        .overlay(alignment: .topTrailing) {
            HStack(spacing: 0) {
                if showDismiss {
                    Button {
                        onDismiss?(session)
                    } label: {
                        Image(systemName: "xmark.circle.fill")
                            .font(.system(size: 13))
                            .foregroundStyle(.white.opacity(0.4))
                            .frame(width: 20, height: 18)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .pointerCursor()
                    .help("Dismiss waiting notification for \(session.sandbox)")
                }
                if canOpen {
                    disclosure
                }
            }
            .padding(.top, 4)
            .padding(.trailing, 6)
        }
    }

    /// The accordion's handle: the row's full reply and its composer are behind
    /// a deliberate click, not a hover. Resting the pointer somewhere is not a
    /// decision — it unfolded rows on the way past, and it could not open
    /// anything you then had to *type* into.
    private var disclosure: some View {
        Button(action: onToggle) {
            Image(systemName: "chevron.right")
                .font(.system(size: 10, weight: .semibold))
                .foregroundStyle(.white.opacity(expanded ? 0.85 : 0.4))
                .rotationEffect(.degrees(expanded ? 90 : 0))
                .frame(width: 20, height: 18)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .pointerCursor()
        .help(expanded
            ? "Close \(session.sandbox)"
            : "Read the full reply and write to \(session.sandbox)")
    }

    /// What the open accordion reveals: Claude's answer in full, and a field to
    /// answer it from here.
    private var drawer: some View {
        VStack(alignment: .leading, spacing: 5) {
            if let reply = drawerReply {
                Text(reply)
                    .font(.system(size: 10))
                    .foregroundStyle(.white.opacity(0.75))
                    .lineLimit(Self.replyLineLimit)
                    // Let it wrap: the interesting case is one long line, not a
                    // pre-broken paragraph.
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            RowChat(sandbox: session.sandbox, store: store, onFocusChange: onComposerFocus)
        }
        // Lines up under the sandbox name, past the status dot.
        .padding(.leading, 25)
        .padding(.trailing, 8)
        .padding(.bottom, 6)
        .transition(.opacity)
    }

    /// A dismissed waiting row loses its amber dot (it's no longer nagging).
    private var dotColor: Color {
        // Dismissed rows go quiet whichever way they were asking: `acknowledge`
        // only ever records `attention` or a finished turn, so the flag alone is
        // the signal.
        acknowledged ? .white.opacity(0.35) : session.accentColor
    }

    /// The question when waiting, else "your turn" once Claude has replied, else
    /// the current activity (ignoring the single-character fragments a
    /// redraw/typing produces), else the state.
    private var subtitle: String {
        if session.state == .attention, let q = session.promptSteps.first {
            let steps = session.promptSteps.count
            return steps > 1 ? "\(q.text) (1 of \(steps))" : q.text
        }
        // What Claude answered beats "waiting for your reply": the turn is over
        // either way, and the answer is the part you actually want to read.
        if showsReply, let lead = session.replyLead { return lead }
        if session.awaitingReply { return "waiting for your reply" }
        if let a = session.activity, a.count >= 3 { return a }
        return session.state.label
    }

    /// Whether *this row* ended up captioned with the reply. A structured
    /// question takes the caption ahead of it (see `subtitle`), and then the
    /// prose colour doesn't apply — it would be describing text the row isn't
    /// showing. `showsCaption` reads it for the same reason: only a caption that
    /// *is* the reply is the one the open drawer makes redundant.
    private var showsReply: Bool {
        if session.state == .attention, session.promptSteps.first != nil { return false }
        return session.showsReply
    }

    private var subtitleColor: Color {
        if waiting { return .orange }
        // A reply is prose, not a status — it shouldn't wear the teal that means
        // "your turn".
        if showsReply { return .white.opacity(0.75) }
        if session.awaitingReply { return awaitingReplyColor }
        return .white.opacity(0.7)
    }
}

/// The full list of session rows.
struct IslandView: View {
    @ObservedObject var store: SessionStore
    /// Row tap handler. Defaults to opening the sandbox in the browser.
    var onSelect: (SessionInfo) -> Void = { openInBrowser($0.sandbox) }
    /// Told when a chat composer takes or gives up keyboard focus. The notch
    /// panel uses it to become key (it can't be typed into otherwise) and to
    /// hold the list open while the user writes; the menu-bar popover, where
    /// focus is ordinary, leaves it at the default no-op.
    var onComposerFocus: (Bool) -> Void = { _ in }
    /// Called when a row's accordion opens or closes. Same reason as
    /// `onComposerFocus`: the notch panel's frame is set by hand.
    var onHeightChange: () -> Void = {}
    /// Told whether any row is currently open. The notch panel gives an island
    /// with an open drawer a longer leash before it retracts.
    var onExpandedChange: (Bool) -> Void = { _ in }

    /// Session ids whose accordion is open.
    ///
    /// Held by the list, not by each row: the panel has to know whether
    /// *anything* is open, and a row's own `@State` would be at the mercy of
    /// SwiftUI keeping its identity across the rebuild every store publish
    /// triggers.
    @State private var expanded: Set<String> = []

    /// Open or close a row's drawer.
    private func toggle(_ session: SessionInfo) {
        withAnimation(.easeOut(duration: 0.18)) {
            if expanded.contains(session.id) {
                expanded.remove(session.id)
            } else {
                expanded.insert(session.id)
            }
        }
        onExpandedChange(!expanded.isEmpty)
        onHeightChange()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            // Account-wide Claude subscription usage (from Claude Code's own
            // statusLine — see /api/usage), shown above the session list.
            if let usage = store.usage, usage.hasData {
                UsageHeader(usage: usage)
                Divider()
                    .overlay(Color.white.opacity(0.08))
                    .padding(.horizontal, 8)
                    .padding(.bottom, 2)
            }
            if store.sessions.isEmpty {
                Text(store.connected ? "No active sessions" : "Waiting for sbxw…")
                    .font(.caption)
                    .foregroundStyle(.white.opacity(0.6))
                    .padding(.vertical, 8)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 8)
            } else {
                // Only offered when it would do something — with nothing
                // pending it would be a dead control on every list.
                if store.dismissable.count > 1 {
                    DismissAllButton(count: store.dismissable.count) {
                        store.acknowledgeAll()
                        onHeightChange()
                    }
                }
                ForEach(store.sessions) { session in
                    SessionRow(
                        session: session,
                        store: store,
                        acknowledged: store.isAcknowledged(session),
                        expanded: expanded.contains(session.id),
                        onSelect: { s in
                            // Case 2: opening the sandbox counts as checking its
                            // notification, so dismiss it too.
                            store.acknowledge(s)
                            onSelect(s)
                        },
                        // Case 1: the explicit ✕ dismisses without navigating.
                        onDismiss: { store.acknowledge($0) },
                        onToggle: { toggle(session) },
                        onComposerFocus: onComposerFocus
                    )
                }
            }
            Divider()
                .overlay(Color.white.opacity(0.08))
                .padding(.horizontal, 8)
                .padding(.top, 2)
            ChatComposer(store: store, onFocusChange: onComposerFocus)
        }
        .padding(.vertical, 6)
        .frame(minWidth: 300)
        .onChange(of: store.sessions) { _, sessions in
            // A sandbox that goes away takes its drawer with it. Without this the
            // id would sit in the set for good and the panel would believe
            // something is still open — the notch would never retract again.
            let liveIDs = Set(sessions.map(\.id))
            if !expanded.isSubset(of: liveIDs) {
                expanded.formIntersection(liveIDs)
                onExpandedChange(!expanded.isEmpty)
            }
            // An open drawer's own text changes under it — Claude answers, and
            // the reply it is printing grows by several lines. Nothing else would
            // resize the panel for that; a relayout that finds the same height is
            // a no-op, so paying it per update is cheap.
            if !expanded.isEmpty { onHeightChange() }
        }
    }
}

/// "Clear all N" strip above the list: dismisses every session that is asking
/// for something, so a pile-up doesn't have to be cleared one ✕ at a time.
///
/// Deliberately quiet — it sits above rows it is *about*, and shouting would
/// make the list read as another notification.
struct DismissAllButton: View {
    let count: Int
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 5) {
                Spacer()
                Image(systemName: "checkmark.circle")
                    .font(.system(size: 9, weight: .semibold))
                Text("Clear all \(count)")
                    .font(.system(size: 10, weight: .medium))
            }
            .foregroundStyle(.white.opacity(hovering ? 0.85 : 0.45))
            .padding(.horizontal, 10)
            .padding(.vertical, 3)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .pointerCursor()
        .onHover { hovering = $0 }
        .help("Dismiss every session waiting on you")
    }
}

/// The message field every island composer is built from — the row drawers and
/// the "New chat" strip at the bottom. One view so the same gesture can't end up
/// with two different feels.
struct ChatField: View {
    let placeholder: String
    @Binding var text: String
    /// The owner's focus flag, so it can drive focus and report it upwards.
    var focus: FocusState<Bool>.Binding
    let sending: Bool
    /// What the send button and the spinner explain about *this* composer.
    let sendHelp: String
    let sendingHelp: String
    let send: () -> Void

    private var empty: Bool {
        text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: "bubble.left.fill")
                .font(.system(size: 10))
                .foregroundStyle(.white.opacity(0.4))
            TextField(placeholder, text: $text)
                .textFieldStyle(.plain)
                .font(.system(size: 12))
                .foregroundStyle(.white)
                .focused(focus)
                .disabled(sending)
                .onSubmit(send)
            if sending {
                ProgressView()
                    .controlSize(.small)
                    .help(sendingHelp)
            } else {
                Button(action: send) {
                    Image(systemName: "arrow.up.circle.fill")
                        .font(.system(size: 13))
                        .foregroundStyle(Color.white.opacity(empty ? 0.25 : 0.75))
                }
                .buttonStyle(.plain)
                .pointerCursor()
                .disabled(empty)
                .help(sendHelp)
            }
        }
    }
}

/// A one-line error under a composer.
struct ChatError: View {
    let message: String
    var body: some View {
        Text(message)
            .font(.system(size: 10))
            .foregroundStyle(.orange)
            .lineLimit(2)
            .fixedSize(horizontal: false, vertical: true)
    }
}

/// The composer inside an open row: write to *that* sandbox's agent from the
/// island, without a browser tab or a terminal.
///
/// The field stays put after a send (only the text clears) — the answer will
/// arrive in the drawer right above it, and a conversation is rarely one
/// message long.
struct RowChat: View {
    let sandbox: String
    @ObservedObject var store: SessionStore
    var onFocusChange: (Bool) -> Void = { _ in }

    @State private var text = ""
    @FocusState private var focused: Bool

    private var target: ChatTarget { .sandbox(sandbox) }
    private var state: SessionStore.ChatPushState { store.chatState(target) }

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            ChatField(
                placeholder: "Reply to \(sandbox)…",
                text: $text,
                focus: $focused,
                sending: state == .sending,
                sendHelp: "Send to \(sandbox)",
                sendingHelp: "Sending to \(sandbox)…",
                send: send
            )
            if case .failed(let message) = state {
                ChatError(message: message)
            }
        }
        // The panel has to know while the field holds focus: see onComposerFocus.
        .onChange(of: focused) { _, isFocused in onFocusChange(isFocused) }
        .onChange(of: text) { _, _ in store.clearChatPushError(target) }
        // Clear on success only; a failed push keeps the text so it can be
        // retried rather than retyped.
        .onChange(of: state) { previous, current in
            if previous == .sending, current == .idle { text = "" }
        }
    }

    private func send() {
        store.pushChat(text, to: target)
    }
}

/// Bottom-of-the-island composer: start a *brand-new* throwaway chat agent
/// without opening a browser or picking a workspace.
///
/// The ＋ expands an inline field; submitting hands the text to
/// `SessionStore.pushChat(_:to:)` as `.newChat`, and the daemon provisions the
/// next free `ephemeral-chat[-N]` for it. "New chat" therefore means what it
/// says — carrying on an existing conversation is the row drawer's job, one
/// click away on the chat's own row.
///
/// That does mean this button costs a container every time, so past
/// `crowdedThreshold` sandboxes it says so before you press it again.
struct ChatComposer: View {
    @ObservedObject var store: SessionStore
    var onFocusChange: (Bool) -> Void = { _ in }

    @State private var open = false
    @State private var text = ""
    @FocusState private var focused: Bool

    /// Number of live sandboxes from which the composer starts warning. Four is
    /// where a laptop starts to notice: each sandbox is a container with its own
    /// writable layers, its own agent process and its own workspace on disk.
    private static let crowdedThreshold = 4

    private var target: ChatTarget { .newChat }
    private var state: SessionStore.ChatPushState { store.chatState(target) }
    private var crowded: Bool { store.sessions.count >= Self.crowdedThreshold }

    /// Typed as `String` rather than left to `Text`'s overloads, which would have
    /// to choose between a localized key and prose for a concatenation.
    private var crowdedText: String {
        "\(store.sessions.count) sandboxes up — each one holds disk and memory. "
            + "Reply in a chat above instead, or remove the ones you're done with."
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            if open {
                ChatField(
                    placeholder: "Ask a new chat agent…",
                    text: $text,
                    focus: $focused,
                    sending: state == .sending,
                    sendHelp: "Start a new ephemeral chat",
                    sendingHelp: "Creating the chat sandbox…",
                    send: send
                )
                .padding(.vertical, 4)
                .padding(.horizontal, 8)
            } else {
                Button {
                    open = true
                    focused = true
                } label: {
                    HStack(spacing: 6) {
                        Image(systemName: "plus")
                            .font(.system(size: 10, weight: .bold))
                        Text("New chat")
                            .font(.system(size: 11))
                        Spacer()
                    }
                    .foregroundStyle(.white.opacity(0.55))
                    .padding(.vertical, 4)
                    .padding(.horizontal, 8)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .pointerCursor()
                .help("Start a new ephemeral chat sandbox")
            }

            if crowded {
                crowdedNote
            }

            if case .failed(let message) = state {
                ChatError(message: message)
                    .padding(.horizontal, 8)
            }
        }
        .onChange(of: focused) { _, isFocused in onFocusChange(isFocused) }
        .onChange(of: text) { _, _ in store.clearChatPushError(target) }
        // A push that succeeded retires the composer; a failed one stays open so
        // the text isn't lost and can be retried.
        .onChange(of: state) { previous, current in
            if previous == .sending, current == .idle {
                text = ""
                open = false
                focused = false
            }
        }
    }

    /// Said plainly and without a scold: what another sandbox costs, and the two
    /// ways out. Shown whether or not the field is open — it is about the button,
    /// not about the message being written.
    private var crowdedNote: some View {
        HStack(alignment: .top, spacing: 5) {
            Image(systemName: "exclamationmark.triangle.fill")
                .font(.system(size: 9))
            Text(crowdedText)
                .font(.system(size: 9))
                .fixedSize(horizontal: false, vertical: true)
        }
        .foregroundStyle(.orange.opacity(0.85))
        .padding(.horizontal, 8)
        .help("Sandboxes are cheap to make and not free to keep")
    }

    private func send() {
        store.pushChat(text, to: target)
    }
}

/// Subscription usage bars (5-hour + weekly windows) shown atop the list.
struct UsageHeader: View {
    let usage: UsageInfo

    var body: some View {
        VStack(spacing: 6) {
            if let p = usage.five_hour_pct {
                UsageBar(label: "5h", pct: p)
            }
            if let p = usage.seven_day_pct {
                UsageBar(label: "Week", pct: p)
            }
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 4)
    }
}

/// A single labelled usage bar: `5h  ▓▓▓░░  42%`.
struct UsageBar: View {
    let label: String
    let pct: Double

    private var color: Color {
        switch pct {
        case ..<70: return .green
        case ..<90: return .orange
        default: return .red
        }
    }

    var body: some View {
        HStack(spacing: 8) {
            Text(label)
                .font(.system(size: 10, weight: .medium))
                .foregroundStyle(.white.opacity(0.6))
                .frame(width: 32, alignment: .leading)
            GeometryReader { geo in
                ZStack(alignment: .leading) {
                    Capsule().fill(Color.white.opacity(0.12))
                    Capsule()
                        .fill(color)
                        .frame(width: max(3, geo.size.width * CGFloat(min(pct, 100) / 100)))
                }
            }
            .frame(height: 5)
            Text("\(Int(pct.rounded()))%")
                .font(.system(size: 10, weight: .semibold, design: .rounded))
                .foregroundStyle(.white.opacity(0.85))
                .frame(width: 34, alignment: .trailing)
        }
    }
}

// MARK: - Notch surfaces

/// Compact toast for a plain state change.
struct ToastView: View {
    let session: SessionInfo

    private var toastText: String {
        if session.awaitingReply { return "waiting for your reply" }
        return session.activity?.isEmpty == false ? session.activity! : session.state.label
    }

    var body: some View {
        HStack(spacing: 10) {
            Circle().fill(session.accentColor).frame(width: 10, height: 10)
            VStack(alignment: .leading, spacing: 2) {
                Text(session.sandbox)
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(.white)
                Text(toastText)
                    .font(.system(size: 11))
                    .foregroundStyle(session.awaitingReply ? awaitingReplyColor : .white.opacity(0.7))
                    .lineLimit(1)
            }
            Spacer(minLength: 8)
            if !session.agent.isEmpty {
                Tag(text: session.agent)
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 8)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// Compact pill the toast shrinks into: agent glyph + short text + a count of
/// how many sessions are live. Mirrors the minimized notification look.
struct MiniToastView: View {
    let session: SessionInfo
    @ObservedObject var store: SessionStore

    /// Claude's answer once the turn is over, else the user's last prompt, else
    /// the current activity, else the state. The answer comes first for the same
    /// reason as on the row: echoing back what *you* typed is the one thing you
    /// already know.
    private var text: String {
        if session.showsReply, let lead = session.replyLead { return lead }
        if let input = session.last_input, !input.isEmpty { return input }
        if let a = session.activity, a.count >= 3 { return a }
        return session.state.label
    }

    var body: some View {
        HStack(spacing: 9) {
            InvaderIcon(color: session.state == .attention
                ? Color(red: 1.0, green: 0.7, blue: 0.28)
                : Color(red: 0.44, green: 0.87, blue: 0.47))
                .frame(width: 17, height: 13)
            Text(text)
                .font(.system(size: 12, weight: .medium, design: .monospaced))
                .foregroundStyle(.white)
                .lineLimit(1)
                .truncationMode(.tail)
            Spacer(minLength: 6)
            Text("\(store.sessions.count)")
                .font(.system(size: 10, weight: .semibold, design: .rounded))
                .foregroundStyle(.white.opacity(0.75))
                .padding(.horizontal, 6)
                .padding(.vertical, 2)
                .background(Color.white.opacity(0.14))
                .clipShape(Capsule())
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// A small pixel-art space invader, drawn (no bundled asset) and tintable —
/// the agent glyph on the notch pill.
struct InvaderIcon: View {
    var color: Color = Color(red: 0.44, green: 0.87, blue: 0.47)

    private static let rows: [[Int]] = [
        [0, 0, 1, 0, 0, 0, 0, 0, 1, 0, 0],
        [0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0],
        [0, 0, 1, 1, 1, 1, 1, 1, 1, 0, 0],
        [0, 1, 1, 0, 1, 1, 1, 0, 1, 1, 0],
        [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1],
        [1, 0, 1, 1, 1, 1, 1, 1, 1, 0, 1],
        [1, 0, 1, 0, 0, 0, 0, 0, 1, 0, 1],
        [0, 0, 0, 1, 1, 0, 1, 1, 0, 0, 0],
    ]

    var body: some View {
        Canvas { ctx, size in
            let cols = 11, rowCount = Self.rows.count
            let cell = min(size.width / CGFloat(cols), size.height / CGFloat(rowCount))
            let ox = (size.width - cell * CGFloat(cols)) / 2
            let oy = (size.height - cell * CGFloat(rowCount)) / 2
            for (r, row) in Self.rows.enumerated() {
                for (c, v) in row.enumerated() where v == 1 {
                    let rect = CGRect(
                        x: ox + CGFloat(c) * cell, y: oy + CGFloat(r) * cell,
                        width: cell, height: cell
                    )
                    ctx.fill(Path(rect), with: .color(color))
                }
            }
        }
    }
}

/// Persistent notch bubble shown while the island is collapsed: an agent glyph,
/// the representative active session's task, and a session count. The black fill
/// rises to the top screen edge so the physical notch sits *inside* the bubble
/// (a sense of inclusion, not a pill floating below it). Depth comes from the
/// soft drop shadow applied by NotchContentView — no border. Renders nothing
/// (a hairline hover strip) when nothing is working or waiting.
struct SummaryPill: View {
    @ObservedObject var store: SessionStore
    /// Notch height (a small lip on screens without a notch): the task row sits
    /// below this, the black fills behind it.
    let topInset: CGFloat

    /// The session whose task to surface, by how much it wants you: an explicit
    /// prompt first, then a turn awaiting your reply, then a working one. Without
    /// the middle case the collapsed notch showed nothing at all while Claude sat
    /// waiting on an inline question — the very gap this closes.
    private var lead: SessionInfo? {
        store.sessions.first { $0.state == .attention && !store.isAcknowledged($0) }
            // Also skipped once dismissed: a prompt the user waved off ends its
            // turn (`Stop` → idle with the last input still set), which reads as
            // "waiting for your reply" and put the very session they'd just
            // dismissed straight back on the notch in teal.
            ?? store.sessions.first { $0.awaitingReply && !store.isAcknowledged($0) }
            ?? store.sessions.first { $0.state == .working }
    }

    /// Once the turn has ended, the pill carries Claude's answer rather than the
    /// prompt that produced it — the collapsed notch is read at a glance, and a
    /// finished session showing your own words back reads as if nothing happened.
    private func task(_ s: SessionInfo) -> String {
        if s.showsReply, let lead = s.replyLead { return lead }
        if let input = s.last_input, !input.isEmpty { return input }
        if let a = s.activity, a.count >= 3 { return a }
        return s.sandbox
    }

    /// Glyph tint mirrors the lead's urgency: amber prompt, teal your-turn,
    /// green working.
    private var leadIconColor: Color {
        guard let lead else { return Color(red: 0.44, green: 0.87, blue: 0.47) }
        if lead.state == .attention { return Color(red: 1.0, green: 0.7, blue: 0.28) }
        if lead.awaitingReply { return awaitingReplyColor }
        return Color(red: 0.44, green: 0.87, blue: 0.47)
    }

    var body: some View {
        Group {
            if let lead {
                VStack(spacing: 0) {
                    // The notch lives in this strip; the black behind it makes
                    // the notch look enclosed by the bubble.
                    Color.clear.frame(height: topInset)
                    HStack(spacing: 9) {
                        InvaderIcon(color: leadIconColor)
                            .frame(width: 17, height: 13)
                        Text(task(lead))
                            .font(.system(size: 12, weight: .medium, design: .monospaced))
                            .foregroundStyle(.white)
                            .lineLimit(1)
                            .truncationMode(.tail)
                        Spacer(minLength: 8)
                        Text("\(store.sessions.count)")
                            .font(.system(size: 11, weight: .semibold, design: .rounded))
                            .foregroundStyle(.white.opacity(0.7))
                            .padding(.horizontal, 7)
                            .padding(.vertical, 2)
                            .background(Color.white.opacity(0.13))
                            .clipShape(Capsule())
                    }
                    .padding(.horizontal, 12)
                    .padding(.top, 3)
                    .padding(.bottom, 9)
                }
                .frame(maxWidth: .infinity)
                // Square top (flush with the screen edge, around the notch),
                // generously rounded bottom — no border, just a drop shadow.
                .background(
                    UnevenRoundedRectangle(
                        topLeadingRadius: 0, bottomLeadingRadius: 22,
                        bottomTrailingRadius: 22, topTrailingRadius: 0,
                        style: .continuous
                    )
                    .fill(Color.black)
                )
                .transition(.opacity)
            } else {
                Color.clear.frame(height: 2)
            }
        }
        .animation(.spring(response: 0.35, dampingFraction: 0.72), value: lead?.id)
    }
}

/// Interactive prompt card: shows the pending question and one button per
/// option (⌘1…9).
///
/// `AskUserQuestion` can ask several questions in one call — the terminal walks
/// them as tabs and submits once at the end. The card mirrors that: it steps
/// through the questions, holding the answers locally, and sends the whole set
/// when the last one is picked. Nothing reaches the PTY before that, so an
/// abandoned card leaves the session exactly as it found it.
struct QuestionCard: View {
    let session: SessionInfo
    @ObservedObject var store: SessionStore
    @ObservedObject var controller: NotchController

    /// One 1-based option number per step already answered, in tab order. Its
    /// count is also the index of the step being asked. Owned by the controller
    /// so it does not depend on SwiftUI keeping this card's identity across the
    /// rebuilds every store publish triggers.
    private var answers: [Int] { controller.draftAnswers }

    private var steps: [Question] { session.promptSteps }

    /// The step currently on screen (nil only in the instant between the last
    /// answer and the card closing).
    private var step: Question? {
        steps.indices.contains(answers.count) ? steps[answers.count] : nil
    }

    private var isMultiStep: Bool { steps.count > 1 }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            header
            if isMultiStep, !answers.isEmpty {
                recap
            }
            if let step {
                if !step.context.isEmpty {
                    contextView(step.context)
                }
                Text(step.text)
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundStyle(.white)
                    .fixedSize(horizontal: false, vertical: true)
                VStack(spacing: 5) {
                    ForEach(Array(step.options.enumerated()), id: \.offset) { idx, option in
                        optionButton(index: idx + 1, text: option)
                    }
                }
            }
            footer
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var header: some View {
        HStack(spacing: 6) {
            Image(systemName: "bubble.left.fill")
                .font(.system(size: 11))
                .foregroundStyle(.orange)
            Text("\(session.agent.isEmpty ? "Agent" : session.agent) asks")
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(.white.opacity(0.85))
            if isMultiStep {
                Text("\(min(answers.count + 1, steps.count))/\(steps.count)")
                    .font(.system(size: 10, weight: .semibold, design: .rounded))
                    .foregroundStyle(.white.opacity(0.75))
                    .padding(.horizontal, 6)
                    .padding(.vertical, 2)
                    .background(Color.white.opacity(0.14))
                    .clipShape(Capsule())
            }
            Spacer()
            Text(session.sandbox)
                .font(.system(size: 10))
                .foregroundStyle(.white.opacity(0.5))
            Button {
                // Dismiss the prompt card and silence its notification; the
                // session keeps waiting in the daemon, we just stop nagging.
                store.acknowledge(session)
                controller.dismissQuestion()
            } label: {
                Image(systemName: "xmark.circle.fill")
                    .font(.system(size: 12))
                    .foregroundStyle(.white.opacity(0.4))
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .pointerCursor()
            .help("Dismiss")
        }
    }

    /// What has been picked so far, so a later step still shows the earlier
    /// choices — the terminal keeps them visible as answered tabs.
    private var recap: some View {
        HStack(spacing: 5) {
            ForEach(Array(answers.enumerated()), id: \.offset) { i, choice in
                Text(label(step: i, choice: choice))
                    .font(.system(size: 10))
                    .foregroundStyle(.white.opacity(0.7))
                    .padding(.horizontal, 7)
                    .padding(.vertical, 2)
                    .background(Color.orange.opacity(0.18))
                    .clipShape(Capsule())
                    .lineLimit(1)
            }
            Spacer(minLength: 0)
        }
    }

    private var footer: some View {
        HStack(spacing: 12) {
            if !answers.isEmpty {
                Button {
                    withAnimation(.easeInOut(duration: 0.18)) {
                        controller.undoAnswer()
                    }
                } label: {
                    HStack(spacing: 5) {
                        Image(systemName: "chevron.left")
                        Text("Back")
                    }
                    .font(.system(size: 11))
                    .foregroundStyle(.white.opacity(0.55))
                }
                .buttonStyle(.plain)
                .pointerCursor()
                .keyboardShortcut(.leftArrow, modifiers: .command)
            }
            Button {
                openInBrowser(session.sandbox)
            } label: {
                HStack(spacing: 5) {
                    Image(systemName: "arrow.up.forward.app")
                    Text("Go to browser")
                }
                .font(.system(size: 11))
                .foregroundStyle(.white.opacity(0.55))
            }
            .buttonStyle(.plain)
            .pointerCursor()
            Spacer(minLength: 0)
        }
        .padding(.top, 2)
    }

    /// The chosen option's label for an answered step.
    private func label(step index: Int, choice: Int) -> String {
        guard steps.indices.contains(index),
              steps[index].options.indices.contains(choice - 1)
        else { return "?" }
        return steps[index].options[choice - 1]
    }

    /// How many context lines the card shows before summarising the rest. The
    /// decision table is one line per option, so this only bites on unusually
    /// long prompts — and the full text is one click away in the browser.
    private static let maxContextLines = 8

    /// The on-screen preamble (a diff, a decision table…) shown above the prompt,
    /// monospaced so tables stay aligned.
    ///
    /// Deliberately a plain stack rather than a `ScrollView`: the panel measures
    /// its own content to size itself, and a scroll container has no natural
    /// height *and* carries scroll geometry, so measuring it re-dirties the view
    /// graph. Inside AppKit's constraint-update pass that re-entrancy raises an
    /// exception and kills the app (see `NotchController.install`). Capping the
    /// line count keeps the card bounded without one.
    private func contextView(_ lines: [String]) -> some View {
        let shown = Array(lines.prefix(Self.maxContextLines))
        let hidden = lines.count - shown.count
        return VStack(alignment: .leading, spacing: 1) {
            ForEach(Array(shown.enumerated()), id: \.offset) { _, line in
                Text(line)
                    .font(.system(size: 10, design: .monospaced))
                    .foregroundStyle(.white.opacity(0.6))
                    .lineLimit(1)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            if hidden > 0 {
                Text("+\(hidden) more")
                    .font(.system(size: 10, design: .monospaced))
                    .foregroundStyle(.white.opacity(0.35))
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .padding(8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.white.opacity(0.05))
        .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
    }

    private func optionButton(index: Int, text: String) -> some View {
        Button {
            withAnimation(.easeInOut(duration: 0.18)) {
                controller.pickAnswer(index, for: session)
            }
        } label: {
            HStack(spacing: 8) {
                Text(index <= 9 ? "⌘\(index)" : "\(index).")
                    .font(.system(size: 10, weight: .semibold, design: .monospaced))
                    .foregroundStyle(.white.opacity(0.6))
                    .frame(width: 24, alignment: .leading)
                Text(text)
                    .font(.system(size: 12))
                    .foregroundStyle(.white)
                    .lineLimit(1)
                Spacer(minLength: 4)
            }
            .padding(.vertical, 7)
            .padding(.horizontal, 10)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Color.white.opacity(0.08))
            .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .pointerCursor()
        .keyboardShortcut(shortcut(index))
    }

    /// ⌘-digit for an option, or none past the ninth: `KeyEquivalent` wraps a
    /// single `Character`, and `Character("10")` is a runtime trap. Options
    /// beyond the ninth stay clickable, they just have no shortcut.
    private func shortcut(_ index: Int) -> KeyboardShortcut? {
        guard (1...9).contains(index) else { return nil }
        return KeyboardShortcut(KeyEquivalent(Character("\(index)")), modifiers: .command)
    }
}

/// Root view hosted in the notch panel. Switches between collapsed (a hover
/// target hidden behind the notch), a toast, an interactive question, and the
/// full list.
///
/// Its `.onHover` drives the auto-hide of the two modes that take mouse events.
/// The others are click-through, so nothing here sees the pointer and the reveal
/// comes from `NotchController.pointerMoved` instead.
struct NotchContentView: View {
    @ObservedObject var controller: NotchController
    @ObservedObject var store: SessionStore

    /// Panel width per mode. The collapsed strip and the full toast share a
    /// width so the *appearance* (collapsed→toast) only grows vertically — a
    /// bubble dropping from the notch, never a sideways slide. Mode-to-mode
    /// changes while visible (toast→mini) animate as a centred contraction.
    private var width: CGFloat {
        switch controller.display {
        // The collapsed pill hangs from the notch (wider than it, like the
        // reference); everything else expands outward from there.
        case .collapsed: return 280
        case .toast: return 380
        case .miniToast: return 260
        case .question: return 520
        case .list: return 420
        }
    }

    /// Clearance above the content: the notch on a notched Mac, a small lip on
    /// screens without one (see NotchController.topClearance).
    private var topClearance: CGFloat { controller.topClearance }

    /// Transparent room around the content so the drop shadow isn't clipped by
    /// the (content-sized) window. No room on top: the bubble hugs the notch and
    /// the shadow falls downward.
    private let shadowPad: CGFloat = 26


    /// Distinguishes the current mode (and which session) so switching between
    /// toast/question/list cross-fades the inner content.
    private var caseKey: String {
        switch controller.display {
        case .collapsed: return "collapsed"
        case .toast(let i): return "toast-\(i.id)"
        case .miniToast(let i): return "mini-\(i.id)"
        case .question(let i): return "question-\(i.id)"
        case .list: return "list"
        }
    }

    var body: some View {
        Group {
            if case .collapsed = controller.display {
                // Persistent notch bubble: the black fills up to the top edge so
                // the notch sits *inside* it (inclusion), with the task row below
                // the notch. Empty (a hairline hover strip) when nothing runs.
                SummaryPill(store: store, topInset: controller.topInset)
            } else {
                VStack(spacing: 0) {
                    Color.clear.frame(height: topClearance) // clear the notch
                    content
                        // Cross-fade when the mode (or session) changes.
                        .id(caseKey)
                        .transition(.opacity)
                }
                // Solid fill (no translucency edge, no stroke) — depth comes
                // from the drop shadow below, not a border.
                .background(
                    RoundedRectangle(cornerRadius: 18, style: .continuous)
                        .fill(Color.black)
                )
                .clipShape(RoundedRectangle(cornerRadius: 18, style: .continuous))
                // Inflate from the notch like a bubble; fade cleanly on close.
                .transition(.asymmetric(
                    insertion: .scale(scale: 0.72, anchor: .top).combined(with: .opacity),
                    removal: .opacity.combined(with: .scale(scale: 0.94, anchor: .top))
                ))
            }
        }
        .frame(width: width)
        // Soft drop shadow (replaces the window's hard shadow rim). The padding
        // gives it room so it isn't clipped by the content-sized window.
        .shadow(color: .black.opacity(0.5), radius: 16, x: 0, y: 8)
        .padding(.horizontal, shadowPad)
        .padding(.bottom, shadowPad)
        .contentShape(Rectangle())
        // Only reaches us in the modes that take mouse events at all — the list
        // and a question card, where the whole surface should hold the island
        // open. While the island is a mere indicator the panel is click-through,
        // so it sees no pointer and the reveal comes from
        // `NotchController`'s own screen-coordinate tracking instead.
        .onHover { controller.hover($0) }
        // The animation curve (bouncy on grow, clean on retract) is chosen by
        // NotchController.setDisplay via withAnimation, so no `.animation(value:)`
        // here — that would apply the same curve to the retract.
    }

    @ViewBuilder
    private var content: some View {
        switch controller.display {
        case .collapsed:
            EmptyView()
        case .toast(let info):
            ToastView(session: info)
        case .miniToast(let info):
            MiniToastView(session: info, store: store)
        case .question(let info):
            QuestionCard(session: info, store: store, controller: controller)
        case .list:
            IslandView(
                store: store,
                onSelect: { info in
                    // Tapping a waiting-with-prompt row opens its answer card;
                    // anything else jumps to the browser.
                    if info.state == .attention, !info.promptSteps.isEmpty {
                        controller.showQuestion(info)
                    } else {
                        openInBrowser(info.sandbox)
                    }
                },
                onComposerFocus: { controller.setComposerActive($0) },
                onHeightChange: { controller.contentHeightChanged() },
                onExpandedChange: { controller.setRowsExpanded($0) }
            )
        }
    }
}
