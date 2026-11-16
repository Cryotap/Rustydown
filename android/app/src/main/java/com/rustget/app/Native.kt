package com.rustget.app

/** JNI bridge to the Rust core (librustget.so). */
object Native {
    init { System.loadLibrary("rustget") }

    @JvmStatic external fun init(dataDir: String, defaultDir: String)

    /** JSON: {items:[…], totalSpeed, active, queued, done, failed} */
    @JvmStatic external fun snapshot(): String

    /** Takes a JSON download request. Returns "ok:<id>" or "error:<message>". */
    @JvmStatic external fun add(json: String): String

    @JvmStatic external fun pause(id: Long)
    @JvmStatic external fun resume(id: Long)
    @JvmStatic external fun remove(id: Long, deleteFile: Boolean)
    @JvmStatic external fun pauseAll()
    @JvmStatic external fun resumeAll()
    @JvmStatic external fun clearFinished()
    @JvmStatic external fun setLimit(id: Long, kbps: Int)

    @JvmStatic external fun getSettings(): String
    /** "" on success, else an error message. */
    @JvmStatic external fun setSettings(json: String): String
    @JvmStatic external fun setMetered(metered: Boolean)

    /** Text box contents -> JSON array of jobs, each [primary, mirrors…]. */
    @JvmStatic external fun parseLines(text: String): String
    /** Any text -> JSON array of http(s) links inside it. */
    @JvmStatic external fun urlsIn(text: String): String
    /** Blocking network call. Returns {"links":[…]} or {"error":"…"}. */
    @JvmStatic external fun fetchLinks(url: String, userAgent: String): String
}
