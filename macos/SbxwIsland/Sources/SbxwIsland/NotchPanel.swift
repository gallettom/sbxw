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
    private var cancellables = Set<AnyCancellable>()

    /// Height of the physical notch (or the menu bar on non-notched Macs).
    private(set) var topInset: CGFloat = 32
    /// Width of the physical notch, so the collapsed summary can match it and
    /// read as an extension of the notch. Falls back to a compact default on
    /// Macs without a notch.
    private(set) var notchWidth: CGFloat = 180

    init(store: SessionStore) {
        self.store = store
        // Plain state changes toast; questions are surfaced by the observer below.
        store.onTransition = { [weak self] info in
            guard let self else { return }
            if info.state == .attention, info.question != nil { return }
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
        topInset = screen.safeAreaInsets.top > 0
            ? screen.safeAreaInsets.top
            : NSStatusBar.system.thickness

        // The notch is the gap between the two usable menu-bar areas. On a
        // notch-less Mac these are nil — keep the compact default width.
        if let left = screen.auxiliaryTopLeftArea, let right = screen.auxiliaryTopRightArea {
            notchWidth = max(right.minX - left.maxX, 120)
        }

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
        p.contentView = host
        hosting = host
        panel = p
        relayout()
        p.orderFrontRegardless()
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

    /// Called after the user picks an answer; the incoming state event will
    /// re-toast/collapse as needed.
    func dismissAfterAnswer() {
        collapse()
    }

    /// Explicitly dismiss the prompt card (the user tapped ✕). The session stays
    /// waiting in the daemon; we just retract the notch.
    func dismissQuestion() {
        collapse()
    }

    // MARK: - Hover

    func hover(_ inside: Bool) {
        // Keep an interactive prompt on screen until it's answered.
        if case .question = display { return }
        hovering = inside
        if inside {
            // The user reached for the island: cancel any toast lifecycle and
            // show the full list.
            toastTask?.cancel()
            expandToList()
        }
        // On leave, scheduleHide collapses after `autoHideDelay` (1 s).
        scheduleHide()
    }

    // MARK: - Reactive question handling

    private func onSessionsChanged(_ sessions: [SessionInfo]) {
        guard panel != nil else { return }
        // Don't auto-surface a prompt the user already dismissed in the island.
        let pending = sessions.first {
            $0.state == .attention && $0.question != nil && !store.acknowledged.contains($0.id)
        }
        switch display {
        case .question(let current):
            // An open card stays until answered or explicitly dismissed with its
            // ✕ (dismissQuestion), so acknowledgement alone doesn't collapse it —
            // that keeps "tap the row to open the card" from closing it instantly.
            if let cur = sessions.first(where: { $0.id == current.id }),
               cur.state == .attention, cur.question != nil {
                // Refresh only when the prompt itself changes (not on every
                // activity/ts tick) to avoid constant relayouts.
                if cur.question != current.question { setDisplay(.question(cur)) }
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

    /// Size the panel to the SwiftUI content. SwiftUI commits the new layout on
    /// the next runloop tick, so measure then. Growing bounces like a bubble;
    /// retracting (`collapsing`) uses a quick ease-in so it disappears cleanly
    /// without an overshoot that would clip the card against the shrinking window.
    private func relayout(collapsing: Bool = false) {
        DispatchQueue.main.async { [weak self] in
            guard let self, let panel = self.panel, let hosting = self.hosting,
                let screen = self.notchedScreen() else { return }
            hosting.layoutSubtreeIfNeeded()
            let fit = hosting.fittingSize
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
