package dev.mdreader.app

import android.os.Bundle
import android.webkit.WebView
import androidx.activity.OnBackPressedCallback
import androidx.activity.enableEdgeToEdge

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
  }

  override fun onWebViewCreate(webView: WebView) {
    super.onWebViewCreate(webView)
    onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
      override fun handleOnBackPressed() {
        // Open-with/share and explicit-component launches can root a task
        // without ACTION_MAIN/CATEGORY_LAUNCHER. Back must retain this reader
        // task too: finishing the final Activity exits Tao's native owner.
        if (isTaskRoot && !webView.canGoBack()) {
          moveTaskToBack(false)
          return
        }
        // Keep the existing Tauri history/nested Activity behavior, including
        // callbacks registered by plugins. Never call finish or process exit.
        isEnabled = false
        try {
          onBackPressedDispatcher.onBackPressed()
        } finally {
          isEnabled = true
        }
      }
    })
  }
}
