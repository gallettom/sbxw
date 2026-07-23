import Foundation

/// Tiny diagnostic logger. Writes timestamped lines to
/// `~/Library/Logs/sbxw-island.log` and echoes them to stdout (visible when
/// launched via `swift run`). Share the log file to debug data flow.
enum Log {
    static let fileURL: URL = {
        let dir = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Logs", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir.appendingPathComponent("sbxw-island.log")
    }()

    private static let queue = DispatchQueue(label: "app.sbxw.island.log")
    private static let stamp: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f
    }()

    static func log(_ message: String) {
        let line = "\(stamp.string(from: Date())) \(message)\n"
        FileHandle.standardError.write(Data("[sbxw-island] \(message)\n".utf8))
        queue.async {
            guard let data = line.data(using: .utf8) else { return }
            if let handle = try? FileHandle(forWritingTo: fileURL) {
                defer { try? handle.close() }
                _ = try? handle.seekToEnd()
                handle.write(data)
            } else {
                try? data.write(to: fileURL)
            }
        }
    }
}
