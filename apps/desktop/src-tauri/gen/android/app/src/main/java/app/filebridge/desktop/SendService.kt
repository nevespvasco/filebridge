package app.filebridge.desktop

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.wifi.WifiManager
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat

/**
 * Keeps the app's process, and so the engine inside it, running while a Job sends with the
 * screen off or another app in front (ADR 0014). Holds a partial wake lock and a Wi-Fi lock,
 * and shows the send's progress. If Android stops it anyway, the Job resumes when the app opens
 * again, like after a crash: every Stage is persisted.
 */
class SendService : Service() {
  companion object {
    const val EXTRA_TITLE = "title"
    const val EXTRA_TEXT = "text"
    const val EXTRA_PROGRESS = "progress"
    private const val CHANNEL = "sending"
    private const val NOTIFICATION_ID = 1
  }

  private var wakeLock: PowerManager.WakeLock? = null
  private var wifiLock: WifiManager.WifiLock? = null

  override fun onBind(intent: Intent?): IBinder? = null

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    val notification = notification(
      intent?.getStringExtra(EXTRA_TITLE) ?: "filebridge",
      intent?.getStringExtra(EXTRA_TEXT) ?: "",
      intent?.getIntExtra(EXTRA_PROGRESS, -1) ?: -1
    )
    val type = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
      ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE
    } else {
      0
    }
    ServiceCompat.startForeground(this, NOTIFICATION_ID, notification, type)
    holdLocks()
    // Restarted without the app's window there would be no engine to keep alive.
    return START_NOT_STICKY
  }

  override fun onDestroy() {
    wakeLock?.takeIf { it.isHeld }?.release()
    wifiLock?.takeIf { it.isHeld }?.release()
    super.onDestroy()
  }

  private fun holdLocks() {
    if (wakeLock == null) {
      val power = getSystemService(Context.POWER_SERVICE) as PowerManager
      wakeLock = power.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "filebridge:send").apply {
        setReferenceCounted(false)
        acquire()
      }
    }
    if (wifiLock == null) {
      val wifi = applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
      @Suppress("DEPRECATION")
      val mode = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
        WifiManager.WIFI_MODE_FULL_HIGH_PERF
      } else {
        WifiManager.WIFI_MODE_FULL
      }
      wifiLock = wifi.createWifiLock(mode, "filebridge:send").apply {
        setReferenceCounted(false)
        acquire()
      }
    }
  }

  private fun notification(title: String, text: String, progress: Int): Notification {
    val manager = getSystemService(NotificationManager::class.java)
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
      manager.createNotificationChannel(
        NotificationChannel(CHANNEL, title, NotificationManager.IMPORTANCE_LOW)
      )
    }
    val open = PendingIntent.getActivity(
      this,
      0,
      Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
      PendingIntent.FLAG_IMMUTABLE
    )
    return NotificationCompat.Builder(this, CHANNEL)
      .setSmallIcon(R.drawable.ic_launcher_foreground)
      .setContentTitle(title)
      .setContentText(text)
      .setContentIntent(open)
      .setOngoing(true)
      .setOnlyAlertOnce(true)
      .setProgress(100, progress.coerceAtLeast(0), progress < 0)
      .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
      .build()
  }
}
