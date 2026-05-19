package com.caoji.live48

import android.graphics.Bitmap
import android.graphics.Canvas
import android.net.Uri
import android.os.Bundle
import android.view.MotionEvent
import android.view.View
import android.widget.ImageView
import android.widget.Toast
import android.widget.TextView
import androidx.activity.enableEdgeToEdge
import androidx.appcompat.app.AppCompatActivity
import androidx.media3.common.MediaItem
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.ui.PlayerView
import org.json.JSONArray
import org.json.JSONObject
import java.net.HttpURLConnection
import java.net.URL
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicInteger

class NativePlayerActivity : AppCompatActivity() {
  data class PlaylistEntry(
    val liveId: String,
    val title: String,
    val nickname: String,
    var streamUrl: String,
  )

  private var player: ExoPlayer? = null
  private lateinit var playerView: PlayerView
  private lateinit var transitionView: ImageView
  private lateinit var topBar: View
  private lateinit var titleView: TextView
  private lateinit var subtitleView: TextView
  private val playlist = mutableListOf<PlaylistEntry>()
  private var currentIndex = 0
  private val networkExecutor = Executors.newSingleThreadExecutor()
  private val loadToken = AtomicInteger(0)
  private var touchStartX = 0f
  private var touchStartY = 0f
  private var isTrackingSwipe = false
  private var isSwitchAnimating = false

  override fun onCreate(savedInstanceState: Bundle?) {
    super.onCreate(savedInstanceState)
    enableEdgeToEdge()
    setContentView(R.layout.activity_native_player)

    playerView = findViewById(R.id.native_player_view)
    transitionView = findViewById(R.id.native_player_transition)
    topBar = findViewById(R.id.native_player_top_bar)
    titleView = findViewById(R.id.native_player_title)
    subtitleView = findViewById(R.id.native_player_subtitle)

    playerView.controllerAutoShow = false
    playerView.controllerShowTimeoutMs = 3000
    playerView.setKeepContentOnPlayerReset(true)
    topBar.visibility = View.VISIBLE

    loadPlaylistFromIntent()
    setupSwipeSwitch()

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
    networkExecutor.shutdownNow()
    super.onDestroy()
  }

  private fun loadPlaylistFromIntent() {
    val playlistJson = intent.getStringExtra(EXTRA_PLAYLIST_JSON).orEmpty()
    if (playlistJson.isNotBlank()) {
      try {
        val array = JSONArray(playlistJson)
        for (i in 0 until array.length()) {
          val item = array.optJSONObject(i) ?: continue
          val liveId = item.optString("liveId")
          if (liveId.isBlank()) continue
          playlist.add(
            PlaylistEntry(
              liveId = liveId,
              title = item.optString("title", ""),
              nickname = item.optString("nickname", ""),
              streamUrl = item.optString("streamUrl", ""),
            )
          )
        }
      } catch (_: Exception) {
      }
    }

    if (playlist.isEmpty()) {
      val streamUrl = intent.getStringExtra(EXTRA_STREAM_URL).orEmpty()
      if (streamUrl.isBlank()) {
        finish()
        return
      }
      playlist.add(
        PlaylistEntry(
          liveId = "",
          title = intent.getStringExtra(EXTRA_TITLE).orEmpty(),
          nickname = intent.getStringExtra(EXTRA_NICKNAME).orEmpty(),
          streamUrl = streamUrl,
        )
      )
    }

    currentIndex = intent.getIntExtra(EXTRA_START_INDEX, 0).coerceIn(0, playlist.lastIndex)
    updateHeaderForCurrent()
  }

  private fun setupSwipeSwitch() {
    playerView.setOnTouchListener { _, event ->
      when (event.actionMasked) {
        MotionEvent.ACTION_DOWN -> {
          touchStartX = event.x
          touchStartY = event.y
          isTrackingSwipe = true
          false
        }
        MotionEvent.ACTION_CANCEL -> {
          isTrackingSwipe = false
          false
        }
        MotionEvent.ACTION_UP -> {
          if (!isTrackingSwipe) {
            return@setOnTouchListener false
          }
          isTrackingSwipe = false
          val dx = event.x - touchStartX
          val dy = event.y - touchStartY
          if (kotlin.math.abs(dy) > kotlin.math.abs(dx) && kotlin.math.abs(dy) > 72) {
            if (dy < 0) {
              switchBySwipe(1)
            } else {
              switchBySwipe(-1)
            }
            return@setOnTouchListener true
          }
          false
        }
        else -> false
      }
    }
  }

  private fun switchBySwipe(delta: Int) {
    if (playlist.size <= 1 || isSwitchAnimating) return
    val nextIndex = currentIndex + delta
    if (nextIndex !in playlist.indices) {
      Toast.makeText(this, if (delta > 0) "已经是最后一个" else "已经是第一个", Toast.LENGTH_SHORT).show()
      return
    }
    isSwitchAnimating = true
    val height = (playerView.height.takeIf { it > 0 } ?: resources.displayMetrics.heightPixels).toFloat()
    val distance = height
    val outY = if (delta > 0) -distance else distance
    val inY = -outY

    val snapshot = capturePlayerFrame()
    if (snapshot != null) {
      transitionView.setImageBitmap(snapshot)
      transitionView.visibility = View.VISIBLE
      transitionView.translationY = 0f
      transitionView.alpha = 1f
    } else {
      transitionView.visibility = View.GONE
    }

    playerView.animate().cancel()
    transitionView.animate().cancel()

    playerView.translationY = inY
    playerView.alpha = 1f
    currentIndex = nextIndex
    playCurrent()

    val duration = 260L
    if (snapshot != null) {
      transitionView.animate()
        .translationY(outY)
        .alpha(1f)
        .setDuration(duration)
        .withEndAction {
          transitionView.setImageDrawable(null)
          transitionView.visibility = View.GONE
        }
        .start()
    }

    playerView.animate()
      .translationY(0f)
      .alpha(1f)
      .setDuration(duration)
      .withEndAction {
        isSwitchAnimating = false
      }
      .start()
  }

