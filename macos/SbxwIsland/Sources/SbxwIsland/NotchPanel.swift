import AppKit
import SwiftUI
import Combine
import QuartzCore

/// What the notch panel is currently showing.
enum IslandDisplay: Equatable {
    /// Hidden behind the physical notch — just a hover target.
    case collapsed
    /// A transient state-change announcement (full form, shown briefly).
    case toast(SessionInfo)
    /// The same announcement shrunk to a compact pill (icon + short text +
    /// count), lingering a little longer before it disappears.
    case miniToast(SessionInfo)
    /// An interactive prompt waiting to be answered.
    case question(SessionInfo)
    /// The full list of sessions (revealed on hover).
    case list
}

/// Drives the notch "Dynamic Island": a borderless panel hugging the notch. It
/// surfaces a pending question card on its own (no click needed), toasts plain
/// state changes, and reveals the full list on hover. The panel sizes itself to
/// the SwiftUI content (height adapts).
@MainActor
final class NotchController: ObservableObject {
    @Published private(set) var display: IslandDisplay = .collapsed

    let store: SessionStore
    private var panel: NSPanel?
    private var hosting: NSHostingView<NotchContentView>?
    private var hovering = false
    private var hideTask: Task<Void, Never>?
    private var toastTask: Task<Void, Never>?
    /// Pending hover-intent reveal (see `hoverIntentDelay`).
    private var hoverTask: Task<Void, Never>?
    private var cancellables = Set<AnyCancellable>()

    /// Whether the screen the island hangs from actually has a notch. Without
    /// one there is nothing to route content around, so the island hugs the top
    /// edge instead of leaving a notch-sized band of empty black.
    @Published private(set) var hasNotch = false
    /// Height of the physical notch, or a small lip on screens without one.
    @Published private(set) var topInset: CGFloat = 32
    /// Width of the physical notch, so the collapsed summary can match it and
    /// read as an extension of the notch. Falls back to a compact default on
    /// Macs without a notch.
    private(set) var notchWidth: CGFloat = 180

    /// Top lip used on notch-less screens: just enough that the content doesn't
    /// start flush against the screen edge.
    private static let flatTopInset: CGFloat = 8

    /// Room the island's content leaves at the top. On a notched Mac that's the
    /// notch, a touch less than the full safe-area inset so the island sits
    /// tight to it; elsewhere it's the plain lip.
    var topClearance: CGFloat { hasNotch ? max(topInset - 6, 0) : topInset }

    init(store: SessionStore) {
        self.store = store
        // Plain state changes toast; questions are surfaced by the observer below.
        store.onTransition = { [weak self] info in
            guard let self else { return }
            if info.state == .attention, !info.promptSteps.isEmpty { return }
            if case .question = self.display { return }
            self.showToast(info)
        }
        // Auto-surface / refresh / dismiss the pending question card. Re-evaluate
        // on acknowledgement changes too, so dismissing one clears the notch.
        store.$sessions.combineLatest(store.$acknowledged)
            .sink { [weak self] sessions, _ in
                MainActor.assumeIsolated { self?.onSessionsChanged(sessions) }
            }
            .store(in: &cancellables)
    }

    // MARK: - Lifecycle

