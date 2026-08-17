import SwiftUI

/// The notch card for a cross-sandbox information request: an agent asked for
/// something outside its own workspace, and this is the human's half of it.
///
/// Two shapes, one per state that wants you:
///
///  - **pending** — the question, and one button per sandbox that could answer
///    it. Picking one sends it; nothing has been shared yet.
///  - **answered** — what came back, and the decision to release it or not.
///
/// A `routed` request draws no card at all: it is out with another agent, which
/// can think for minutes, and the notch has no business holding the menu bar for
/// something you cannot act on (see `RelayRequest.needsYou`).
///
/// Deliberately no `ScrollView` and every block line-capped — the panel measures
/// this content to size itself, and a scroll container has no natural height, so
/// measuring one re-dirties the view graph mid-layout and takes the app with it
/// (the same constraint `QuestionCard.contextView` is written around).
struct RelayCard: View {
    let request: RelayRequest
    @ObservedObject var store: SessionStore
    @ObservedObject var relay: RelayStore
    @ObservedObject var controller: NotchController

    /// Lines of the question shown before the rest is summarised.
    private static let maxQuestionLines = 7

    /// Lines of the answer shown *and releasable*. Past this the card cannot
    /// show you what you would be releasing, so it stops offering to — see
    /// `canRelease`.
    private static let maxAnswerLines = 10

    /// Sandboxes offered a shortcut. Nine because that is where ⌘-digit stops;
    /// the rest are one click away in the browser.
    private static let maxTargets = 9

    private var busy: Bool { relay.acting.contains(request.id) }

    private var targets: [String] {
        relay.candidates(for: request, sessions: store.sessions)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            header
            quoted(request.question, limit: Self.maxQuestionLines, tint: .purple)
            if let note = request.note, !note.isEmpty {
                Label(note, systemImage: "exclamationmark.triangle.fill")
                    .font(.system(size: 10))
                    .foregroundStyle(.orange.opacity(0.9))
                    .fixedSize(horizontal: false, vertical: true)
            }
            switch request.state {
            case .pending: routingSection
            case .answered: reviewSection
            default: EmptyView()
            }
            footer
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .opacity(busy ? 0.55 : 1)
    }

    // MARK: - Header