  private fun initializePlayer() {
    if (player == null) {
      val exoPlayer = ExoPlayer.Builder(this).build()
      player = exoPlayer
      playerView.player = exoPlayer
    }
    playCurrent()
    playerView.hideController()
    topBar.visibility = View.VISIBLE
  }

  private fun updateHeaderForCurrent() {
    val entry = playlist[currentIndex]
    titleView.text = if (entry.title.isBlank()) getString(R.string.default_video_title) else entry.title
    subtitleView.text = entry.nickname
  }

  private fun playCurrent() {
    val current = playlist[currentIndex]
    updateHeaderForCurrent()
    preloadAdjacent(currentIndex)
    val token = loadToken.incrementAndGet()

    if (current.streamUrl.isNotBlank()) {
      playResolvedStream(current.streamUrl, token)
      return
    }

    networkExecutor.execute {
      val fetchedUrl = fetchStreamUrl(current.liveId)
      runOnUiThread {
        if (token != loadToken.get()) return@runOnUiThread
        if (fetchedUrl.isNullOrBlank()) {
          Toast.makeText(this, "获取播放地址失败", Toast.LENGTH_SHORT).show()
          return@runOnUiThread
        }
        current.streamUrl = fetchedUrl
        playResolvedStream(fetchedUrl, token)
      }
    }
  }

  private fun playResolvedStream(url: String, token: Int) {
    if (token != loadToken.get()) return
    val exoPlayer = player ?: return
    exoPlayer.setMediaItem(MediaItem.fromUri(Uri.parse(url)))
    exoPlayer.prepare()
    exoPlayer.playWhenReady = true
  }

  private fun postJson(endpoint: String, body: JSONObject): JSONObject? {
    val connection = (URL(endpoint).openConnection() as HttpURLConnection).apply {
      requestMethod = "POST"
      connectTimeout = 10_000
      readTimeout = 10_000
      doOutput = true
      setRequestProperty("Host", "pocketapi.48.cn")
      setRequestProperty("Content-Type", "application/json;charset=utf-8")
      setRequestProperty("User-Agent", "PocketFans201807/6.0.16 (iPhone; iOS 13.5.1; Scale/2.00)")
      setRequestProperty("Accept-Language", "zh-Hans-AW;q=1")
      setRequestProperty(
        "appInfo",
        """{"vendor":"apple","deviceId":"11111111-1111-1111-1111-111111111111","appVersion":"7.0.4","appBuild":"23011601","osVersion":"16.3.1","osType":"ios","deviceName":"iPhone XR","os":"ios"}"""
      )
    }

    return try {
      connection.outputStream.use { output ->
        output.write(body.toString().toByteArray(Charsets.UTF_8))
      }
      val responseText = connection.inputStream.bufferedReader().use { it.readText() }
      JSONObject(responseText)
    } catch (_: Exception) {
      null
    } finally {
      connection.disconnect()
    }
  }

  private fun fetchStreamUrl(liveId: String): String? {
    if (liveId.isBlank()) return null

    val getOneBody = JSONObject().put("liveId", liveId)
    val getOneJson = postJson("https://pocketapi.48.cn/live/api/v1/live/getLiveOne", getOneBody)
    val contentObj = getOneJson?.optJSONObject("content")
    val playStreamPath = contentObj?.optString("playStreamPath", "").orEmpty()
    if (playStreamPath.isNotBlank()) {
      return playStreamPath
    }

    val getStreamBody = JSONObject()
      .put("liveId", liveId)
      .put("streamType", 3)
    val getStreamJson = postJson("https://pocketapi.48.cn/live/api/v1/live/getlivestream", getStreamBody)
    val content = getStreamJson?.opt("content")
    return when (content) {
      is String -> content.takeIf { it.isNotBlank() }
      else -> null
    }
  }

  private fun releasePlayer() {
    player?.release()
    player = null
  }

  private fun capturePlayerFrame(): Bitmap? {
    val width = playerView.width
    val height = playerView.height
    if (width <= 0 || height <= 0) return null
    return try {
      val bitmap = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888)
      val canvas = Canvas(bitmap)
      playerView.draw(canvas)
      bitmap
    } catch (_: Exception) {
      null
    }
  }

  private fun preloadAdjacent(index: Int) {
    val candidates = listOf(index - 1, index + 1)
    for (candidate in candidates) {
      if (candidate !in playlist.indices) continue
      val entry = playlist[candidate]
      if (entry.streamUrl.isNotBlank() || entry.liveId.isBlank()) continue
      networkExecutor.execute {
        val fetched = fetchStreamUrl(entry.liveId)
        if (!fetched.isNullOrBlank()) {
          entry.streamUrl = fetched
        }
      }
    }
  }

  companion object {
    const val EXTRA_STREAM_URL = "stream_url"
    const val EXTRA_TITLE = "title"
    const val EXTRA_NICKNAME = "nickname"
    const val EXTRA_IS_LIVE = "is_live"
    const val EXTRA_PLAYLIST_JSON = "playlist_json"
    const val EXTRA_START_INDEX = "start_index"
  }
}
