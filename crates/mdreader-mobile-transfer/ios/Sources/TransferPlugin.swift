import Foundation
import UIKit
import WebKit
import CryptoKit
import Tauri

private struct CopyRequest: Decodable {
  let path: String; let name: String; let mime: String
  let sha256: String; let bytes: UInt32; let action: String
}

/// Receives only the app's native-authorized private staged copy. Hashing runs
/// off the UI thread. OS presentation never becomes an input filesystem grant.
class TransferPlugin: Plugin, UIDocumentPickerDelegate {
  private let account = AccountSession()
  private let providerVault = ProviderVault()
  @objc public func vaultRead(_ invoke: Invoke) { DispatchQueue.global(qos: .userInitiated).async { self.providerVault.read(invoke) } }
  @objc public func vaultWrite(_ invoke: Invoke) { DispatchQueue.global(qos: .userInitiated).async { self.providerVault.write(invoke) } }
  @objc public func vaultDelete(_ invoke: Invoke) { DispatchQueue.global(qos: .userInitiated).async { self.providerVault.delete(invoke) } }
  @objc public func authenticate(_ invoke: Invoke) throws { try account.authenticate(invoke, window: manager.viewController?.view.window) }
  @objc public func cancelAuthentication(_ invoke: Invoke) throws { try account.cancel(invoke) }
  private var pending: Invoke?
  private var presenting = false

  @objc public func presentCopy(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(CopyRequest.self)
    DispatchQueue.main.async {
      guard !self.presenting else { invoke.reject("transfer_busy"); return }
      self.presenting = true
      DispatchQueue.global(qos: .userInitiated).async {
        guard let file = self.verified(args) else {
          DispatchQueue.main.async { self.presenting = false; invoke.reject("transfer_unavailable") }
          return
        }
        DispatchQueue.main.async { self.present(file, args, invoke) }
      }
    }
  }
  private func verified(_ args: CopyRequest) -> URL? {
    guard args.bytes <= 20 * 1024 * 1024,
      ["export", "share"].contains(args.action),
      ["text/markdown", "application/zip"].contains(args.mime),
      args.sha256.count == 64, !args.name.isEmpty, args.name.count <= 160,
      !args.name.contains("/"), !args.name.contains("\\"),
      let cache = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask).first,
      let identifier = Bundle.main.bundleIdentifier else { return nil }
    // Tauri's iOS app_cache_dir appends the bundle identifier to Caches.
    let root = cache.resolvingSymlinksInPath().appendingPathComponent(identifier)
      .appendingPathComponent("mdreader-transfers-v1").standardizedFileURL
    let file = URL(fileURLWithPath: args.path).standardizedFileURL
    guard file.path.hasPrefix(root.path + "/"), file.resolvingSymlinksInPath() == file,
      let attributes = try? FileManager.default.attributesOfItem(atPath: file.path),
      attributes[.type] as? FileAttributeType == .typeRegular,
      (attributes[.size] as? NSNumber)?.uint32Value == args.bytes,
      (attributes[.referenceCount] as? NSNumber)?.intValue == 1,
      let data = try? Data(contentsOf: file, options: .mappedIfSafe), data.count == Int(args.bytes),
      SHA256.hash(data: data).map({ String(format: "%02x", $0) }).joined() == args.sha256
    else { return nil }
    return file
  }
  private func present(_ file: URL, _ args: CopyRequest, _ invoke: Invoke) {
    guard let controller = self.manager.viewController, controller.presentedViewController == nil else {
      presenting = false; invoke.reject("transfer_unavailable"); return
    }
    if args.action == "export" {
      pending = invoke
      let picker = UIDocumentPickerViewController(forExporting: [file], asCopy: true)
      picker.delegate = self
      controller.present(picker, animated: true)
    } else {
      let share = UIActivityViewController(activityItems: [file], applicationActivities: nil)
      share.completionWithItemsHandler = { _, completed, _, error in
        self.presenting = false
        if error != nil { invoke.reject("transfer_unconfirmed") }
        else { invoke.resolve(["state": completed ? "presented" : "cancelled"]) }
      }
      if let popover = share.popoverPresentationController {
        popover.sourceView = controller.view
        popover.sourceRect = CGRect(x: controller.view.bounds.midX, y: controller.view.bounds.midY, width: 1, height: 1)
      }
      controller.present(share, animated: true)
    }
  }
  public func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
    presenting = false
    if urls.isEmpty { pending?.reject("transfer_unconfirmed") }
    else { pending?.resolve(["state": "exported"]) }
    pending = nil
  }
  public func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
    presenting = false; pending?.resolve(["state": "cancelled"]); pending = nil
  }
}
@_cdecl("init_plugin_mdreader_transfer")
func initPlugin() -> Plugin { TransferPlugin() }
