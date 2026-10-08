import Foundation
import AuthenticationServices
import UIKit
import Tauri

struct AuthenticationRequest: Decodable { let attempt: String; let authorization: String; let redirect: String }
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
        args.redirect == identifier + ".auth://callback/mdreader-sync",
        let url = URL(string: args.authorization), url.scheme == "https", url.user == nil, url.password == nil,
        let query = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems,
        query.filter({$0.name == "redirect_uri"}).count == 1,
        query.first(where: {$0.name == "redirect_uri"})?.value == args.redirect,
        let window = window else { invoke.reject("authentication_unavailable"); return }
      self.attempt = args.attempt; self.window = window; self.pending = invoke
      let session = ASWebAuthenticationSession(url: url, callbackURLScheme: identifier + ".auth") { callback, error in
        DispatchQueue.main.async {
          guard self.attempt == args.attempt else { return }
          self.session = nil; self.attempt = nil; self.window = nil; self.pending = nil
          if let error = error as? ASWebAuthenticationSessionError, error.code == .canceledLogin {
            invoke.resolve(["state":"cancelled"]); return
          }
          guard error == nil, let callback = callback, callback.absoluteString.utf8.count <= 16384,
            callback.scheme == identifier + ".auth", callback.host == "callback", callback.path == "/mdreader-sync",
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