    private var header: some View {
        HStack(spacing: 6) {
            Image(systemName: "arrow.left.arrow.right")
                .font(.system(size: 10, weight: .semibold))
                .foregroundStyle(.purple)
            Text(request.from)
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(.white.opacity(0.9))
                .lineLimit(1)
            Text(request.state == .answered ? "is waiting on your review" : "wants to ask another sandbox")
                .font(.system(size: 12))
                .foregroundStyle(.white.opacity(0.6))
                .lineLimit(1)
            Spacer(minLength: 4)
            Text(elapsedLabel(request.elapsed))
                .font(.system(size: 10, design: .rounded))
                .foregroundStyle(.white.opacity(0.4))
            Button {
                // "Later", never "no". The daemon keeps the request and the
                // asking agent keeps waiting; refusing is its own button, below,
                // because an ignored request gets asked again and a refused one
                // does not.
                relay.dismiss(request)
                controller.dismissRelay()
            } label: {
                Image(systemName: "xmark.circle.fill")
                    .font(.system(size: 12))
                    .foregroundStyle(.white.opacity(0.4))
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .pointerCursor()
            .help("Later — keeps the request, stops showing it here")
        }
    }

    // MARK: - Pending: who gets the question

    private var routingSection: some View {
        VStack(alignment: .leading, spacing: 5) {
            if targets.isEmpty {
                Text("No other sandbox is running — start one, or refuse this.")
                    .font(.system(size: 11))
                    .foregroundStyle(.white.opacity(0.55))
                    .fixedSize(horizontal: false, vertical: true)
            } else {
                sectionLabel("Send it to")
                ForEach(Array(targets.prefix(Self.maxTargets).enumerated()), id: \.element) { idx, name in
                    targetButton(index: idx + 1, name: name)
                }
                if targets.count > Self.maxTargets {
                    Text("+\(targets.count - Self.maxTargets) more in the browser")
                        .font(.system(size: 10))
                        .foregroundStyle(.white.opacity(0.35))
                }
            }
        }
    }

    private func targetButton(index: Int, name: String) -> some View {
        Button {
            relay.route(request, to: name)
            // The request leaves the "needs you" states the moment the daemon
            // confirms, and the notch follows that rather than guessing here.
            controller.dismissRelay()
        } label: {
            HStack(spacing: 8) {
                Text(index <= 9 ? "⌘\(index)" : "\(index).")
                    .font(.system(size: 10, weight: .semibold, design: .monospaced))
                    .foregroundStyle(.white.opacity(0.6))
                    .frame(width: 24, alignment: .leading)
                Text(name)
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
        .disabled(busy)
        .keyboardShortcut(shortcut(index))
    }

    // MARK: - Answered: release it or not

    /// Whether the card may offer to release the answer.
    ///
    /// Only when it can show the whole thing. The browser popup lets you *edit*
    /// an answer before releasing it — trim it, cut a secret out of it — and the
    /// notch has no editor, so one click here would release text verbatim. That
    /// is defensible for an answer you have read in full and indefensible for one
    /// the card truncated, which is exactly what this refuses to do.
    private var canRelease: Bool {
        guard let answer = request.answerText else { return false }
        return answer.split(separator: "\n", omittingEmptySubsequences: false).count
            <= Self.maxAnswerLines && answer.count <= 600
    }

    @ViewBuilder
    private var reviewSection: some View {
        if let answer = request.answerText {
            VStack(alignment: .leading, spacing: 5) {
                sectionLabel(request.to.map { "\($0) answered" } ?? "Answer")
                quoted(answer, limit: Self.maxAnswerLines, tint: .green)
                if !canRelease {
                    Label(
                        "Too long to read here — review and release it in the browser.",
                        systemImage: "arrow.up.forward.app"
                    )
                    .font(.system(size: 10))
                    .foregroundStyle(.white.opacity(0.5))
                    .fixedSize(horizontal: false, vertical: true)
                }
                HStack(spacing: 8) {
                    if canRelease {
                        Button {
                            relay.approve(request)
                            controller.dismissRelay()
                        } label: {
                            HStack(spacing: 5) {
                                Image(systemName: "checkmark")
                                Text("Send to \(request.from)")
                                    .lineLimit(1)
                            }
                            .font(.system(size: 11, weight: .semibold))
                            .foregroundStyle(.white)
                            .padding(.vertical, 6)
                            .padding(.horizontal, 12)
                            .background(Color.green.opacity(0.35))
                            .clipShape(Capsule())
                            .contentShape(Capsule())
                        }
                        .buttonStyle(.plain)
                        .pointerCursor()
                        .disabled(busy)
                        .keyboardShortcut(.return, modifiers: .command)
                    }
                    refuseButton
                    Spacer(minLength: 0)
                }
                .padding(.top, 2)
            }
        }
    }

    // MARK: - Shared bits

    private var refuseButton: some View {
        Button {
            relay.deny(request)
            controller.dismissRelay()
        } label: {
            HStack(spacing: 5) {
                Image(systemName: "hand.raised.fill")
                Text("Refuse")
            }
            .font(.system(size: 11))
            .foregroundStyle(.red.opacity(0.85))
            .padding(.vertical, 6)
            .padding(.horizontal, 10)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .pointerCursor()
        .disabled(busy)
        .help("Nothing is shared, now or later — and the agent is told not to re-send it")
    }

    private var footer: some View {
        HStack(spacing: 12) {
            if request.state == .pending {
                refuseButton
            }
            Button {
                // Handing this off to the browser popup — where the same
                // request is reviewable and editable — is "later" from the
                // notch's point of view: dismiss it here exactly as the ✕
                // would, rather than leaving the card sitting open behind the
                // tab the user just switched to.
                //
                // `relay.dismiss`, not `controller.dismissRelay()` alone: the
                // former records *what* was dismissed (this state), so if the
                // request moves on without the user watching for it in the
                // browser — the sandbox answers, say — the card is owed a
                // reappearance and `apply(_:)` gives it one. Collapsing the
                // panel without that record would leave the notch silent on
                // the next arrival too, on the assumption the user is still
                // looking at a tab they may have long since closed.
                relay.dismiss(request)
                controller.dismissRelay()
                openInBrowser(request.from)
            } label: {
                HStack(spacing: 5) {
                    Image(systemName: "arrow.up.forward.app")
                    Text("Open in browser")
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

    private func sectionLabel(_ text: String) -> some View {
        Text(text.uppercased())
            .font(.system(size: 9, weight: .semibold))
            .kerning(0.5)
            .foregroundStyle(.white.opacity(0.45))
            .lineLimit(1)
    }

    /// Someone else's words, quoted rather than styled as ours: a question
    /// written to look like an instruction still has to read as something a
    /// stranger said. Line-capped for the reason in the type's doc comment.
    private func quoted(_ text: String, limit: Int, tint: Color) -> some View {
        let lines = text.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        let shown = Array(lines.prefix(limit))
        let hidden = lines.count - shown.count
        return HStack(alignment: .top, spacing: 8) {
            Rectangle()
                .fill(tint.opacity(0.7))
                .frame(width: 2)
            VStack(alignment: .leading, spacing: 2) {
                ForEach(Array(shown.enumerated()), id: \.offset) { _, line in
                    Text(line)
                        .font(.system(size: 12))
                        .foregroundStyle(.white.opacity(0.92))
                        .fixedSize(horizontal: false, vertical: true)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                if hidden > 0 {
                    Text("+\(hidden) more line\(hidden == 1 ? "" : "s")")
                        .font(.system(size: 10))
                        .foregroundStyle(.white.opacity(0.35))
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
        }
        .padding(.vertical, 6)
        .padding(.horizontal, 8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.white.opacity(0.05))
        .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
    }

    /// ⌘-digit for a target, or none past the ninth: `KeyEquivalent` wraps a
    /// single `Character`, and `Character("10")` is a runtime trap.
    private func shortcut(_ index: Int) -> KeyboardShortcut? {
        guard (1...9).contains(index) else { return nil }
        return KeyboardShortcut(KeyEquivalent(Character("\(index)")), modifiers: .command)
    }
}

/// The strip at the top of the island's list when requests are waiting on you —
/// the way back to a card closed with "Later", and the only place a `routed`
/// request is visible at all.
///
/// Counted, not listed one per line: the list below it is already the island's
/// long surface, and a pile of relay rows would push the sessions off the notch.
struct RelayBanner: View {
    @ObservedObject var relay: RelayStore
    let onOpen: (RelayRequest) -> Void
    @State private var hovering = false

    // Deliberately *not* `relay.pendingForYou`: that also drops a dismissed
    // request, and this banner exists specifically to be the way back to one.
    // `acting` is excluded on its own terms — the same in-flight window
    // `pendingForYou` guards against, so a click here can't reopen the very
    // request the user just acted on before the daemon has echoed it back.
    private var waiting: [RelayRequest] {
        relay.requests.filter { $0.needsYou && !relay.acting.contains($0.id) }
    }
    private var outstanding: [RelayRequest] { relay.requests.filter { $0.state == .routed } }

    var body: some View {
        if let first = waiting.first {
            button(label: waitingLabel, icon: "arrow.left.arrow.right", tint: .purple) {
                onOpen(first)
            }
        } else if let first = outstanding.first {
            // Nothing to decide yet — shown so a question out with another agent
            // isn't simply invisible until it comes back.
            //
            // Opens the browser rather than a card: the only things left to do
            // to a routed request are re-routing it and refusing it, and both
            // live in the popup over there. A card for it would also be closed
            // the instant it opened, since the notch only holds a request that
            // wants something (see `NotchController.onRelayChanged`).
            button(label: outstandingLabel, icon: "hourglass", tint: .white.opacity(0.5)) {
                openInBrowser(first.from)
            }
        }
    }

    private var waitingLabel: String {
        let n = waiting.count
        return n == 1
            ? "\(waiting[0].from) is asking another sandbox"
            : "\(n) sandbox requests need you"
    }

    private var outstandingLabel: String {
        let n = outstanding.count
        guard n == 1 else { return "\(n) questions out with other sandboxes" }
        let req = outstanding[0]
        return "\(req.to ?? "a sandbox") is answering \(req.from)"
    }

    private func button(
        label: String, icon: String, tint: Color, action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            HStack(spacing: 6) {
                Image(systemName: icon)
                    .font(.system(size: 9, weight: .semibold))
                    .foregroundStyle(tint)
                Text(label)
                    .font(.system(size: 10, weight: .medium))
                    .foregroundStyle(.white.opacity(hovering ? 0.95 : 0.7))
                    .lineLimit(1)
                Spacer(minLength: 0)
                Image(systemName: "chevron.right")
                    .font(.system(size: 8, weight: .semibold))
                    .foregroundStyle(.white.opacity(hovering ? 0.7 : 0.3))
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 5)
            .background(Color.purple.opacity(hovering ? 0.22 : 0.14))
            .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .pointerCursor()
        .onHover { hovering = $0 }
        .padding(.horizontal, 8)
        .padding(.bottom, 2)
    }
}
