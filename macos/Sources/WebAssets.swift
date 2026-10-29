import Foundation
import WebKit

/// Serves the bundled web UI (Resources/web) at app://notes/…. A custom scheme rather than file:// so ES modules and
/// the pdf.js worker load normally, and so the page can call Ollama directly: Ollama accepts app:// origins by default.
final class WebAssets: NSObject, WKURLSchemeHandler {
    let root: URL
    init(root: URL) { self.root = root.standardizedFileURL }

    private static let types = [
        "html": "text/html", "js": "text/javascript", "mjs": "text/javascript", "css": "text/css",
        "json": "application/json", "svg": "image/svg+xml", "png": "image/png", "ico": "image/x-icon",
    ]

    func webView(_ webView: WKWebView, start task: WKURLSchemeTask) {
        guard let url = task.request.url else { return }
        let path = url.path.count > 1 ? String(url.path.dropFirst()) : "index.html"
        let file = root.appendingPathComponent(path).standardizedFileURL
        guard file.path.hasPrefix(root.path + "/"), let data = try? Data(contentsOf: file) else {
            task.didReceive(HTTPURLResponse(url: url, statusCode: 404, httpVersion: "HTTP/1.1", headerFields: [:])!)
            task.didFinish()
            return
        }
        var type = Self.types[file.pathExtension.lowercased()] ?? "application/octet-stream"
        if type.hasPrefix("text/") || type == "application/json" { type += "; charset=utf-8" }
        let headers = ["Content-Type": type, "Content-Length": String(data.count), "Cache-Control": "no-store"]
        task.didReceive(HTTPURLResponse(url: url, statusCode: 200, httpVersion: "HTTP/1.1", headerFields: headers)!)
        task.didReceive(data)
        task.didFinish()
    }

    func webView(_ webView: WKWebView, stop task: WKURLSchemeTask) {} // responses are sent synchronously
}