    func install() {
        guard panel == nil, let screen = notchedScreen() else { return }
        adoptScreen(screen)

        let p = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: notchWidth, height: topInset),
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: false
        )
        p.isFloatingPanel = true
        p.level = NSWindow.Level(rawValue: NSWindow.Level.mainMenu.rawValue + 2)
        // No window shadow: on a borderless panel it draws a hard rim that reads
        // as a border. The shadow is a soft SwiftUI drop shadow on the content
        // (see NotchContentView), with transparent padding around it for room.
        p.hasShadow = false
        p.isOpaque = false
        p.backgroundColor = .clear
        p.hidesOnDeactivate = false
        p.ignoresMouseEvents = false
        p.collectionBehavior = [.canJoinAllSpaces, .stationary, .fullScreenAuxiliary]
        let host = NSHostingView(rootView: NotchContentView(controller: self, store: store))
        // Don't let the hosting view drive the *window's* content-size extrema.
        // That pass (`updateWindowContentSizeExtremaIfNecessary` → `minSize`)
        // measures the SwiftUI content from inside AppKit's constraint-update
        // cycle; measuring a card whose layout has its own geometry re-dirties
        // the view graph, which posts a fresh `setNeedsUpdateConstraints:` to
        // the window mid-pass. AppKit throws on that re-entrancy and
        // NSApplication turns the exception into a crash — which is what took
        // the island down every time a question card appeared. We size the
        // panel ourselves in `relayout()`, so the intrinsic size (what that
        // measurement reads) is the only option we need.
        host.sizingOptions = [.intrinsicContentSize]
        p.contentView = host
        hosting = host
        panel = p
        relayout()
        p.orderFrontRegardless()

        // Displays come and go (lid closed, monitor plugged in) and the island
        // may end up on a screen with no notch — re-measure and re-place it.
        NotificationCenter.default
            .publisher(for: NSApplication.didChangeScreenParametersNotification)
            .sink { [weak self] _ in
                MainActor.assumeIsolated {
                    guard let self, let screen = self.notchedScreen() else { return }
                    self.adoptScreen(screen)
                    self.relayout()
                }
            }
            .store(in: &cancellables)
    }

    /// Force the list open (menu-bar "Show statuses" action).
    func reveal() {
        expandToList()
        scheduleHide()
    }

    /// Show a session's interactive prompt card. It stays until answered.
    func showQuestion(_ info: SessionInfo) {
        hideTask?.cancel()
        toastTask?.cancel()
        setDisplay(.question(info))
    }

    /// Explicitly dismiss the prompt card (the user tapped ✕). The session stays
    /// waiting in the daemon; we just retract the notch.
    func dismissQuestion() {
        collapse()
    }

    // MARK: - Multi-step prompt draft

    /// Answers picked so far in the open question card, one per step in tab
    /// order.
    ///
    /// It lives here rather than in the card's `@State` for two reasons: the
    /// card is re-created on every store publish, so its own state depends on
    /// SwiftUI preserving view identity across those rebuilds; and the panel
    /// has to re-measure whenever a step changes, which only this object can
    /// do. `setDisplay` clears it, so every card starts from step one.
    @Published private(set) var draftAnswers: [Int] = []

    /// Record a choice for the step on screen: walk to the next question, or
    /// submit the whole set once the last one is answered. Nothing reaches the
    /// PTY until that final pick.
    func pickAnswer(_ index: Int, for session: SessionInfo) {
        let total = session.promptSteps.count
        var next = draftAnswers
        next.append(index)
        Log.log("pick \(index) → step \(next.count)/\(total) for \(session.id)")
        guard next.count < total else {
            store.answer(session, indices: next)
            collapse()
            return
        }
        draftAnswers = next
        // The next step is a different height, and no display change triggers
        // the usual relayout.
        relayout()
    }

    /// Step back to the previous question of a multi-part prompt.
    func undoAnswer() {
        guard !draftAnswers.isEmpty else { return }
        draftAnswers.removeLast()
        relayout()
    }

    // MARK: - Hover

    /// How long the pointer has to linger before the island opens. Without this
    /// dwell, merely sweeping the mouse across the top edge — on the way to a
    /// screen sitting above this one — pops the list open.
    private static let hoverIntentDelay: Duration = .milliseconds(350)

    func hover(_ inside: Bool) {
        // Keep an interactive prompt on screen until it's answered.
        if case .question = display { return }
        hovering = inside
        hoverTask?.cancel()

        guard inside else {
            // Only the hover-revealed list retracts on leave; a toast keeps its
            // own lifecycle, so a pointer passing by doesn't cut it short.
            if display == .list { scheduleHide() }
            return
        }

        // Reveal only once the pointer has stayed put: a quick pass-through
        // leaves (cancelling this task) long before it fires.
        hoverTask = Task { [weak self] in
            try? await Task.sleep(for: Self.hoverIntentDelay)
            guard let self, !Task.isCancelled, self.hovering else { return }
            if case .question = self.display { return }
            // The user reached for the island: drop any toast lifecycle and
            // show the full list, then arm the usual auto-hide.
            self.toastTask?.cancel()
            self.expandToList()
            self.scheduleHide()
        }
    }

    // MARK: - Reactive question handling

    private func onSessionsChanged(_ sessions: [SessionInfo]) {
        guard panel != nil else { return }
        // Don't auto-surface a prompt the user already dismissed in the island.
        let pending = sessions.first {
            $0.state == .attention && !$0.promptSteps.isEmpty
                && !store.isAcknowledged($0)
        }
        switch display {
        case .question(let current):
            // An open card stays until answered or explicitly dismissed with its
            // ✕ (dismissQuestion), so acknowledgement alone doesn't collapse it —
            // that keeps "tap the row to open the card" from closing it instantly.
            if let cur = sessions.first(where: { $0.id == current.id }),
               cur.state == .attention, !cur.promptSteps.isEmpty {
                // Refresh only when the prompt itself changes (not on every
                // activity/ts tick) to avoid constant relayouts. Compare every
                // step: two prompts can open on the same first question.
                if cur.promptSteps != current.promptSteps { setDisplay(.question(cur)) }
            } else if let p = pending {
                setDisplay(.question(p)) // that one resolved; show the next
            } else {
                collapse()
            }
        case .collapsed:
            if let p = pending {
                showQuestion(p)
            } else {
                // Resize the collapsed panel to fit the summary pill as the
                // working/waiting counts change (a no-op when the size is stable).
                relayout()
            }
        case .toast, .miniToast:
            if let p = pending { showQuestion(p) }
        case .list:
            break // don't interrupt an explicit hover-list
        }
    }

    /// A state change announces itself as a full toast for 1 s, then shrinks to
    /// a compact pill for 3 s, then disappears — unless the user hovers (which
    /// opens the list) or a question takes over in the meantime.
    private func showToast(_ info: SessionInfo) {
        hideTask?.cancel()
        toastTask?.cancel()
        setDisplay(.toast(info))
        toastTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(1))
            guard let self, !Task.isCancelled else { return }
            guard case .toast(let cur) = self.display, cur.id == info.id else { return }
            self.setDisplay(.miniToast(cur))
            try? await Task.sleep(for: .seconds(3))
            guard !Task.isCancelled else { return }
            guard case .miniToast = self.display, !self.hovering else { return }
            self.collapse()
        }
    }

    private func expandToList() {
        toastTask?.cancel()
        if display != .list { setDisplay(.list) }
    }

    private func collapse() {
        if display != .collapsed { setDisplay(.collapsed) }
    }

    private func setDisplay(_ new: IslandDisplay) {
        // Any card that opens starts on its first step. The only time an open
        // card is re-set is when its prompt actually changed (see
        // onSessionsChanged), and that prompt deserves fresh answers.
        if !draftAnswers.isEmpty { draftAnswers = [] }
        // Retracting to the notch should be quick and clean; growing/morphing
        // gets the bouncy bubble. Drive the SwiftUI transition and the window
        // frame with matching curves.
        let collapsing = (new == .collapsed)
        let curve: Animation = collapsing
            ? .easeIn(duration: 0.22)
            : .spring(response: 0.40, dampingFraction: 0.62)
        withAnimation(curve) { display = new }
        relayout(collapsing: collapsing)
    }

    /// How long a toast / revealed list stays before auto-collapsing.
    private static let autoHideDelay: Duration = .seconds(1)

    private func scheduleHide() {
        hideTask?.cancel()
        hideTask = Task { [weak self] in
            try? await Task.sleep(for: Self.autoHideDelay)
            guard let self, !Task.isCancelled else { return }
            if case .question = self.display { return } // never auto-dismiss a prompt
            if self.hovering {
                self.expandToList()
                self.scheduleHide()
            } else {
                self.collapse()
            }
        }
    }

    // MARK: - Geometry

    private func notchedScreen() -> NSScreen? {
        NSScreen.screens.first(where: { $0.safeAreaInsets.top > 0 }) ?? NSScreen.main
    }

    /// Measure the screen the island hangs from. With a notch the island has to
    /// route its content around it; without one (external display, notch-less
    /// Mac) it only keeps a small lip, so toasts don't float below a band of
    /// empty black.
    private func adoptScreen(_ screen: NSScreen) {
        hasNotch = screen.safeAreaInsets.top > 0
        topInset = hasNotch ? screen.safeAreaInsets.top : Self.flatTopInset
        // The notch is the gap between the two usable menu-bar areas. On a
        // notch-less Mac these are nil — fall back to the compact default width.
        if hasNotch, let left = screen.auxiliaryTopLeftArea, let right = screen.auxiliaryTopRightArea {
            notchWidth = max(right.minX - left.maxX, 120)
        } else {
            notchWidth = 180
        }
    }

    /// Measured size of the SwiftUI content. `fittingSize` is derived from the
    /// constraints the hosting view installs, and with `sizingOptions` trimmed
    /// to the intrinsic size that's the value to fall back on. Non-finite or
    /// sentinel components (`noIntrinsicMetric` is -1) are dropped: they would
    /// otherwise reach `setFrame`, which throws on a NaN dimension.
    private nonisolated static func contentSize(of hosting: NSView) -> NSSize {
        let fitting = hosting.fittingSize
        let intrinsic = hosting.intrinsicContentSize
        func best(_ a: CGFloat, _ b: CGFloat) -> CGFloat {
            max(a.isFinite ? a : 0, b.isFinite ? b : 0)
        }
        return NSSize(
            width: best(fitting.width, intrinsic.width),
            height: best(fitting.height, intrinsic.height)
        )
    }

    /// Size the panel to the SwiftUI content. SwiftUI commits the new layout on
    /// the next runloop tick, so measure then. Growing bounces like a bubble;
    /// retracting (`collapsing`) uses a quick ease-in so it disappears cleanly
    /// without an overshoot that would clip the card against the shrinking window.
    private func relayout(collapsing: Bool = false) {
        DispatchQueue.main.async { [weak self] in
            guard let self, let panel = self.panel, let hosting = self.hosting,
                let screen = self.notchedScreen() else { return }
            hosting.layoutSubtreeIfNeeded()
            let fit = Self.contentSize(of: hosting)
            let w = max(fit.width, 1)
            let h = max(fit.height, self.topInset)
            let frame = screen.frame
            let target = NSRect(x: frame.midX - w / 2, y: frame.maxY - h, width: w, height: h)

            // Only animate once the panel is on screen; the very first layout
            // must land instantly (no from-zero slide).
            guard panel.isVisible, panel.frame != target else {
                panel.setFrame(target, display: true)
                return
            }
            NSAnimationContext.runAnimationGroup { ctx in
                if collapsing {
                    ctx.duration = 0.22
                    ctx.timingFunction = CAMediaTimingFunction(name: .easeIn)
                } else {
                    ctx.duration = 0.5
                    // Pronounced overshoot (y ≫ 1 mid-curve): the panel inflates
                    // past its target and settles, like a bubble bouncing in.
                    ctx.timingFunction = CAMediaTimingFunction(controlPoints: 0.34, 1.45, 0.28, 1.0)
                }
                ctx.allowsImplicitAnimation = true
                panel.animator().setFrame(target, display: true)
            }
        }
    }
}
