import SwiftUI
import AppKit

@main
struct SbxwIslandApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate

    var body: some Scene {
        MenuBarExtra {
            MenuContent(store: delegate.store)
        } label: {
            MenuBarLabel(store: delegate.store)
        }
        .menuBarExtraStyle(.window)
    }
}

/// Owns the app's long-lived objects and hides the Dock icon so sbxw Island
/// lives purely in the menu bar / notch.
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate, ObservableObject {
    /// The live delegate, for the menu items that need to reach it.
    ///
    /// Not `NSApp.delegate as? AppDelegate`: under `@NSApplicationDelegateAdaptor`
    /// that cast comes back **nil** at runtime, even though AppKit is calling our
    /// lifecycle methods — SwiftUI does not necessarily hand AppKit this very
    /// object. Every menu item that went through it was silently doing nothing:
    /// "Show statuses on the notch", "Settings…", and the `store.restart()` that
    /// is supposed to follow a URL change. The app keeps its own handle instead.
    static var shared: AppDelegate?

    let store = SessionStore()
    /// Cross-sandbox information requests. Its own store and its own SSE
    /// connection: a relay request belongs to two sessions at once and outlives
    /// either, so it is not something `SessionStore` could hold a row for.
    let relay = RelayStore()
    private lazy var notch = NotchController(store: store, relay: relay)
    /// Settings live in a real window, not a sheet. `MenuBarExtra(.window)`
    /// closes its popover the moment it loses focus — which is exactly when a
    /// sheet appears — so the sheet outlived the view that owned it: its
    /// `dismiss()` had nothing left to close and the `@State` flag went with the
    /// torn-down hierarchy. The only way out was relaunching the app.
    private var settingsWindow: NSWindow?

    func applicationDidFinishLaunching(_ notification: Notification) {
        // This runs on the instance AppKit actually drives, so it's the one the
        // menu should talk to.
        AppDelegate.shared = self
        NSApp.setActivationPolicy(.accessory)
        notch.install()   // wires store.onTransition + relay.onArrival before events start
        store.start()
        relay.start()
    }

    func revealNotch() {
        notch.reveal()
    }

    /// Open (or raise) the settings window. An `.accessory` app has to activate
    /// itself for the window to take keys, or the text field can't be typed in.
    func showSettings() {
        Log.log("showSettings: existing=\(settingsWindow != nil) screens=\(NSScreen.screens.count)")
        if settingsWindow == nil {
            let window = NSWindow(
                contentRect: NSRect(x: 0, y: 0, width: 400, height: 130),
                styleMask: [.titled, .closable],
                backing: .buffered,
                defer: false
            )
            window.title = "sbxw Island Settings"
            // We hold the only reference; without this the window would be
            // deallocated on close and raising it again would crash.
            window.isReleasedWhenClosed = false
            window.delegate = self
            let hosting = NSHostingView(
                rootView: SettingsView(onClose: { [weak self] in self?.closeSettings() })
            )
            window.contentView = hosting
            // Let the content ask for a size, but never below a usable one:
            // `fittingSize` comes back zero until the hosting view has laid out,
            // and a 0×0 window is simply nothing on screen.
            let fitting = hosting.fittingSize
            window.setContentSize(NSSize(
                width: max(fitting.width, 400),
                height: max(fitting.height, 140)
            ))
            window.center()
            settingsWindow = window
        }
        NSApp.activate(ignoringOtherApps: true)
        settingsWindow?.makeKeyAndOrderFront(nil)
        if let w = settingsWindow {
            Log.log("showSettings: frame=\(NSStringFromRect(w.frame)) visible=\(w.isVisible) key=\(w.isKeyWindow) level=\(w.level.rawValue) alpha=\(w.alphaValue)")
        }
    }

    func closeSettings() {
        settingsWindow?.close()
    }
}

extension AppDelegate: NSWindowDelegate {
    /// Drop the window when it closes — by the ✕, by Cancel, or by ⌘W — so the
    /// next "Settings…" builds a fresh one seeded with the current URL rather
    /// than re-showing a stale field.
    func windowWillClose(_ notification: Notification) {
        guard (notification.object as? NSWindow) === settingsWindow else { return }
        // Dropped on the next turn of the run loop: we hold the only strong
        // reference, and releasing it while the window is still closing would
        // pull the rug from under the very call we're inside.
        DispatchQueue.main.async { [weak self] in self?.settingsWindow = nil }
    }
}

