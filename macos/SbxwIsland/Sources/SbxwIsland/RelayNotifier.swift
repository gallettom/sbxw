import AppKit
import Foundation
import UserNotifications

/// The Mac's system notification for a relay request that waits for the
/// person, and for its reminder after 10 minutes (D17 of the reliability
/// audit, R5).
///
/// Only the island posts these: it is a bundled app (`app.sbxw.island`),
/// which `UNUserNotificationCenter` needs, and it already follows every
/// request. The desktop app shows the request in its own window instead.
/// When the island is not running, there is no system notification: the
/// desktop's badge and notice, and the phone's push, still say it.
///
/// The text names the sandbox and the kind of request, never the request's
/// words (they may be confidential, and a notification is shown on a locked
/// screen and kept in Notification Center). No sound.
///
/// Not posted while the person looks at the request: the sbxw desktop app is
/// in front (it opens its Requests panel on the request when it arrives).
/// Posted at most once per request and kind of post, the request told apart
/// by its id and creation time.
@MainActor
final class RelayNotifier {
    static let desktopBundle = "app.sbxw.desktop"

    /// "id@created#first" or "id@created#reminder", for requests still open.
    private var posted: Set<String> = []
    private var asked = false

    /// The request starts to wait for the person (`reminder == false`), or
    /// has waited 10 minutes (`true`).
    func waiting(_ req: RelayRequest, reminder: Bool) {
        let key = "\(req.key)#\(reminder ? "reminder" : "first")"
        guard !posted.contains(key) else { return }
        posted.insert(key)
        if Self.desktopInFront() {
            Log.log("relay notify \(req.id): skipped, the desktop app is in front")
            return
        }
        let content = UNMutableNotificationContent()
        content.title = reminder ? "Still waiting for you" : "A request waits for you"
        content.body = Self.body(req)
        content.sound = nil
        content.threadIdentifier = "relay"
        let request = UNNotificationRequest(identifier: "relay-\(req.key)", content: content, trigger: nil)
        let center = UNUserNotificationCenter.current()
        let post = {
            center.add(request) { err in
                if let err { Log.log("relay notify \(req.id): \(err.localizedDescription)") }
            }
        }
        if asked {
            post()
            return
        }
        asked = true
        // Alerts only: never a sound, never the badge.
        center.requestAuthorization(options: [.alert]) { ok, _ in
            Log.log("relay notify: notifications \(ok ? "allowed" : "not allowed")")
            if ok { post() }
        }
    }

    /// The request was settled or the daemon forgot it: its posts go from
    /// Notification Center, and its keys from memory.
    func settled(_ live: [RelayRequest]) {
        let keys = Set(live.map(\.key))
        let gone = posted.filter { !keys.contains(String($0.split(separator: "#").first ?? "")) }
        guard !gone.isEmpty else { return }
        posted.subtract(gone)
        let ids = Set(gone.map { "relay-" + String($0.split(separator: "#").first ?? "") })
        UNUserNotificationCenter.current().removeDeliveredNotifications(withIdentifiers: Array(ids))
    }

    /// "web-app asks for a screenshot": the sandbox and the kind, no content.
    static func body(_ req: RelayRequest) -> String {
        let who = req.from.split(separator: "~").last.map(String.init) ?? req.from
        if req.state == .answered { return "\(who): an answer waits for your review" }
        switch req.kind ?? .question {
        case .screenshot: return "\(who) asks for a screenshot"
        case .mac: return "\(who) asks to use the virtual Mac"
        case .question: return "\(who) asks a question"
        case .unknown: return "\(who) asks you something"
        }
    }

    static func desktopInFront() -> Bool {
        NSWorkspace.shared.frontmostApplication?.bundleIdentifier == desktopBundle
    }
}
