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

extension IslandDisplay {
    /// Whether this mode owns the pointer outright: the whole panel takes mouse
    /// events, and its content's `.onHover` drives the auto-hide.
    ///
    /// The list and a question card do. Both toasts are read-only: they announce,
    /// they don't offer. The collapsed bubble is the in-between case and is not
    /// covered here — it takes clicks only over the part of the panel it actually
    /// draws, and only when it draws anything (see
    /// `NotchController.updateClickThrough`).
    ///
    /// All of it in service of one rule: the panel hangs over the menu bar of
    /// whatever app you are working in, and a window that eats clicks it has no
    /// use for is a window in your way.
    var isInteractive: Bool {
        switch self {
        case .list, .question: return true
        case .collapsed, .toast, .miniToast: return false
        }
    }
}

/// A borderless `NSPanel` refuses key status, which makes any text field inside
/// it impossible to type into — including the island's chat composer. The
/// `.nonactivatingPanel` style already means "accept keys without activating the
/// app", so opting in here is the whole fix.
final class KeyablePanel: NSPanel {
    override var canBecomeKey: Bool { true }
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
    /// The chat composer holds keyboard focus: the list must stay put until the
    /// user is done writing (see `setComposerActive`).
    private var composerActive = false
    /// At least one row has its accordion open (see `setRowsExpanded`).
    private var rowsExpanded = false
    private var hideTask: Task<Void, Never>?
    private var toastTask: Task<Void, Never>?
    /// Pending hover-intent reveal (see `hoverIntentDelay`).
    private var hoverTask: Task<Void, Never>?
    private var cancellables = Set<AnyCancellable>()
    /// Pointer-tracking monitors (see `trackPointer`). Kept for as long as the
    /// panel itself, which is the life of the process.
    private var mouseMonitors: [Any] = []

