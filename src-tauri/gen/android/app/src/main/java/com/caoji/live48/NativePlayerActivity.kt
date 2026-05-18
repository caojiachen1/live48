package com.caoji.live48

import android.net.Uri
import android.os.Bundle
import android.view.View
import android.widget.TextView
import androidx.activity.enableEdgeToEdge
import androidx.appcompat.app.AppCompatActivity
import androidx.media3.common.MediaItem
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.ui.PlayerView

class NativePlayerActivity : AppCompatActivity() {
  private var player: ExoPlayer? = null
  private lateinit var playerView: PlayerView
  private lateinit var topBar: View
  private var streamUrl: String? = null

  override fun onCreate(savedInstanceState: Bundle?) {
    super.onCreate(savedInstanceState)
    enableEdgeToEdge()
    setContentView(R.layout.activity_native_player)

    streamUrl = intent.getStringExtra(EXTRA_STREAM_URL)
    val title = intent.getStringExtra(EXTRA_TITLE).orEmpty()
    val nickname = intent.getStringExtra(EXTRA_NICKNAME).orEmpty()

    playerView = findViewById(R.id.native_player_view)
    topBar = findViewById(R.id.native_player_top_bar)

    playerView.controllerAutoShow = false
    playerView.controllerShowTimeoutMs = 3000
    playerView.setControllerVisibilityListener(
      PlayerView.ControllerVisibilityListener { visibility ->
        topBar.visibility = visibility
      }
    )

    findViewById<TextView>(R.id.native_player_title).text =
      if (title.isBlank()) getString(R.string.default_video_title) else title
    findViewById<TextView>(R.id.native_player_subtitle).text = nickname
    findViewById<android.view.View>(R.id.native_player_close).setOnClickListener {
      finish()
    }
  }

  override fun onStart() {
    super.onStart()
    if (player == null) {
      initializePlayer()
    }
  }

  override fun onStop() {
    super.onStop()
    releasePlayer()
  }

  override fun onDestroy() {
    releasePlayer()
    super.onDestroy()
  }

  private fun initializePlayer() {
    val url = streamUrl
    if (url.isNullOrBlank()) {
      finish()
      return
    }

    val exoPlayer = ExoPlayer.Builder(this).build()
    player = exoPlayer
    playerView.player = exoPlayer

    val mediaItem = MediaItem.fromUri(Uri.parse(url))
    exoPlayer.setMediaItem(mediaItem)
    exoPlayer.playWhenReady = true
    exoPlayer.prepare()
    playerView.hideController()
    topBar.visibility = View.GONE
  }

  private fun releasePlayer() {
    player?.release()
    player = null
  }

  companion object {
    const val EXTRA_STREAM_URL = "stream_url"
    const val EXTRA_TITLE = "title"
    const val EXTRA_NICKNAME = "nickname"
    const val EXTRA_IS_LIVE = "is_live"
  }
}
