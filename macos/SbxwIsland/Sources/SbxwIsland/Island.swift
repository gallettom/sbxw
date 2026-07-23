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
    /// Whether the user has already dismissed this session's waiting notification.
    var acknowledged: Bool = false
    /// Called when the row is tapped (e.g. show the prompt card, or jump).
    var onSelect: (SessionInfo) -> Void = { openInBrowser($0.sandbox) }
    /// Called when the user taps the row's ✕ to dismiss its waiting state. Absent
    /// (no ✕) where dismissal doesn't apply.
    var onDismiss: ((SessionInfo) -> Void)? = nil

    /// Still-waiting *and* not yet dismissed: the row reads as needing you.
    private var waiting: Bool {
        session.state == .attention && !acknowledged
    }

    private var hasQuestion: Bool {
        session.state == .attention && session.question != nil
    }

    /// Show a dismiss ✕ only while the row is actively asking for attention.
    private var showDismiss: Bool {
        waiting && onDismiss != nil
    }

    var body: some View {
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
                    Text(subtitle)
                        .font(.system(size: 10))
                        .foregroundStyle(subtitleColor)
                        .lineLimit(1)
                }
                Spacer(minLength: 6)
                VStack(alignment: .trailing, spacing: 3) {
                    if !session.agent.isEmpty {
                        Tag(text: session.mode == "bash" ? "bash" : session.agent)
                    }
                    if waiting, session.question != nil {
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
            // Leave room on the right for the dismiss ✕ overlay when shown.
            .padding(.trailing, showDismiss ? 26 : 8)
            .frame(maxWidth: .infinity, alignment: .leading)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .pointerCursor()
        .help(hasQuestion ? "Answer \(session.sandbox)" : "Open \(session.sandbox) in the browser")
        // A separate button layered above the row: tapping it dismisses without
        // triggering the row's own tap.
        .overlay(alignment: .trailing) {
            if showDismiss {
                Button {
                    onDismiss?(session)
                } label: {
                    Image(systemName: "xmark.circle.fill")
                        .font(.system(size: 13))
                        .foregroundStyle(.white.opacity(0.4))
                        .padding(.trailing, 8)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .pointerCursor()
                .help("Dismiss waiting notification for \(session.sandbox)")
            }
        }
    }

    /// A dismissed waiting row loses its amber dot (it's no longer nagging).
    private var dotColor: Color {
        (session.state == .attention && acknowledged)
            ? .white.opacity(0.35)
            : session.state.dotColor
    }

    /// The question when waiting, else the current activity (ignoring the
    /// single-character fragments a redraw/typing produces), else the state.
    private var subtitle: String {
        if session.state == .attention, let q = session.question {
            return q.text
        }
        if let a = session.activity, a.count >= 3 { return a }
        return session.state.label
    }

    private var subtitleColor: Color {
        waiting ? .orange : .white.opacity(0.7)
    }
}

/// The full list of session rows.
struct IslandView: View {
    @ObservedObject var store: SessionStore
    /// Row tap handler. Defaults to opening the sandbox in the browser.
    var onSelect: (SessionInfo) -> Void = { openInBrowser($0.sandbox) }

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
                ForEach(store.sessions) { session in
                    SessionRow(
                        session: session,
                        acknowledged: store.acknowledged.contains(session.id),
                        onSelect: { s in
                            // Case 2: opening the sandbox counts as checking its
                            // notification, so dismiss it too.
                            store.acknowledge(s)
                            onSelect(s)
                        },
                        // Case 1: the explicit ✕ dismisses without navigating.
                        onDismiss: { store.acknowledge($0) }
                    )
                }
            }
        }
        .padding(.vertical, 6)
        .frame(minWidth: 300)
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

    var body: some View {
        HStack(spacing: 10) {
            Circle().fill(session.state.dotColor).frame(width: 10, height: 10)
            VStack(alignment: .leading, spacing: 2) {
                Text(session.sandbox)
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(.white)
                Text(session.activity?.isEmpty == false ? session.activity! : session.state.label)
                    .font(.system(size: 11))
                    .foregroundStyle(.white.opacity(0.7))
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

    /// Prefer the user's last prompt, else the current activity, else the state.
    private var text: String {
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

    /// The session whose task to surface: a waiting one first (it needs you),
    /// else a working one.
    private var lead: SessionInfo? {
        store.sessions.first { $0.state == .attention && !store.acknowledged.contains($0.id) }
            ?? store.sessions.first { $0.state == .working }
    }

    private func task(_ s: SessionInfo) -> String {
        if let input = s.last_input, !input.isEmpty { return input }
        if let a = s.activity, a.count >= 3 { return a }
        return s.sandbox
    }

    var body: some View {
        Group {
            if let lead {
                VStack(spacing: 0) {
                    // The notch lives in this strip; the black behind it makes
                    // the notch look enclosed by the bubble.
                    Color.clear.frame(height: topInset)
                    HStack(spacing: 9) {
                        InvaderIcon(color: lead.state == .attention
                            ? Color(red: 1.0, green: 0.7, blue: 0.28)  // amber when waiting
                            : Color(red: 0.44, green: 0.87, blue: 0.47)) // green when working
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

/// Interactive prompt card: shows the parsed question and one button per option
/// (⌘1…9). Selecting one answers the session's PTY.
struct QuestionCard: View {
    let session: SessionInfo
    @ObservedObject var store: SessionStore
    @ObservedObject var controller: NotchController

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 6) {
                Image(systemName: "bubble.left.fill")
                    .font(.system(size: 11))
                    .foregroundStyle(.orange)
                Text("\(session.agent.isEmpty ? "Agent" : session.agent) asks")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(.white.opacity(0.85))
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
            if let q = session.question {
                if !q.context.isEmpty {
                    contextView(q.context)
                }
                Text(q.text)
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundStyle(.white)
                    .fixedSize(horizontal: false, vertical: true)
                VStack(spacing: 5) {
                    ForEach(Array(q.options.enumerated()), id: \.offset) { idx, option in
                        optionButton(index: idx + 1, text: option)
                    }
                }
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
            .padding(.top, 2)
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    /// The on-screen preamble (a diff, a decision table…) shown above the prompt,
    /// monospaced so tables stay aligned; scrolls if tall.
    private func contextView(_ lines: [String]) -> some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 1) {
                ForEach(Array(lines.enumerated()), id: \.offset) { _, line in
                    Text(line)
                        .font(.system(size: 10, design: .monospaced))
                        .foregroundStyle(.white.opacity(0.6))
                        .lineLimit(1)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
            .padding(8)
        }
        .frame(maxHeight: 150)
        .background(Color.white.opacity(0.05))
        .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
    }

    private func optionButton(index: Int, text: String) -> some View {
        Button {
            store.answer(session, index: index)
            controller.dismissAfterAnswer()
        } label: {
            HStack(spacing: 8) {
                Text("⌘\(index)")
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
        .keyboardShortcut(shortcut(index), modifiers: .command)
    }

    private func shortcut(_ index: Int) -> KeyEquivalent {
        KeyEquivalent(Character("\(index)"))
    }
}

/// Root view hosted in the notch panel. Switches between collapsed (a hover
/// target hidden behind the notch), a toast, an interactive question, and the
/// full list. Its `.onHover` drives reveal/auto-hide.
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
            IslandView(store: store) { info in
                // Tapping a waiting-with-prompt row opens its answer card;
                // anything else jumps to the browser.
                if info.state == .attention, info.question != nil {
                    controller.showQuestion(info)
                } else {
                    openInBrowser(info.sandbox)
                }
            }
        }
    }
}