    /// Whether the screen the island hangs from actually has a notch. Without
    /// one there is nothing to route content around, so the island hugs the top
    /// edge instead of leaving a notch-sized band of empty black.
    @Published private(set) var hasNotch = false
    /// Height of the physical notch, or a small lip on screens without one.
    @Published private(set) var topInset: CGFloat = 32
    /// Width of the physical notch: the panel's first size, and the width of the
    /// band that reveals the island on hover (see `revealBand`). Falls back to a
    /// compact default on Macs without a notch.
    private var notchWidth: CGFloat = 180

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
        // The user went to read this sandbox's terminal in the browser. An open
        // card survives a plain acknowledgement on purpose (see
        // `onSessionsChanged`) — that guard exists so opening a row from the
        // island doesn't close the card you just opened. It must not apply here:
        // this acknowledgement comes from the user leaving for the other window,
        // and a card still hanging over the menu bar is then pointing at a
        // question they are already reading in full.
        store.onWatched = { [weak self] sandbox in
            guard let self else { return }
            switch self.display {
            case .question(let current), .toast(let current), .miniToast(let current):
                if current.sandbox == sandbox { self.collapse() }
            default:
                break
            }
        }
        // Auto-surface / refresh / dismiss the pending question card. Re-evaluate
        // on acknowledgement changes too, so dismissing one clears the notch.
        // …and on hushes, which retire the collapsed bubble the same way.
        store.$sessions.combineLatest(store.$acknowledged, store.$hushedWorking)
            .sink { [weak self] sessions, _, _ in
                MainActor.assumeIsolated { self?.onSessionsChanged(sessions) }
            }
            .store(in: &cancellables)
        // Composers change the list's height as they swap between the ＋ row, the
        // field, a spinner and an error line — the one at the bottom and every
        // open row's alike. The panel is sized by hand (see `install`), so
        // nothing else would follow that.
        store.$chatPush
            .sink { [weak self] _ in
                MainActor.assumeIsolated {
                    guard let self, self.display == .list else { return }
                    self.relayout()
                }
            }
            .store(in: &cancellables)
    }

    // MARK: - Lifecycle

    func install() {
        guard panel == nil, let screen = notchedScreen() else { return }
        adoptScreen(screen)

        let p = KeyablePanel(
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
        // Click-through until there is something to click (see
        // `updateClickThrough`). It starts collapsed with no session, so it
        // starts transparent.
        p.ignoresMouseEvents = true
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
        trackPointer()

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

    /// The collapsed bubble was clicked: open the list at once, skipping the
    /// hover dwell the pointer would otherwise have to sit through.
    ///
    /// The pointer is on the island by construction, so say so. Without it the
    /// one-second auto-hide can retract the list the user is looking straight at:
    /// while collapsed the hover flag follows the *notch band* (`revealBand`),
    /// which the bubble is far wider than, so a click on its shoulder leaves
    /// `hovering` false with only SwiftUI's `.onHover` on the just-swapped-in list
    /// to correct it in time.
    func revealFromPill() {
        hovering = true
        reveal()
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

    /// The content grew or shrank on its own (a row's reply accordion opening,
    /// say) rather than because the display mode changed. The panel is sized by
    /// hand, so nothing else would follow it.
    ///
    /// `relayout` already measures on the next runloop tick, which is exactly
    /// the wait this needs for SwiftUI to commit the new height first.
    func contentHeightChanged() {
        relayout()
    }

    /// The chat composer took or gave up keyboard focus.
    ///
    /// Two things have to happen for typing in the notch to work at all. The
    /// panel must become key — a borderless one refuses by default, which is
    /// what `KeyablePanel` overrides — and it must stop auto-retracting, or the
    /// list would slide away mid-sentence (`scheduleHide` fires a second after
    /// the pointer settles). Keying is deliberately tied to *focus* rather than
    /// to the list being open, so merely hovering the island never steals keys
    /// from whatever you were working in.
    func setComposerActive(_ active: Bool) {
        composerActive = active
        if active {
            hideTask?.cancel()
            panel?.makeKeyAndOrderFront(nil)
        } else {
            // Hand keys back and resume the usual retract timer.
            panel?.resignKey()
            scheduleHide()
        }
        // Expanding the field (or folding it back) changes the content height.
        relayout()
    }

    /// A row's accordion opened or closed.
    ///
    /// Only the auto-hide leash changes: an open drawer is a deliberate "I'm
    /// reading this", and one second after the pointer leaves is not long enough
    /// to have been worth the click. Deliberately *not* a pin — that belongs to a
    /// focused composer alone, or a forgotten open row would leave the island
    /// hanging under the notch for good.
    func setRowsExpanded(_ expanded: Bool) {
        rowsExpanded = expanded
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

    /// Watch the pointer ourselves, because in the modes that matter the panel
    /// cannot.
    ///
    /// A click-through window (`ignoresMouseEvents`) receives *no* mouse events,
    /// so SwiftUI's `.onHover` inside it never fires — the reveal has to come from
    /// somewhere else. Neither monitor asks for Accessibility: that is only
    /// required to monitor the *keyboard*.
    private func trackPointer() {
        // Moves reveal. Deliberately no `.mouseDragged`: while a button is held
        // the pointer is doing something — holding a menu open, dragging a
        // selection — and the island staying down through it is exactly right.
        monitorPointer([.mouseMoved]) { [weak self] point in
            self?.pointerMoved(to: point)
        }
        // Clicks retract (see `clickedOutside`).
        monitorPointer([.leftMouseDown, .rightMouseDown, .otherMouseDown]) { [weak self] point in
            self?.clickedOutside(point)
        }
    }

    /// Report the pointer's screen position for every event matching `mask`.
    ///
    /// Two monitors, because each is blind to what the other sees: the global one
    /// only reports events headed for *other* applications, the local one only
    /// those headed for ours. The island has to react to both — a click in another
    /// app and a click on our own menu-bar icon are the same gesture as far as the
    /// notch is concerned.
    private func monitorPointer(
        _ mask: NSEvent.EventTypeMask,
        report: @escaping @MainActor (NSPoint) -> Void
    ) {
        // Read the location now, at event time, rather than in the hop below,
        // where the pointer may already have moved on.
        let deliver = {
            let location = NSEvent.mouseLocation
            DispatchQueue.main.async { MainActor.assumeIsolated { report(location) } }
        }
        if let global = NSEvent.addGlobalMonitorForEvents(matching: mask, handler: { _ in deliver() }) {
            mouseMonitors.append(global)
        }
        if let local = NSEvent.addLocalMonitorForEvents(matching: mask, handler: { event in
            deliver()
            return event // pass it on: we are observing, not intercepting
        }) {
            mouseMonitors.append(local)
        }
    }

    /// A click landed somewhere else while the list was open: retract at once.
    ///
    /// The auto-hide timer is for a pointer that *wandered*; a click elsewhere is
    /// a decision, and taking a second to honour it is exactly what makes the
    /// island feel like it is in the way.
    ///
    /// Scoped to the list. A question card stays until it is answered or its ✕ is
    /// used — that is deliberate everywhere else in this file, and an errant click
    /// must not lose a prompt. The toasts are click-through and run their own
    /// three-second course.
    ///
    /// A focused composer is no exception, even though it otherwise pins the list
    /// open: while it has focus the panel is *key*, so leaving it up over the app
    /// you just clicked into would go on swallowing your keystrokes. The
    /// half-written message is lost — the same as when the auto-hide fires — and
    /// the alternative is worse. Its focus flag has to be cleared by hand: the
    /// field is torn down with the view, which reports no focus change on the way
    /// out, and a stale `composerActive` would pin every later list open for good.
    private func clickedOutside(_ screenPoint: NSPoint) {
        guard display == .list, let panel, !panel.frame.contains(screenPoint) else { return }
        if composerActive {
            composerActive = false
            panel.resignKey()
        }
        // The pointer is elsewhere by definition, and a dwell still counting down
        // would otherwise re-open what we are closing.
        hovering = false
        hoverTask?.cancel()
        collapse()
    }

    /// The pointer moved.
    private func pointerMoved(to screenPoint: NSPoint) {
        guard let panel else { return }
        updateClickThrough(at: screenPoint)
        if display.isInteractive {
            // The content's `.onHover` owns this mode — the whole surface holds
            // the island open, margin included, and second-guessing it here would
            // fight it. Only one thing is worth saying: a pointer that is plainly
            // outside the window is not hovering it. Without that backstop a
            // missed `mouseExited` leaves `hovering` stuck true, `scheduleHide`
            // re-arms on it forever, and the island hangs under the notch — the
            // one failure mode this panel must never have.
            if hovering, !panel.frame.contains(screenPoint) { hover(false) }
            return
        }
        hover(revealBand(of: panel).contains(screenPoint))
    }

    /// The band whose hover reveals the island, in screen coordinates: the
    /// panel's full height, but only the notch's width, centred on it.
    ///
    /// The collapsed bubble hangs 280 pt wide (332 counting the invisible margin
    /// its shadow needs) over the menu bar of whatever app you are working in, and
    /// a pointer crossing the top edge on the way to *that* app's menus used to
    /// unfold the island in your face. Aiming at the notch is the gesture; passing
    /// beside it is not.
    ///
    /// Only the width narrows. The full height matters: with nothing running the
    /// collapsed content is a 2 pt hairline, and the shadow's margin below it is
    /// the whole reason there is anything to hover.
    private func revealBand(of panel: NSPanel) -> NSRect {
        let frame = panel.frame
        let width = min(notchWidth, frame.width)
        return NSRect(
            x: frame.midX - width / 2, y: frame.minY,
            width: width, height: frame.height
        )
    }

    /// The bubble the collapsed island actually *draws*, in screen coordinates.
    ///
    /// The panel is bigger than what you see: the content carries transparent
    /// padding on three sides so its drop shadow isn't clipped by the
    /// content-sized window (`NotchContentView.shadowPad`). That margin is
    /// nothing to click, so it stays out of the hit area.
    private func pillRect(of panel: NSPanel) -> NSRect {
        let pad = NotchContentView.shadowPad
        let frame = panel.frame
        return NSRect(
            x: frame.minX + pad, y: frame.minY + pad,
            width: max(frame.width - 2 * pad, 0),
            height: max(frame.height - pad, 0)
        )
    }

    // MARK: - Click-through

    /// Decide whether the panel swallows mouse events, from the current mode and
    /// where the pointer is.
    ///
    /// The list and a question card take everything. The collapsed island used to
    /// take nothing — but once it grew a summary bubble that was wrong: a black
    /// pill naming the session Claude is working on sat there fully drawn and
    /// perfectly unclickable, and clicks meant for it fell through to the menu bar
    /// of the app behind. So while the bubble is drawn (`store.summaryLead`) the
    /// panel accepts the pointer, and only over the bubble itself: the shadow
    /// margin around it, and the hairline the pill shrinks to when nothing is
    /// running, must go on passing clicks through to whatever is underneath.
    ///
    /// Pointer-position-dependent because that hit area is a rectangle inside the
    /// window, and a window is all-or-nothing about `ignoresMouseEvents`. The two
    /// monitors in `trackPointer` keep it current from either side: while we are
    /// click-through the moves belong to the app below (global monitor), and once
    /// we are not they belong to us (local monitor).
    private func updateClickThrough(
        for display: IslandDisplay? = nil,
        at screenPoint: NSPoint = NSEvent.mouseLocation
    ) {
        guard let panel else { return }
        let display = display ?? self.display
        let takesEvents: Bool
        if display.isInteractive {
            takesEvents = true
        } else if display == .collapsed, store.summaryLead != nil {
            takesEvents = pillRect(of: panel).contains(screenPoint)
        } else {
            takesEvents = false // a toast announces; it doesn't offer
        }
        if panel.ignoresMouseEvents == takesEvents { panel.ignoresMouseEvents = !takesEvents }
    }

    /// Hover reported by the panel's own content (`NotchContentView.onHover`).
    ///
    /// Ignored while collapsed: that surface covers the shadow margin too, and it
    /// is live only when the pointer sits on the bubble — neither matches the
    /// notch-width band that decides the reveal, which `pointerMoved` owns and
    /// applies uniformly whether or not the panel happens to be taking events.
    func hoverFromContent(_ inside: Bool) {
        guard display != .collapsed else { return }
        hover(inside)
    }

    func hover(_ inside: Bool) {
        // Reported on every mouse move by `pointerMoved`, not just on a crossing,
        // and nothing below this line is idempotent: re-entering would cancel and
        // restart the dwell timer on every twitch of the pointer, so the island
        // would open only once the pointer had stopped dead. Changes only.
        guard inside != hovering else { return }
        // Record where the pointer is even when we go on to ignore it. The flag
        // is read later by the toast and auto-hide timers, and the early returns
        // below used to skip this line — so a question card appearing between
        // the pointer entering and leaving pinned `hovering` to `true` long
        // after the pointer had gone, and every later auto-collapse saw a hover
        // that wasn't happening and declined to retract.
        hovering = inside
        // Keep an interactive prompt on screen until it's answered.
        if case .question = display { return }
        // Likewise a half-written chat: the pointer wandering off must not take
        // the field (and the text in it) with it.
        if composerActive { return }
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
            guard case .miniToast = self.display else { return }
            if self.hovering {
                // Defer, don't abandon. Bailing out here (the old `!hovering`
                // guard) consumed the pill's only timer, and nothing re-armed
                // it — `hover(false)` reschedules for `.list` alone — so a
                // pointer merely resting near the notch when the three seconds
                // ran out left the mini pill on screen for good.
                self.scheduleHide()
            } else {
                self.collapse()
            }
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
        // Leaving the list tears its rows down (NotchContentView keys the
        // content on the mode), so whatever they had open goes with them. Without
        // this the flag would stay true and every later list would inherit the
        // longer leash.
        if new != .list { rowsExpanded = false }
        // Take mouse events only where there is something to click, so an
        // announcement can't swallow a click meant for the app underneath (see
        // `updateClickThrough`). Set before the animation, not after: the panel is
        // over the menu bar for the whole of it — so the mode is handed over
        // explicitly rather than read from `display`, which is only assigned
        // (inside the animation) below.
        updateClickThrough(for: new)
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

    /// The same, for a list with an open row accordion: long enough to finish
    /// reading a reply after the pointer has wandered off, still short enough
    /// that the island always comes back on its own.
    private static let expandedHideDelay: Duration = .seconds(5)

    private func scheduleHide() {
        hideTask?.cancel()
        hideTask = Task { [weak self] in
            let delay = self?.rowsExpanded == true
                ? Self.expandedHideDelay
                : Self.autoHideDelay
            try? await Task.sleep(for: delay)
            guard let self, !Task.isCancelled else { return }
            if case .question = self.display { return } // never auto-dismiss a prompt
            if self.composerActive { return } // nor a chat being written
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
    @MainActor
    private static func contentSize(of hosting: NSView) -> NSSize {
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
            // This block only ever runs on the main queue, so state the
            // isolation instead of marking the measurement `nonisolated` —
            // reading NSView geometry from a non-isolated context is an error
            // under Swift 6, and the deferral to the next runloop tick (which
            // is what lets SwiftUI commit its layout first) has to stay.
            MainActor.assumeIsolated {
                guard let self, let panel = self.panel, let hosting = self.hosting,
                    let screen = self.notchedScreen() else { return }
                hosting.layoutSubtreeIfNeeded()
                let fit = Self.contentSize(of: hosting)
                let w = max(fit.width, 1)
                let h = max(fit.height, self.topInset)
                let frame = screen.frame
                let target = NSRect(x: frame.midX - w / 2, y: frame.maxY - h, width: w, height: h)
                // The hit area of the collapsed bubble is derived from the panel
                // frame, and the frame is what just changed (the pill grows and
                // shrinks with its lead session's task).
                defer { self.updateClickThrough() }

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
                        // Pronounced overshoot (y ≫ 1 mid-curve): the panel
                        // inflates past its target and settles, like a bubble
                        // bouncing in.
                        ctx.timingFunction = CAMediaTimingFunction(
                            controlPoints: 0.34, 1.45, 0.28, 1.0)
                    }
                    ctx.allowsImplicitAnimation = true
                    panel.animator().setFrame(target, display: true)
                }
            }
        }
    }
}
