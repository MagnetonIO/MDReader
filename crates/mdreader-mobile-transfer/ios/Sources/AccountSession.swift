import Foundation
import AuthenticationServices
import UIKit
import Tauri

struct AuthenticationRequest: Decodable { let attempt: String; let authorization: String; let redirect: String; let provider: String? }
struct CancelAuthentication: Decodable { let attempt: String }
/// One native session. Codes never pass through the reader WebView or URL events.
final class AccountSession: NSObject, ASWebAuthenticationPresentationContextProviding {
  private var session: ASWebAuthenticationSession?
  private var attempt: String?
  private var window: UIWindow?
  private var pending: Invoke?
  func authenticate(_ invoke: Invoke, window: UIWindow?) throws {
    let args = try invoke.parseArgs(AuthenticationRequest.self)
    DispatchQueue.main.async {
      guard self.session == nil, UUID(uuidString: args.attempt) != nil,
        let identifier = Bundle.main.bundleIdentifier,
        let redirect = self.redirect(args, identifier: identifier),
        let url = URL(string: args.authorization), url.scheme == "https", url.user == nil, url.password == nil,
        let query = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems,
        query.filter({$0.name == "redirect_uri"}).count == 1,
        query.first(where: {$0.name == "redirect_uri"})?.value == args.redirect,
        let window = window else { invoke.reject("authentication_unavailable"); return }
      self.attempt = args.attempt; self.window = window; self.pending = invoke
      let session = ASWebAuthenticationSession(url: url, callbackURLScheme: redirect.scheme) { callback, error in
        DispatchQueue.main.async {
          guard self.attempt == args.attempt else { return }
          self.session = nil; self.attempt = nil; self.window = nil; self.pending = nil
          if let error = error as? ASWebAuthenticationSessionError, error.code == .canceledLogin {
            invoke.resolve(["state":"cancelled"]); return
          }
          guard error == nil, let callback = callback, callback.absoluteString.utf8.count <= 16384,
            callback.scheme == redirect.scheme, callback.host == redirect.host, callback.path == redirect.path,
            callback.user == nil, callback.password == nil, callback.port == nil, callback.fragment == nil
          else { invoke.reject("authentication_unconfirmed"); return }
          invoke.resolve(["state":"completed", "url":callback.absoluteString])
        }
      }
      session.presentationContextProvider = self
      self.session = session
      if !session.start() {
        self.session = nil; self.attempt = nil; self.window = nil; self.pending = nil
        invoke.reject("authentication_unavailable")
      }
    }
  }
  private func redirect(_ args: AuthenticationRequest, identifier: String) -> URL? {
    guard let authorization = URLComponents(string: args.authorization),
      let redirect = URL(string: args.redirect), redirect.user == nil, redirect.password == nil,
      redirect.port == nil, redirect.query == nil, redirect.fragment == nil else { return nil }
    switch args.provider {
    case nil:
      guard args.redirect == identifier + ".auth://callback/mdreader-sync" else { return nil }
    case "dropbox":
      guard args.redirect == identifier + ".auth://callback/dropbox",
        authorization.host == "www.dropbox.com", authorization.path == "/oauth2/authorize" else { return nil }
    case "google_drive":
      guard authorization.host == "accounts.google.com", authorization.path == "/o/oauth2/v2/auth",
        let clients = authorization.queryItems?.filter({$0.name == "client_id"}), clients.count == 1,
        let client = clients.first?.value, client.hasSuffix(".apps.googleusercontent.com") else { return nil }
      let prefix = String(client.dropLast(".apps.googleusercontent.com".count))
      guard !prefix.isEmpty, prefix.range(of: "^[a-zA-Z0-9-]+$", options: .regularExpression) != nil,
        args.redirect == "com.googleusercontent.apps." + prefix + ":/oauth2redirect" else { return nil }
    default: return nil
    }
    return redirect
  }
  func cancel(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(CancelAuthentication.self)
    DispatchQueue.main.async {
      if self.attempt == args.attempt {
        let pending = self.pending
        let session = self.session
        self.session = nil; self.attempt = nil; self.window = nil; self.pending = nil
        session?.cancel()
        pending?.resolve(["state":"cancelled"])
      }
      invoke.resolve()
    }
  }
  func presentationAnchor(for session: ASWebAuthenticationSession) -> ASPresentationAnchor {
    // The actual window is required before starting a session and retained until
    // its completion; no unrelated application's presentation context is used.
    return window ?? UIWindow()
  }
}
