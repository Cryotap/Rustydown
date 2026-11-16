package com.rustget.app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.graphics.drawable.Icon
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import org.json.JSONObject

/**
 * Foreground service: keeps the process (and the Rust engine inside it) alive while downloads run,
 * shows one progress notification, and stops itself when nothing is left to do.
 */
class DownloadService : Service() {

    companion object {
        const val CHANNEL = "downloads"
        const val NOTIF_ID = 1
        const val ACTION_PAUSE_ALL = "com.rustget.app.PAUSE_ALL"
        @Volatile var running = false

        fun start(ctx: Context) {
            if (running) return
            try {
                ctx.startForegroundService(Intent(ctx, DownloadService::class.java))
            } catch (_: Exception) {
                // not allowed to start from the background right now; the UI will retry
            }
        }
    }

    private var wake: PowerManager.WakeLock? = null
    @Volatile private var alive = false

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        running = true
        if (intent?.action == ACTION_PAUSE_ALL) Native.pauseAll()

        val nm = getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL, "Downloads", NotificationManager.IMPORTANCE_LOW)
        )
        val first = build("Starting…", 0, true)
        if (Build.VERSION.SDK_INT >= 29) {
            startForeground(NOTIF_ID, first, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        } else {
            startForeground(NOTIF_ID, first)
        }

        if (!alive) {
            alive = true
            val pm = getSystemService(Context.POWER_SERVICE) as PowerManager
            wake = pm.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "rustget:downloads").also { it.acquire() }
            Thread { pollLoop(nm) }.start()
        }
        return START_NOT_STICKY
    }

    private fun pollLoop(nm: NotificationManager) {
        var idle = 0
        var tick = 0
        while (alive) {
            try {
                if (tick % 5 == 0) Native.setMetered(Util.isMetered(this))
                tick++
                val snap = JSONObject(Native.snapshot())
                val active = snap.optInt("active")
                val queued = snap.optInt("queued")
                if (Util.hasWork(snap)) idle = 0 else idle++
                if (idle >= 8) break

                val items = snap.getJSONArray("items")
                var done = 0L
                var total = 0L
                for (i in 0 until items.length()) {
                    val o = items.getJSONObject(i)
                    if (o.getString("status") == "Active" && !o.isNull("total")) {
                        done += o.getLong("done")
                        total += o.getLong("total")
                    }
                }
                val pct = if (total > 0) (done * 100 / total).toInt() else 0
                val text = if (active > 0) {
                    "$active active · ${Util.speed(snap.optDouble("totalSpeed"))}" +
                        (if (queued > 0) " · $queued queued" else "")
                } else if (queued > 0) "$queued waiting" else "Finishing…"
                nm.notify(NOTIF_ID, build(text, pct, total == 0L))
            } catch (_: Exception) {
            }
            try { Thread.sleep(1000) } catch (_: InterruptedException) { break }
        }
        stopSelf()
    }

    private fun build(text: String, pct: Int, indeterminate: Boolean): Notification {
        val open = PendingIntent.getActivity(
            this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE
        )
        val pause = PendingIntent.getService(
            this, 1,
            Intent(this, DownloadService::class.java).setAction(ACTION_PAUSE_ALL),
            PendingIntent.FLAG_IMMUTABLE
        )
        return Notification.Builder(this, CHANNEL)
            .setSmallIcon(android.R.drawable.stat_sys_download)
            .setContentTitle("RustGet")
            .setContentText(text)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setContentIntent(open)
            .setProgress(100, pct, indeterminate)
            .addAction(
                Notification.Action.Builder(
                    Icon.createWithResource(this, android.R.drawable.ic_media_pause), "Pause all", pause
                ).build()
            )
            .build()
    }

    override fun onDestroy() {
        alive = false
        running = false
        try { wake?.release() } catch (_: Exception) {}
        wake = null
        stopForeground(STOP_FOREGROUND_REMOVE)
        super.onDestroy()
    }
}
