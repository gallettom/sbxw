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
    let store = SessionStore()
    private lazy var notch = NotchController(store: store)

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.accessory)
        notch.install()   // wires store.onTransition before events start
        store.start()
    }

    func revealNotch() {
        notch.reveal()
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
        if store.sessions.contains(where: { $0.awaitingReply }) { return "bubble.left.fill" }
        if store.sessions.contains(where: { $0.state == .working }) { return "circle.hexagongrid.fill" }
        return "square.grid.2x2"
    }
}

/// The popover shown when the menu-bar item is clicked. Styled as a dark card
/// so it matches the notch island (both use white-on-black content).
struct MenuContent: View {
    @ObservedObject var store: SessionStore
    @State private var showSettings = false

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
                    (NSApp.delegate as? AppDelegate)?.revealNotch()
                }
                MenuButton("Settings…") { showSettings = true }
                MenuButton("Quit sbxw Island") { NSApp.terminate(nil) }
            }
            .padding(.horizontal, 4)
        }
        .padding(12)
        .frame(width: 320)
        .background(Color.black.opacity(0.92))
        .sheet(isPresented: $showSettings) {
            SettingsView()
        }
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
struct SettingsView: View {
    @Environment(\.dismiss) private var dismiss
    @State private var baseURL = Config.baseURL

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("sbxw daemon URL").font(.headline)
            TextField("http://sbxw.localhost:7681", text: $baseURL)
                .textFieldStyle(.roundedBorder)
                .frame(width: 340)
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }
                Button("Save") {
                    Config.baseURL = baseURL.trimmingCharacters(in: .whitespaces)
                    (NSApp.delegate as? AppDelegate)?.store.restart()
                    dismiss()
                }
                .keyboardShortcut(.defaultAction)
            }
        }
        .padding(20)
    }
}
