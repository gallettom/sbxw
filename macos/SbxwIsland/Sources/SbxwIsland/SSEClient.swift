import Foundation

/// A Server-Sent Events client built on `URLSessionDataDelegate`.
///
/// `URLSession.bytes(for:)` / `.lines` buffers the response body and does not
/// reliably deliver `text/event-stream` chunks incrementally, so events never
/// arrive until the connection closes. The delegate's `didReceive data:`
/// callback, by contrast, fires as bytes land — the standard way to stream SSE
/// on Apple platforms. This class assembles complete events and hands their raw
/// JSON `data:` payload to `onEvent`.
///
/// All callbacks fire on the session's background delegate queue; the caller is
/// responsible for hopping to whatever actor it needs.
final class SSEClient: NSObject, URLSessionDataDelegate {
    var onOpen: (() -> Void)?
    var onRawLine: ((String) -> Void)?
    var onEvent: ((String) -> Void)?
    var onClose: ((Error?) -> Void)?

    private var session: URLSession!
    private var task: URLSessionDataTask?
    private var buffer = Data()
    private var dataFields = ""

    override init() {
        super.init()
        let cfg = URLSessionConfiguration.default
        cfg.requestCachePolicy = .reloadIgnoringLocalAndRemoteCacheData
        cfg.timeoutIntervalForRequest = 3600
        cfg.timeoutIntervalForResource = 86400
        cfg.httpAdditionalHeaders = ["Accept": "text/event-stream", "Cache-Control": "no-cache"]
        session = URLSession(configuration: cfg, delegate: self, delegateQueue: nil)
    }

    func connect(url: URL) {
        buffer.removeAll()
        dataFields = ""
        let req = URLRequest(url: url)
        let t = session.dataTask(with: req)
        task = t
        t.resume()
    }

    func disconnect() {
        task?.cancel()
        task = nil
        // Break URLSession's retain on this delegate so the client can dealloc.
        session?.invalidateAndCancel()
    }

    // MARK: URLSessionDataDelegate

    func urlSession(
        _ session: URLSession,
        dataTask: URLSessionDataTask,
        didReceive response: URLResponse,
        completionHandler: @escaping (URLSession.ResponseDisposition) -> Void
    ) {
        if let http = response as? HTTPURLResponse, http.statusCode == 200 {
            onOpen?()
            completionHandler(.allow)
        } else {
            completionHandler(.cancel)
        }
    }

    func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive data: Data) {
        buffer.append(data)
        // Split off complete lines on LF as they arrive.
        while let nl = buffer.firstIndex(of: 0x0a) {
            let lineData = buffer[buffer.startIndex..<nl]
            buffer.removeSubrange(buffer.startIndex...nl)
            var line = String(data: lineData, encoding: .utf8) ?? ""
            if line.hasSuffix("\r") { line.removeLast() }
            handle(line)
        }
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        onClose?(error)
    }

    // MARK: SSE framing

    private func handle(_ line: String) {
        onRawLine?(line)
        if line.isEmpty {
            if !dataFields.isEmpty {
                onEvent?(dataFields)
                dataFields = ""
            }
        } else if line.hasPrefix("data:") {
            dataFields += line.dropFirst("data:".count).trimmingCharacters(in: .whitespaces)
        }
        // ":" comment lines (keep-alives) and other fields are ignored.
    }
}
