// filebridge's iOS plugin (ADR 0014), the twin of the Android one
// (gen/android/.../FilebridgePlugin.kt); src/native.rs calls it.
//
// - pickFolder: a folder from the Files app, as a path the engine reads with std::fs. iOS lets
//   the app read it only while it holds the folder's security-scoped access, so the access is
//   opened at once and a bookmark is kept to open it again in later launches.
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

class FilebridgePlugin: Plugin, UIDocumentPickerDelegate {
  /// Bookmarks of the picked folders, by path, in the app's own preferences.
  private let bookmarksKey = "filebridge.folderBookmarks"
  /// Folders readable now, by path. Never closed while the app runs: a Job may read them later.
  private var opened: [String: URL] = [:]
  private var pendingPick: Invoke?
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

  // MARK: Folders

  @objc public func pickFolder(_ invoke: Invoke) {
    DispatchQueue.main.async {
      let picker = UIDocumentPickerViewController(
        forOpeningContentTypes: [.folder], asCopy: false)
      picker.delegate = self
      picker.allowsMultipleSelection = false
      self.pendingPick = invoke
      self.manager.viewController?.present(picker, animated: true)
    }
  }

  func documentPicker(
    _ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]
  ) {
    guard let invoke = pendingPick else { return }
    pendingPick = nil
    guard let url = urls.first else {
      invoke.resolve()
      return
    }
    guard url.startAccessingSecurityScopedResource() else {
      invoke.reject("iOS gave no access to this folder", code: "not_readable")
      return
    }
    opened[url.path] = url
    var bookmarks = savedBookmarks()
    do {
      bookmarks[url.path] = try url.bookmarkData(
        options: [], includingResourceValuesForKeys: nil, relativeTo: nil)
      save(bookmarks)
    } catch {
      // Readable for this launch only: a Job left mid-way can't read it after a restart.
      NSLog("filebridge: no bookmark for a picked folder: \(error)")
    }
    invoke.resolve(["path": url.path])
  }

  func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
    pendingPick?.resolve()
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
func initPlugin() -> Plugin {
  return FilebridgePlugin()
}
