import AppKit
import UniformTypeIdentifiers
import WebKit

/// What the page asks of the app (see web/platform.js): `await webkit.messageHandlers.desktop.postMessage({cmd, …})`.
final class Bridge: NSObject, WKScriptMessageHandlerWithReply {
    weak var window: NSWindow?
    /// ~/Library/Application Support/Ollama Notes, unless set: `defaults write local.ollama-notes DataDirectory <path>`
    let dataDir = UserDefaults.standard.string(forKey: "DataDirectory")
        .map { URL(fileURLWithPath: ($0 as NSString).expandingTildeInPath, isDirectory: true) }
        ?? FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        .appendingPathComponent("Ollama Notes", isDirectory: true)
    var dataFile: URL { dataDir.appendingPathComponent("notes.json") }
    private var backedUp = false

    func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage,
                               replyHandler: @escaping (Any?, String?) -> Void) {
        guard message.frameInfo.securityOrigin.protocol == "app", let body = message.body as? [String: Any] else {
            return replyHandler(nil, "unexpected message")
        }
        let text = { (key: String) in body[key] as? String ?? "" }
        do {
            switch body["cmd"] as? String {
            case "load": replyHandler(try load(), nil)
            case "save": try save(text("json")); replyHandler(true, nil)
            case "saveFile": saveFile(name: text("name"), contents: text("text"), reply: replyHandler)
            case "startOllama": replyHandler(OllamaLauncher.start(host: text("host")), nil)
            case let cmd: replyHandler(nil, "unknown command: \(cmd ?? "none")")
            }
        } catch {
            replyHandler(nil, error.localizedDescription)
        }
    }

    /// The saved state (JSON text), or nil before the first save.
    private func load() throws -> String? {
        guard FileManager.default.fileExists(atPath: dataFile.path) else { return nil }
        return try String(contentsOf: dataFile, encoding: .utf8)
    }

    private func save(_ json: String) throws {
        let fm = FileManager.default
        try fm.createDirectory(at: dataDir, withIntermediateDirectories: true)
        if !backedUp, fm.fileExists(atPath: dataFile.path) {
            // Keep the state this session started from, in case something goes wrong in it (a bad import, Clear map…).
            let backup = dataDir.appendingPathComponent("notes.backup.json")
            try? fm.removeItem(at: backup)
            try? fm.copyItem(at: dataFile, to: backup)
        }
        backedUp = true
        try Data(json.utf8).write(to: dataFile, options: .atomic)
    }

    /// Asks where to save `contents`; replies with the path, or nil if cancelled.
    private func saveFile(name: String, contents: String, reply: @escaping (Any?, String?) -> Void) {
        guard let window else { return reply(nil, "no window") }
        let panel = NSSavePanel()
        panel.nameFieldStringValue = name
        if let type = UTType(filenameExtension: (name as NSString).pathExtension) { panel.allowedContentTypes = [type] }
        panel.beginSheetModal(for: window) { response in
            guard response == .OK, let url = panel.url else { return reply(nil, nil) }
            do {
                try Data(contents.utf8).write(to: url, options: .atomic)
                reply(url.path, nil)
            } catch {
                reply(nil, error.localizedDescription)
            }
        }
    }
}

/// Best-effort start of a local Ollama server, as server.py did. Returns what was done, or nil when the host is
/// another machine or Ollama isn't installed.
enum OllamaLauncher {
    static func start(host: String) -> String? {
        guard let name = URL(string: host)?.host?.lowercased(), ["127.0.0.1", "localhost", "0.0.0.0", "::1"].contains(name)
        else { return nil }
        if let app = NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.electron.ollama") { // Ollama.app
            let config = NSWorkspace.OpenConfiguration()
            config.activates = false
            NSWorkspace.shared.openApplication(at: app, configuration: config)
            return "opened Ollama.app"
        }
        // Apps opened from Finder get a minimal PATH, so also look where the installers put the CLI.
        let path = (ProcessInfo.processInfo.environment["PATH"] ?? "").split(separator: ":").map(String.init)
        guard let exe = (path + ["/opt/homebrew/bin", "/usr/local/bin", NSHomeDirectory() + "/.local/bin"])
            .map({ $0 + "/ollama" }).first(where: FileManager.default.isExecutableFile(atPath:))
        else { return nil }
        let serve = Process()
        serve.executableURL = URL(fileURLWithPath: exe)
        serve.arguments = ["serve"]
        serve.standardOutput = FileHandle.nullDevice
        serve.standardError = FileHandle.nullDevice
        guard (try? serve.run()) != nil else { return nil }
        return "ran `ollama serve`"
    }
}
