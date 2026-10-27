// filebridge's iOS plugin (ADR 0014), the twin of the Android one
// (gen/android/.../FilebridgePlugin.kt); src/native.rs calls it.
//
// - pickFolder, pickFiles: a folder, or any number of files, from the Files app, as paths the
//   engine reads with std::fs. iOS lets the app read them only while it holds their
//   security-scoped access, so the access is opened at once and a bookmark is kept for each to
//   open it again in later launches.
// - restoreAccess: opens every kept bookmark again, before the engine resumes Jobs left mid-way.
// - sending: keeps the screen on while Jobs send (iOS suspends an app soon after the user leaves
//   it), and asks for the short extra time iOS gives to finish the current Batch on leaving.

import Tauri
import UIKit
import UniformTypeIdentifiers
import WebKit

struct SendingArgs: Decodable {
  let active: Bool
}

/// `{}`: native.rs reads a missing `path` as "cancelled"; a bare `resolve()` would send null.
private let nothing: JsonObject = [:]

class FilebridgePlugin: Plugin, UIDocumentPickerDelegate {
  /// Bookmarks of the picked folders and files, by path, in the app's own preferences (the key
  /// predates files).
  private let bookmarksKey = "filebridge.folderBookmarks"
  /// Folders and files readable now, by path. Never closed while the app runs: a Job may read
  /// them later.
  private var opened: [String: URL] = [:]
  private var pendingPick: Invoke?
  /// Whether the pending pick is files (answered as `paths`) rather than one folder (`path`).
  private var pickingFiles = false
  private var sendingNow = false
  private var backgroundTask: UIBackgroundTaskIdentifier = .invalid

  override func load(webview: WKWebView) {
    let center = NotificationCenter.default
    center.addObserver(
      self, selector: #selector(didEnterBackground),
      name: UIApplication.didEnterBackgroundNotification, object: nil)
    center.addObserver(
      self, selector: #selector(willEnterForeground),
      name: UIApplication.willEnterForegroundNotification, object: nil)
  }

  // MARK: Folders and files

  @objc public func pickFolder(_ invoke: Invoke) {
    present(invoke, types: [.folder], files: false)
  }

  @objc public func pickFiles(_ invoke: Invoke) {
    present(invoke, types: [.item], files: true)
  }

  private func present(_ invoke: Invoke, types: [UTType], files: Bool) {
    DispatchQueue.main.async {
      let picker = UIDocumentPickerViewController(
        forOpeningContentTypes: types, asCopy: false)
      picker.delegate = self
      picker.allowsMultipleSelection = files
      self.pendingPick = invoke
      self.pickingFiles = files
      self.manager.viewController?.present(picker, animated: true)
    }
  }

  func documentPicker(
    _ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]
  ) {
    guard let invoke = pendingPick else { return }
    pendingPick = nil
    var bookmarks = savedBookmarks()
    var paths: [String] = []
    for url in urls {
      // No access: counted as skipped, as Android counts files with no path.
      guard url.startAccessingSecurityScopedResource() else { continue }
      opened[url.path] = url
      do {
        bookmarks[url.path] = try url.bookmarkData(
          options: [], includingResourceValuesForKeys: nil, relativeTo: nil)
      } catch {
        // Readable for this launch only: a Job left mid-way can't read it after a restart.
        NSLog("filebridge: no bookmark for a picked item: \(error)")
      }
      paths.append(url.path)
    }
    save(bookmarks)
    if !urls.isEmpty && paths.isEmpty {
      invoke.reject("iOS gave no access to what was picked", code: "not_readable")
    } else if pickingFiles {
      invoke.resolve(["paths": paths, "skipped": urls.count - paths.count])
    } else if let path = paths.first {
      invoke.resolve(["path": path])
    } else {
      invoke.resolve(nothing)
    }
  }

  func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
    pendingPick?.resolve(pickingFiles ? ["paths": [String](), "skipped": 0] : nothing)
    pendingPick = nil
  }

  @objc public func restoreAccess(_ invoke: Invoke) {
    var bookmarks = savedBookmarks()
    var paths: [String] = []
    for (path, data) in bookmarks {
      if opened[path] != nil {
        paths.append(path)
        continue
      }
      var stale = false
      guard
        let url = try? URL(
          resolvingBookmarkData: data, options: [], relativeTo: nil,
          bookmarkDataIsStale: &stale),
        url.startAccessingSecurityScopedResource()
      else {
        // The folder is gone, or iOS no longer allows it: its Jobs report the Source missing.
        bookmarks.removeValue(forKey: path)
        continue
      }
      if url.path != path || stale {
        bookmarks.removeValue(forKey: path)
        if let fresh = try? url.bookmarkData(
          options: [], includingResourceValuesForKeys: nil, relativeTo: nil)
        {
          bookmarks[url.path] = fresh
        }
      }
      opened[url.path] = url
      paths.append(url.path)
    }
    save(bookmarks)
    invoke.resolve(["paths": paths])
  }

  private func savedBookmarks() -> [String: Data] {
    UserDefaults.standard.dictionary(forKey: bookmarksKey) as? [String: Data] ?? [:]
  }

  private func save(_ bookmarks: [String: Data]) {
    UserDefaults.standard.set(bookmarks, forKey: bookmarksKey)
  }

  // MARK: Sending

  @objc public func sending(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(SendingArgs.self)
    DispatchQueue.main.async {
      self.sendingNow = args.active
      UIApplication.shared.isIdleTimerDisabled = args.active
      if !args.active {
        self.endBackgroundTask()
      }
    }
    invoke.resolve()
  }

  @objc private func didEnterBackground() {
    guard sendingNow, backgroundTask == .invalid else { return }
    backgroundTask = UIApplication.shared.beginBackgroundTask(withName: "filebridge send") {
      [weak self] in
      self?.endBackgroundTask()
    }
  }

  @objc private func willEnterForeground() {
    endBackgroundTask()
  }

  private func endBackgroundTask() {
    guard backgroundTask != .invalid else { return }
    UIApplication.shared.endBackgroundTask(backgroundTask)
    backgroundTask = .invalid
  }
}

@_cdecl("init_plugin_filebridge")
public func initPlugin() -> Plugin {
  return FilebridgePlugin()
}
