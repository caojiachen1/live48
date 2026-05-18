package com.caoji.live48

import android.content.Intent
import android.os.Bundle
import android.webkit.JavascriptInterface
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
  }

  override fun onWebViewCreate(webView: WebView) {
    super.onWebViewCreate(webView)
    webView.addJavascriptInterface(NativePlayerBridge(this), "Live48NativePlayer")
  }
}

class NativePlayerBridge(private val activity: MainActivity) {
  @JavascriptInterface
  fun isAvailable(): Boolean = true

  @JavascriptInterface
  fun playStream(url: String, title: String?, nickname: String?, isLive: Boolean) {
    activity.runOnUiThread {
      val intent = Intent(activity, NativePlayerActivity::class.java).apply {
        putExtra(NativePlayerActivity.EXTRA_STREAM_URL, url)
        putExtra(NativePlayerActivity.EXTRA_TITLE, title ?: "")
        putExtra(NativePlayerActivity.EXTRA_NICKNAME, nickname ?: "")
        putExtra(NativePlayerActivity.EXTRA_IS_LIVE, isLive)
      }
      activity.startActivity(intent)
    }
  }
}
