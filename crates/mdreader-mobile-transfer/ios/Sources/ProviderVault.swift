import Foundation
import Security
import Tauri

private struct ProviderVaultRequest: Decodable {
  let namespace: String; let reference: String; let bytes: String?
}
/// Provider custody is this-device-only Keychain material, never a synced file.
final class ProviderVault {
  private func request(_ invoke: Invoke) throws -> (ProviderVaultRequest, [String:Any]) {
    let args = try invoke.parseArgs(ProviderVaultRequest.self)
    guard args.namespace.range(of: "^[a-zA-Z0-9._-]{1,200}$", options: .regularExpression) != nil,
      args.reference.hasPrefix("native-provider-v3-"),
      let id = UUID(uuidString: String(args.reference.dropFirst(19))),
      args.reference == "native-provider-v3-" + id.uuidString.lowercased() else {
      throw NSError(domain: "custody_unavailable", code: 1)
    }
    return (args, [kSecClass as String:kSecClassGenericPassword,
      kSecAttrService as String:args.namespace + ".native-provider-credentials-v3",
      kSecAttrAccount as String:args.reference,
      kSecAttrSynchronizable as String:false])
  }
  func read(_ invoke: Invoke) {
    do {
      let (_, base) = try request(invoke)
      var query = base; query[kSecReturnData as String] = true; query[kSecMatchLimit as String] = kSecMatchLimitOne
      var result: CFTypeRef?; let status = SecItemCopyMatching(query as CFDictionary, &result)
      if status == errSecItemNotFound { invoke.resolve([:]); return }
      guard status == errSecSuccess, let bytes = result as? Data, bytes.count <= 128*1024 else {
        invoke.reject("custody_unavailable"); return
      }
      invoke.resolve(["bytes":bytes.base64EncodedString()])
    } catch { invoke.reject("custody_unavailable") }
  }
  func write(_ invoke: Invoke) {
    do {
      let (args, base) = try request(invoke)
      guard let encoded = args.bytes, encoded.utf8.count <= 176*1024,
        let bytes = Data(base64Encoded: encoded), bytes.count <= 128*1024 else {
        invoke.reject("custody_unavailable"); return
      }
      let attributes: [String:Any] = [kSecValueData as String:bytes,
        kSecAttrAccessible as String:kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly]
      var status = SecItemUpdate(base as CFDictionary, attributes as CFDictionary)
      if status == errSecItemNotFound {
        status = SecItemAdd(base.merging(attributes) { _, new in new } as CFDictionary, nil)
      }
      guard status == errSecSuccess else { invoke.reject("custody_unavailable"); return }
      invoke.resolve()
    } catch { invoke.reject("custody_unavailable") }
  }
  func delete(_ invoke: Invoke) {
    do {
      let (_, query) = try request(invoke)
      let status = SecItemDelete(query as CFDictionary)
      guard status == errSecSuccess || status == errSecItemNotFound else { invoke.reject("custody_unavailable"); return }
      invoke.resolve()
    } catch { invoke.reject("custody_unavailable") }
  }
}
