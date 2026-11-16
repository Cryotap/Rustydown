package com.rustget.app

import android.app.Application
import android.content.Context
import android.net.ConnectivityManager
import android.os.Environment
import org.json.JSONObject
import java.util.Locale

/** Starts the Rust engine once per process; it keeps running while the service or activity lives. */
class App : Application() {
    override fun onCreate() {
        super.onCreate()
        val downloads = getExternalFilesDir(Environment.DIRECTORY_DOWNLOADS) ?: filesDir
        Native.init(filesDir.absolutePath, downloads.absolutePath)
    }
}

object Util {
    fun bytes(n: Long): String {
        var v = n.toDouble()
        val units = arrayOf("B", "KB", "MB", "GB", "TB")
        var i = 0
        while (v >= 1024 && i < units.size - 1) { v /= 1024; i++ }
        return when {
            i == 0 -> "$n B"
            v >= 100 -> String.format(Locale.US, "%.0f %s", v, units[i])
            v >= 10 -> String.format(Locale.US, "%.1f %s", v, units[i])
            else -> String.format(Locale.US, "%.2f %s", v, units[i])
        }
    }

    fun speed(bps: Double): String = bytes(bps.coerceAtLeast(0.0).toLong()) + "/s"

    fun eta(secs: Long): String {
        val h = secs / 3600
        val m = (secs % 3600) / 60
        val s = secs % 60
        return when {
            h > 0 -> String.format(Locale.US, "%dh %02dm", h, m)
            m > 0 -> String.format(Locale.US, "%dm %02ds", m, s)
            else -> "${s}s"
        }
    }

    fun isMetered(ctx: Context): Boolean {
        val cm = ctx.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
        return cm.isActiveNetworkMetered
    }

    /** True if anything is running or due to start now (scheduled downloads don't count). */
    fun hasWork(snap: JSONObject): Boolean {
        val items = snap.optJSONArray("items") ?: return false
        val now = System.currentTimeMillis() / 1000
        for (i in 0 until items.length()) {
            val o = items.getJSONObject(i)
            when (o.getString("status")) {
                "Active" -> return true
                "Queued" -> if (o.optLong("startAt", 0) <= now) return true
            }
        }
        return false
    }
}