/// The menu-bar icon. Reflects the most urgent state across all sessions:
/// waiting for input (bell badge) > your turn to reply (speech bubble) >
/// working > idle.
struct MenuBarLabel: View {
    @ObservedObject var store: SessionStore

    var body: some View {
        Image(systemName: symbol)
    }

    private var symbol: String {
        if store.needsAttention { return "bell.badge.fill" }
        // A quieter cue than the bell: Claude replied and it's your move (an
        // inline question has no explicit prompt to nag about).
        if store.sessions.contains(where: { $0.awaitingReply && !store.isAcknowledged($0) }) {
            return "bubble.left.fill"
        }
        if store.sessions.contains(where: { $0.state == .working }) { return "circle.hexagongrid.fill" }
        return "square.grid.2x2"
    }
}

/// The popover shown when the menu-bar item is clicked. Styled as a dark card
/// so it matches the notch island (both use white-on-black content).
struct MenuContent: View {
    @ObservedObject var store: SessionStore

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text("Sessions")
                    .font(.headline)
                    .foregroundStyle(.white)
                Spacer()
                Circle()
                    .fill(store.connected ? Color.green : Color.red)
                    .frame(width: 7, height: 7)
                    .help(store.connected ? "Connected to sbxw" : "sbxw not reachable")
            }
            .padding(.horizontal, 8)

            StateSummary(store: store)
                .padding(.horizontal, 8)

            IslandView(store: store)

            Divider().overlay(Color.white.opacity(0.1))

            VStack(alignment: .leading, spacing: 2) {
                MenuButton("Show statuses on the notch") {
                    AppDelegate.shared?.revealNotch()
                }
                MenuButton("Settings…") { AppDelegate.shared?.showSettings() }
                MenuButton("Quit sbxw Island") { NSApp.terminate(nil) }
            }
            .padding(.horizontal, 4)
        }
        .padding(12)
        .frame(width: 320)
        .background(Color.black.opacity(0.92))
    }
}

/// A plain white-on-dark row button for the popover menu.
struct MenuButton: View {
    let title: String
    let action: () -> Void
    init(_ title: String, action: @escaping () -> Void) {
        self.title = title
        self.action = action
    }
    var body: some View {
        Button(action: action) {
            Text(title)
                .font(.system(size: 12))
                .foregroundStyle(.white.opacity(0.9))
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.vertical, 5)
                .padding(.horizontal, 8)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

/// A one-line "N working · N waiting · N idle" tally.
struct StateSummary: View {
    @ObservedObject var store: SessionStore

    var body: some View {
        HStack(spacing: 12) {
            tally(.working, "working")
            tally(.attention, "waiting")
            tally(.idle, "idle")
            Spacer()
        }
        .font(.system(size: 11))
        .foregroundStyle(.white.opacity(0.6))
    }

    private func count(_ state: SessionState) -> Int {
        store.sessions.filter { $0.state == state }.count
    }

    private func tally(_ state: SessionState, _ name: String) -> some View {
        HStack(spacing: 4) {
            Circle().fill(state.dotColor).frame(width: 7, height: 7)
            Text("\(count(state)) \(name)")
        }
    }
}

/// Lets the user point the app at a non-default daemon URL / port.
///
/// Closing is the window's job, not the view's: `@Environment(\.dismiss)` only
/// means something to a presentation SwiftUI itself put on screen, and this is a
/// plain `NSWindow` the delegate owns.
struct SettingsView: View {
    let onClose: () -> Void
    @State private var baseURL = Config.baseURL

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("sbxw daemon URL").font(.headline)
            TextField("http://sbxw.localhost:7681", text: $baseURL)
                .textFieldStyle(.roundedBorder)
                .frame(width: 340)
            HStack {
                Spacer()
                // Cancel discards the edit by simply never writing it back —
                // `baseURL` is view state until Save says otherwise.
                Button("Cancel", role: .cancel) { onClose() }
                    .keyboardShortcut(.cancelAction)
                Button("Save") {
                    Config.baseURL = baseURL.trimmingCharacters(in: .whitespaces)
                    // Both streams follow the daemon URL; leaving the relay one
                    // pointed at the old address would keep the island's
                    // sessions and its requests on two different daemons.
                    AppDelegate.shared?.store.restart()
                    AppDelegate.shared?.relay.restart()
                    onClose()
                }
                .keyboardShortcut(.defaultAction)
            }
        }
        .padding(20)
    }
}
