import AppKit
import WebKit

/// Ollama Notes for macOS: one window showing the web UI from Resources/web (served as app://notes), plus what a
/// browser tab can't do: a menu bar, open/save panels, a data file in Application Support, and starting Ollama.
final class AppDelegate: NSObject, NSApplicationDelegate, NSWindowDelegate, WKNavigationDelegate, WKUIDelegate {
    static let page = URL(string: "app://notes/index.html")!
    let bridge = Bridge()
    var window: NSWindow!
    var webView: WKWebView!
    private var quitting = false

    func applicationWillFinishLaunching(_ notification: Notification) {
        NSWindow.allowsAutomaticWindowTabbing = false
        NSApp.mainMenu = makeMenu()
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        // A second copy would fight the first over the data file, so hand over to the one already running.
        if let id = Bundle.main.bundleIdentifier, let other = NSRunningApplication.runningApplications(withBundleIdentifier: id)
            .first(where: { $0.processIdentifier != getpid() }) {
            other.activate(options: .activateAllWindows)
            NSApp.terminate(nil)
            return
        }
        let config = WKWebViewConfiguration()
        config.setURLSchemeHandler(WebAssets(root: Bundle.main.resourceURL!.appendingPathComponent("web")), forURLScheme: "app")
        config.userContentController.addScriptMessageHandler(bridge, contentWorld: .page, name: "desktop")
        webView = WKWebView(frame: .zero, configuration: config)
        webView.navigationDelegate = self
        webView.uiDelegate = self
        webView.isInspectable = true // right-click › Inspect Element
        webView.setValue(false, forKey: "drawsBackground") // no white flash before the page paints

        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1280, height: 820),
                          styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
        window.title = "Ollama Notes"
        window.titleVisibility = .hidden
        window.titlebarAppearsTransparent = true
        window.backgroundColor = NSColor(srgbRed: 0x16 / 255, green: 0x19 / 255, blue: 0x23 / 255, alpha: 1) // --bg2, as the top bar
        window.appearance = NSAppearance(named: .darkAqua)
        window.minSize = NSSize(width: 760, height: 480)
        window.contentView = webView
        window.delegate = self
        window.isReleasedWhenClosed = false
        if !window.setFrameUsingName("Main") { window.center() }
        window.setFrameAutosaveName("Main")
        bridge.window = window
        webView.load(URLRequest(url: Self.page))
        window.makeKeyAndOrderFront(nil)
    }

    func applicationSupportsSecureRestorableState(_ app: NSApplication) -> Bool { true }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard let webView else { return .terminateNow }
        if quitting { return .terminateLater }
        quitting = true
        // Let the page write a save that is still waiting on its debounce timer (but never hang on quit).
        let done = { NSApp.reply(toApplicationShouldTerminate: true) }
        webView.callAsyncJavaScript("await window.desktopFlush?.()", in: nil, in: .page) { _ in done() }
        DispatchQueue.main.asyncAfter(deadline: .now() + 3, execute: done)
        return .terminateLater
    }

    // Closing the window quits, by way of applicationShouldTerminate and the page's last save.
    func windowShouldClose(_ sender: NSWindow) -> Bool {
        NSApp.terminate(nil)
        return false
    }

    // MARK: menu bar

    /// Runs one of the page's commands (`commands` in web/app.js).
    @objc func runCommand(_ sender: NSMenuItem) {
        guard let name = sender.representedObject as? String else { return }
        webView?.evaluateJavaScript("window.desktopCommand?.('\(name)'); undefined")
    }

    @objc func showData(_ sender: Any?) {
        if FileManager.default.fileExists(atPath: bridge.dataFile.path) {
            NSWorkspace.shared.activateFileViewerSelecting([bridge.dataFile])
        } else {
            try? FileManager.default.createDirectory(at: bridge.dataDir, withIntermediateDirectories: true)
            NSWorkspace.shared.open(bridge.dataDir)
        }
    }

    @objc func openModelLibrary(_ sender: Any?) { NSWorkspace.shared.open(URL(string: "https://ollama.com/library")!) }

    private func makeMenu() -> NSMenu {
        let bar = NSMenu()
        func menu(_ title: String, _ items: [NSMenuItem]) -> NSMenu {
            let m = NSMenu(title: title)
            items.forEach(m.addItem)
            bar.addItem(withTitle: title, action: nil, keyEquivalent: "").submenu = m
            return m
        }
        func item(_ title: String, _ action: Selector, _ key: String = "", _ mods: NSEvent.ModifierFlags = .command,
                  target: AnyObject? = nil, command: String? = nil) -> NSMenuItem {
            let i = NSMenuItem(title: title, action: action, keyEquivalent: key)
            i.keyEquivalentModifierMask = mods
            i.target = target
            i.representedObject = command
            return i
        }
        func command(_ title: String, _ name: String, _ key: String = "", _ mods: NSEvent.ModifierFlags = .command) -> NSMenuItem {
            item(title, #selector(runCommand(_:)), key, mods, target: self, command: name)
        }
        let sep = NSMenuItem.separator
        let services = NSMenuItem(title: "Services", action: nil, keyEquivalent: "")
        services.submenu = NSMenu(title: "Services")
        NSApp.servicesMenu = services.submenu

        _ = menu("Ollama Notes", [
            item("About Ollama Notes", #selector(NSApplication.orderFrontStandardAboutPanel(_:))),
            sep(),
            command("Settings…", "settings", ","),
            sep(),
            services,
            sep(),
            item("Hide Ollama Notes", #selector(NSApplication.hide(_:)), "h"),
            item("Hide Others", #selector(NSApplication.hideOtherApplications(_:)), "h", [.command, .option]),
            item("Show All", #selector(NSApplication.unhideAllApplications(_:))),
            sep(),
            item("Quit Ollama Notes", #selector(NSApplication.terminate(_:)), "q"),
        ])
        _ = menu("File", [
            command("New Chat", "new-chat", "n"),
            command("New Note", "new-note", "n", [.command, .shift]),
            sep(),
            command("Import Files…", "import-files", "o"),
            command("Import Map…", "import-map", "o", [.command, .shift]),
            command("Export Map…", "export-map", "s", [.command, .shift]),
            sep(),
            item("Show Data in Finder", #selector(showData(_:)), target: self),
            sep(),
            item("Close Window", #selector(NSWindow.performClose(_:)), "w"),
        ])
        _ = menu("Edit", [
            item("Undo", Selector(("undo:")), "z"),
            item("Redo", Selector(("redo:")), "z", [.command, .shift]),
            sep(),
            item("Cut", #selector(NSText.cut(_:)), "x"),
            item("Copy", #selector(NSText.copy(_:)), "c"),
            item("Paste", #selector(NSText.paste(_:)), "v"),
            item("Paste and Match Style", #selector(NSTextView.pasteAsPlainText(_:)), "v", [.command, .option, .shift]),
            item("Delete", #selector(NSText.delete(_:))),
            item("Select All", #selector(NSText.selectAll(_:)), "a"),
        ])
        _ = menu("View", [
            command("Fit Map", "fit", "0"),
            command("Zoom In", "zoom-in", "="),
            command("Zoom Out", "zoom-out", "-"),
            command("Tidy Layout", "tidy"),
            sep(),
            command("Focus Chat", "focus-chat", "l"),
            command("Filter Notes", "filter-notes", "f"),
            sep(),
            item("Enter Full Screen", #selector(NSWindow.toggleFullScreen(_:)), "f", [.command, .control]),
        ])
        NSApp.windowsMenu = menu("Window", [
            item("Minimize", #selector(NSWindow.performMiniaturize(_:)), "m"),
            item("Zoom", #selector(NSWindow.performZoom(_:))),
            sep(),
            item("Bring All to Front", #selector(NSApplication.arrangeInFront(_:))),
        ])
        NSApp.helpMenu = menu("Help", [item("Ollama Model Library", #selector(openModelLibrary(_:)), target: self)])
        return bar
    }

    // MARK: web view

    // The window only ever shows the app's own page; links in replies open in the default browser.
    func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction,
                 decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
        let url = action.request.url
        if url?.scheme == "app" || url?.scheme == "about" { return decisionHandler(.allow) }
        if let url { openExternally(url) }
        decisionHandler(.cancel)
    }

    func webView(_ webView: WKWebView, createWebViewWith configuration: WKWebViewConfiguration, for action: WKNavigationAction,
                 windowFeatures: WKWindowFeatures) -> WKWebView? {
        if let url = action.request.url { openExternally(url) }
        return nil
    }

    private func openExternally(_ url: URL) {
        if ["http", "https", "mailto"].contains(url.scheme ?? "") { NSWorkspace.shared.open(url) }
    }

    func webViewWebContentProcessDidTerminate(_ webView: WKWebView) { webView.reload() }

    // <input type=file>: + File, Import map, and the File menu's import commands
    func webView(_ webView: WKWebView, runOpenPanelWith parameters: WKOpenPanelParameters, initiatedByFrame frame: WKFrameInfo,
                 completionHandler: @escaping ([URL]?) -> Void) {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = parameters.allowsMultipleSelection
        panel.beginSheetModal(for: window) { completionHandler($0 == .OK ? panel.urls : nil) }
    }

    func webView(_ webView: WKWebView, runJavaScriptConfirmPanelWithMessage message: String, initiatedByFrame frame: WKFrameInfo,
                 completionHandler: @escaping (Bool) -> Void) {
        let alert = makeAlert(message)
        alert.addButton(withTitle: "OK")
        alert.addButton(withTitle: "Cancel")
        alert.beginSheetModal(for: window) { completionHandler($0 == .alertFirstButtonReturn) }
    }

    func webView(_ webView: WKWebView, runJavaScriptAlertPanelWithMessage message: String, initiatedByFrame frame: WKFrameInfo,
                 completionHandler: @escaping () -> Void) {
        makeAlert(message).beginSheetModal(for: window) { _ in completionHandler() }
    }

    /// "Delete everything? (Export first.)" -> bold question, with the rest as the explanation below it.
    private func makeAlert(_ message: String) -> NSAlert {
        let alert = NSAlert()
        let parts = message.split(separator: "?", maxSplits: 1)
        alert.messageText = parts.count == 2 ? String(parts[0]) + "?" : message
        alert.informativeText = parts.count == 2 ? parts[1].trimmingCharacters(in: .whitespaces) : ""
        return alert
    }
}
