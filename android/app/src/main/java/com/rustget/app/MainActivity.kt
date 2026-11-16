package com.rustget.app

import android.Manifest
import android.app.Activity
import android.app.AlertDialog
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.Typeface
import android.graphics.drawable.GradientDrawable
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Environment
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import android.text.Editable
import android.text.InputType
import android.text.TextWatcher
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.widget.Button
import android.widget.CheckBox
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.ProgressBar
import android.widget.ScrollView
import android.widget.TextView
import android.widget.Toast
import org.json.JSONArray
import org.json.JSONObject

/** The whole UI, built in code. All downloading happens in the Rust engine behind [Native]. */
class MainActivity : Activity() {

    private val BG = Color.parseColor("#121317")
    private val CARD = Color.parseColor("#1E2026")
    private val FIELD = Color.parseColor("#2A2D35")
    private val BTN = Color.parseColor("#363A45")
    private val ACCENT = Color.parseColor("#E8590C")
    private val TEXT = Color.WHITE
    private val MUTED = Color.parseColor("#9AA0A6")
    private val GOOD = Color.parseColor("#46BE78")
    private val BAD = Color.parseColor("#EB6464")

    private val ui = Handler(Looper.getMainLooper())
    private val panes = LinkedHashMap<String, View>()
    private val tabs = LinkedHashMap<String, TextView>()
    private var current = "downloads"

    private lateinit var speedText: TextView
    private lateinit var banner: LinearLayout
    private lateinit var bannerText: TextView
    private lateinit var list: LinearLayout
    private lateinit var empty: TextView
    private val chips = LinkedHashMap<String, TextView>()
    private var filter = "all"
    private val rows = HashMap<Long, Row>()
    private var knownUrls = HashSet<String>()
    private var lastClip = ""
    private var clipWatch = true
    private var userAgent = ""

    // add pane
    private lateinit var addUrls: EditText
    private lateinit var addName: EditText
    private lateinit var addReferer: EditText
    private lateinit var addCookie: EditText
    private lateinit var addUser: EditText
    private lateinit var addPass: EditText
    private lateinit var addHeaders: EditText
    private lateinit var addChecksum: EditText
    private lateinit var addConns: EditText
    private lateinit var addLimit: EditText
    private lateinit var addDelay: EditText

    // grabber pane
    private lateinit var grabUrl: EditText
    private lateinit var grabFilter: EditText
    private lateinit var grabStatus: TextView
    private lateinit var grabList: LinearLayout
    private val grabLinks = ArrayList<String>()
    private val grabTicked = HashSet<String>()

    // settings pane
    private lateinit var setDir: EditText
    private lateinit var setActive: EditText
    private lateinit var setConns: EditText
    private lateinit var setRetries: EditText
    private lateinit var setLimit: EditText
    private lateinit var setUa: EditText
    private lateinit var setMetered: CheckBox
    private lateinit var setClip: CheckBox
    private lateinit var setSort: CheckBox
    private var settingsJson = JSONObject()

    // ───────────── small view helpers ─────────────

    private fun dp(v: Int) = (v * resources.displayMetrics.density).toInt()

    private fun round(color: Int, r: Int = 10) = GradientDrawable().apply {
        setColor(color)
        cornerRadius = dp(r).toFloat()
    }

    private fun lp(w: Int = ViewGroup.LayoutParams.MATCH_PARENT, h: Int = ViewGroup.LayoutParams.WRAP_CONTENT,
                   top: Int = 0, weight: Float = 0f) =
        LinearLayout.LayoutParams(w, h, weight).apply { topMargin = dp(top) }

    private fun label(text: String, size: Float = 14f, color: Int = TEXT, bold: Boolean = false) =
        TextView(this).apply {
            this.text = text
            textSize = size
            setTextColor(color)
            if (bold) typeface = Typeface.DEFAULT_BOLD
        }

    private fun button(text: String, fill: Int = BTN, onClick: () -> Unit) = Button(this).apply {
        this.text = text
        isAllCaps = false
        setTextColor(TEXT)
        textSize = 13f
        background = round(fill, 8)
        minHeight = 0
        minimumHeight = dp(40)
        minWidth = 0
        minimumWidth = dp(64)
        setPadding(dp(14), dp(6), dp(14), dp(6))
        setOnClickListener { onClick() }
    }

    private fun edit(hint: String, lines: Int = 1, type: Int = InputType.TYPE_CLASS_TEXT) = EditText(this).apply {
        this.hint = hint
        setHintTextColor(MUTED)
        setTextColor(TEXT)
        textSize = 14f
        background = round(FIELD, 8)
        setPadding(dp(12), dp(10), dp(12), dp(10))
        if (lines > 1) {
            inputType = type or InputType.TYPE_TEXT_FLAG_MULTI_LINE
            minLines = lines
            gravity = Gravity.TOP or Gravity.START
        } else {
            inputType = type
            setSingleLine(true)
        }
    }

    private fun numberEdit(hint: String) = edit(hint, 1, InputType.TYPE_CLASS_NUMBER)

    private fun vbox(pad: Int = 0) = LinearLayout(this).apply {
        orientation = LinearLayout.VERTICAL
        setPadding(dp(pad), dp(pad), dp(pad), dp(pad))
    }

    private fun hbox() = LinearLayout(this).apply { orientation = LinearLayout.HORIZONTAL }

    private fun scroll(inner: View) = ScrollView(this).apply {
        isFillViewport = false
        addView(inner)
    }

    private fun toast(msg: String) = Toast.makeText(this, msg, Toast.LENGTH_SHORT).show()

    private fun intOf(e: EditText, fallback: Int = 0) = e.text.toString().trim().toIntOrNull() ?: fallback

    // ───────────── segment map ─────────────

    private inner class SegBar(ctx: Context) : View(ctx) {
        var total = 0L
        var segs: JSONArray = JSONArray()
        private val p = Paint()
        override fun onDraw(c: Canvas) {
            p.color = Color.parseColor("#373B45")
            c.drawRect(0f, 0f, width.toFloat(), height.toFloat(), p)
            if (total <= 0) return
            p.color = ACCENT
            for (i in 0 until segs.length()) {
                val s = segs.getJSONArray(i)
                val x0 = width * (s.getLong(0).toFloat() / total)
                val x1 = width * (s.getLong(1).toFloat() / total)
                if (x1 > x0) c.drawRect(x0, 0f, maxOf(x1, x0 + 1f), height.toFloat(), p)
            }
        }
    }

    // ───────────── one download card ─────────────

    private inner class Row(val id: Long) {
        val view = vbox(12)
        val name = label("", 16f, TEXT, true)
        val status = label("", 13f, MUTED)
        val bar = ProgressBar(this@MainActivity, null, android.R.attr.progressBarStyleHorizontal).apply { max = 1000 }
        val actions = hbox()
        val primary: Button
        val detailsBtn: Button
        val details = label("", 12f, MUTED)
        val segBar = SegBar(this@MainActivity)
        val detailBox = vbox()
        var expanded = false
        var st = ""
        var limit = 0

        init {
            view.tag = id
            view.background = round(CARD)
            view.addView(name)
            view.addView(status, lp(top = 4))
            view.addView(bar, lp(h = dp(10), top = 8))
            primary = button("Pause") {
                when (st) {
                    "Active", "Queued" -> Native.pause(id)
                    else -> {
                        Native.resume(id)
                        DownloadService.start(this@MainActivity)
                    }
                }
            }
            detailsBtn = button("Details") { toggleDetails() }
            actions.addView(primary)
            actions.addView(detailsBtn, lp(w = ViewGroup.LayoutParams.WRAP_CONTENT).apply { leftMargin = dp(8) })
            actions.addView(button("Remove") { confirmRemove(id) },
                lp(w = ViewGroup.LayoutParams.WRAP_CONTENT).apply { leftMargin = dp(8) })
            view.addView(actions, lp(top = 10))

            detailBox.visibility = View.GONE
            detailBox.addView(details)
            detailBox.addView(segBar, lp(h = dp(10), top = 8))
            detailBox.addView(button("Speed limit") { askLimit(id, limit) }, lp(w = ViewGroup.LayoutParams.WRAP_CONTENT, top = 8))
            view.addView(detailBox, lp(top = 8))
        }

        fun toggleDetails() {
            expanded = !expanded
            detailBox.visibility = if (expanded) View.VISIBLE else View.GONE
            detailsBtn.text = if (expanded) "Hide" else "Details"
        }

        fun bind(o: JSONObject) {
            st = o.getString("status")
            limit = o.optInt("limitKbps", 0)
            name.text = o.getString("name")
            val total = if (o.isNull("total")) -1L else o.getLong("total")
            val done = o.getLong("done")
            val now = System.currentTimeMillis() / 1000
            val phase = o.optString("phase", "")
            var color = MUTED
            val line: String = when (st) {
                "Active" -> if (phase.isNotEmpty()) phase else {
                    var t = if (total >= 0) "${Util.bytes(done)} / ${Util.bytes(total)}" else Util.bytes(done)
                    t += "  ·  " + Util.speed(o.getDouble("speed"))
                    if (!o.isNull("eta")) t += "  ·  " + Util.eta(o.getLong("eta")) + " left"
                    if (o.getInt("conns") > 1) t += "  ·  ${o.getInt("conns")} connections"
                    t
                }
                "Queued" -> {
                    val at = o.optLong("startAt", 0)
                    if (at > now) "Scheduled: starts in " + Util.eta(at - now)
                    else if (phase.isNotEmpty()) phase else "Queued"
                }
                "Paused" -> if (total >= 0) "Paused  ·  ${Util.bytes(done)} / ${Util.bytes(total)}" else "Paused  ·  ${Util.bytes(done)}"
                "Failed" -> { color = BAD; o.optString("error", "Failed") }
                else -> {
                    color = GOOD
                    var t = Util.bytes(if (total >= 0) total else done)
                    if (!o.isNull("verified")) {
                        if (o.getBoolean("verified")) t += "  ·  checksum OK"
                        else { t += "  ·  CHECKSUM MISMATCH"; color = BAD }
                    }
                    t
                }
            }
            status.text = line
            status.setTextColor(color)

            bar.visibility = if (st != "Done" && total > 0) View.VISIBLE else View.GONE
            if (total > 0) bar.progress = (done * 1000 / total).toInt().coerceIn(0, 1000)

            primary.visibility = if (st == "Done") View.GONE else View.VISIBLE
            primary.text = when (st) {
                "Active", "Queued" -> "Pause"
                "Failed" -> "Retry"
                else -> "Resume"
            }

            if (expanded) {
                val sb = StringBuilder()
                sb.append(o.getString("url")).append("\n")
                if (o.getInt("mirrors") > 0) sb.append("${o.getInt("mirrors")} mirror(s) used if this one fails\n")
                val fp = o.optString("finalPath", "")
                sb.append(if (fp.isNotEmpty()) "Saved to $fp" else "Saving to ${o.getString("dir")}").append("\n")
                if (st != "Done" && total >= 0 && !o.getBoolean("ranges"))
                    sb.append("This server can't resume or split downloads: single connection.\n")
                if (limit > 0) sb.append("Speed limit: $limit KB/s\n")
                val err = o.optString("error", "")
                if (st == "Done" && err.isNotEmpty()) sb.append(err)
                details.text = sb.toString().trimEnd()
                val segs = o.getJSONArray("segs")
                segBar.total = if (total > 0) total else 0
                segBar.segs = segs
                segBar.visibility = if (st != "Done" && segs.length() > 0 && total > 0) View.VISIBLE else View.GONE
                segBar.invalidate()
            }
        }
    }

    // ───────────── lifecycle ─────────────

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        window.statusBarColor = BG
        window.navigationBarColor = BG

        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setBackgroundColor(BG)
        }

        val top = hbox().apply { setPadding(dp(16), dp(12), dp(16), dp(8)); gravity = Gravity.CENTER_VERTICAL }
        top.addView(label("RustGet", 22f, ACCENT, true), lp(w = 0, weight = 1f))
        speedText = label("Idle", 13f, MUTED)
        top.addView(speedText)
        root.addView(top)

        val content = android.widget.FrameLayout(this)
        root.addView(content, lp(h = 0, weight = 1f))

        panes["downloads"] = buildDownloads()
        panes["add"] = buildAdd()
        panes["grab"] = buildGrab()
        panes["settings"] = buildSettings()
        for ((_, p) in panes) content.addView(p, ViewGroup.LayoutParams(-1, -1))

        val nav = hbox().apply { setBackgroundColor(Color.parseColor("#181A1F")) }
        for ((key, title) in listOf("downloads" to "Downloads", "add" to "Add", "grab" to "Grabber", "settings" to "Settings")) {
            val t = label(title, 14f, MUTED, true).apply {
                gravity = Gravity.CENTER
                setPadding(0, dp(14), 0, dp(14))
                setOnClickListener { show(key) }
            }
            tabs[key] = t
            nav.addView(t, lp(w = 0, weight = 1f))
        }
        root.addView(nav)
        setContentView(root)

        show("downloads")
        loadSettings()
        handleIntent(intent)
        if (Build.VERSION.SDK_INT >= 33 &&
            checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 1)
    }

    override fun onNewIntent(intent: Intent?) {
        super.onNewIntent(intent)
        setIntent(intent)
        handleIntent(intent)
    }

    private val tick = object : Runnable {
        override fun run() {
            refresh()
            ui.postDelayed(this, 500)
        }
    }

    override fun onResume() {
        super.onResume()
        tick.run()
    }

    override fun onPause() {
        ui.removeCallbacks(tick)
        super.onPause()
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (hasFocus && clipWatch) checkClipboard()
    }

    // ───────────── navigation ─────────────

    private fun show(key: String) {
        current = key
        for ((k, p) in panes) p.visibility = if (k == key) View.VISIBLE else View.GONE
        for ((k, t) in tabs) {
            t.setTextColor(if (k == key) ACCENT else MUTED)
        }
        if (key == "settings") loadSettings()
    }

    private fun handleIntent(i: Intent?) {
        if (i == null) return
        val text = when (i.action) {
            Intent.ACTION_SEND -> i.getStringExtra(Intent.EXTRA_TEXT)
            Intent.ACTION_VIEW -> i.dataString
            else -> null
        } ?: return
        val urls = JSONArray(Native.urlsIn(text))
        if (urls.length() > 0) {
            val sb = StringBuilder()
            for (n in 0 until urls.length()) sb.append(urls.getString(n)).append("\n")
            addUrls.setText(sb.toString().trim())
            show("add")
        }
    }

    // ───────────── downloads pane ─────────────

    private fun buildDownloads(): View {
        val box = vbox(12)

        banner = vbox(12).apply { background = round(Color.parseColor("#282C38")); visibility = View.GONE }
        banner.addView(label("Link on your clipboard", 14f, TEXT, true))
        bannerText = label("", 12f, MUTED)
        banner.addView(bannerText, lp(top = 2))
        val br = hbox()
        br.addView(button("Download") {
            val u = bannerText.text.toString()
            val r = Native.add(JSONObject().put("urls", JSONArray().put(u)).toString())
            toast(if (r.startsWith("ok:")) "Added to queue" else r.removePrefix("error:"))
            banner.visibility = View.GONE
            refresh()
        })
        br.addView(button("Dismiss") { banner.visibility = View.GONE }, lp(w = ViewGroup.LayoutParams.WRAP_CONTENT).apply { leftMargin = dp(8) })
        banner.addView(br, lp(top = 8))
        box.addView(banner, lp(top = 0))

        val chipRow = hbox()
        for ((key, title) in listOf("all" to "All", "active" to "Active", "done" to "Done", "failed" to "Failed")) {
            val c = label(title, 13f, MUTED, true).apply {
                gravity = Gravity.CENTER
                setPadding(dp(12), dp(8), dp(12), dp(8))
                setOnClickListener { filter = key; refresh() }
            }
            chips[key] = c
            chipRow.addView(c, lp(w = 0, weight = 1f).apply { rightMargin = dp(6) })
        }
        box.addView(chipRow, lp(top = 8))

        val actions = hbox()
        actions.addView(button("Pause all") { Native.pauseAll() })
        actions.addView(button("Resume all") {
            Native.resumeAll()
            DownloadService.start(this)
        }, lp(w = ViewGroup.LayoutParams.WRAP_CONTENT).apply { leftMargin = dp(8) })
        actions.addView(button("Clear done") { Native.clearFinished() }, lp(w = ViewGroup.LayoutParams.WRAP_CONTENT).apply { leftMargin = dp(8) })
        box.addView(actions, lp(top = 8))

        list = vbox()
        empty = label("Nothing here yet\nCopy a link, or open the Add tab", 15f, MUTED).apply {
            gravity = Gravity.CENTER
            setPadding(0, dp(48), 0, 0)
        }
        list.addView(empty)
        val sv = scroll(list)
        box.addView(sv, lp(h = 0, top = 8, weight = 1f))
        return box
    }

    private fun refresh() {
        val snap = try { JSONObject(Native.snapshot()) } catch (e: Exception) { return }
        val items = snap.getJSONArray("items")
        val active = snap.getInt("active")
        val queued = snap.getInt("queued")

        speedText.text = when {
            active > 0 -> Util.speed(snap.getDouble("totalSpeed")) + "  ·  $active active"
            queued > 0 -> "$queued queued"
            else -> "Idle"
        }
        for ((k, c) in chips) {
            val n = when (k) {
                "all" -> items.length()
                "active" -> active + queued + countStatus(items, "Paused")
                "done" -> snap.getInt("done")
                else -> snap.getInt("failed")
            }
            val title = when (k) { "all" -> "All"; "active" -> "Active"; "done" -> "Done"; else -> "Failed" }
            c.text = "$title $n"
            c.background = round(if (k == filter) ACCENT else CARD, 16)
            c.setTextColor(if (k == filter) Color.WHITE else MUTED)
        }

        knownUrls = HashSet()
        val byId = HashMap<Long, JSONObject>()
        val order = ArrayList<Long>()
        for (i in items.length() - 1 downTo 0) { // newest first
            val o = items.getJSONObject(i)
            knownUrls.add(o.getString("url"))
            val st = o.getString("status")
            val ok = when (filter) {
                "all" -> true
                "active" -> st == "Active" || st == "Queued" || st == "Paused"
                "done" -> st == "Done"
                else -> st == "Failed"
            }
            if (ok) {
                val id = o.getLong("id")
                byId[id] = o
                order.add(id)
            }
        }

        // drop rows that are gone, create missing ones
        val gone = rows.keys.filter { !byId.containsKey(it) }
        for (id in gone) rows.remove(id)
        for (id in order) if (!rows.containsKey(id)) rows[id] = Row(id)

        val shown = ArrayList<Long>()
        for (n in 0 until list.childCount) (list.getChildAt(n).tag as? Long)?.let { shown.add(it) }
        if (shown != order) {
            list.removeAllViews()
            for (id in order) list.addView(rows[id]!!.view, lp(top = 8))
            if (order.isEmpty()) list.addView(empty)
        }
        for (id in order) rows[id]!!.bind(byId[id]!!)

        if (Util.hasWork(snap) && !DownloadService.running) DownloadService.start(this)
    }

    private fun countStatus(items: JSONArray, status: String): Int {
        var n = 0
        for (i in 0 until items.length()) if (items.getJSONObject(i).getString("status") == status) n++
        return n
    }

    private fun confirmRemove(id: Long) {
        val cb = CheckBox(this).apply { text = "Also delete the file from storage"; setPadding(dp(20), dp(8), dp(20), 0) }
        AlertDialog.Builder(this)
            .setTitle("Remove download?")
            .setView(cb)
            .setPositiveButton("Remove") { _, _ -> Native.remove(id, cb.isChecked); refresh() }
            .setNegativeButton("Cancel", null)
            .show()
    }

    private fun askLimit(id: Long, current: Int) {
        val e = numberEdit("KB/s, 0 = no limit").apply { setText(current.toString()) }
        AlertDialog.Builder(this)
            .setTitle("Speed limit (KB/s)")
            .setView(e)
            .setPositiveButton("Set") { _, _ -> Native.setLimit(id, intOf(e)) }
            .setNegativeButton("Cancel", null)
            .show()
    }

    private fun checkClipboard() {
        try {
            val cm = getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
            val t = cm.primaryClip?.getItemAt(0)?.coerceToText(this)?.toString() ?: return
            if (t == lastClip) return
            lastClip = t
            val urls = JSONArray(Native.urlsIn(t))
            if (urls.length() == 0) return
            val u = urls.getString(0)
            if (knownUrls.contains(u)) return
            bannerText.text = u
            banner.visibility = View.VISIBLE
        } catch (_: Exception) {
        }
    }

    // ───────────── add pane ─────────────

    private fun buildAdd(): View {
        val box = vbox(12)
        box.addView(label("Add downloads", 20f, TEXT, true))
        box.addView(label("One link per line.  a | b  adds b as a mirror of a.  img[001-020].jpg  expands to a numbered batch.", 12f, MUTED), lp(top = 4))
        addUrls = edit("https://example.com/file.zip", 6, InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_URI)
        box.addView(addUrls, lp(top = 8))

        val row = hbox()
        row.addView(button("Paste") {
            val cm = getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
            val t = cm.primaryClip?.getItemAt(0)?.coerceToText(this)?.toString()?.trim()
            if (t.isNullOrEmpty()) toast("Clipboard is empty") else {
                val cur = addUrls.text.toString()
                addUrls.setText(if (cur.isEmpty()) t else cur.trimEnd() + "\n" + t)
            }
        })
        row.addView(button("Clear") { addUrls.setText("") }, lp(w = ViewGroup.LayoutParams.WRAP_CONTENT).apply { leftMargin = dp(8) })
        box.addView(row, lp(top = 8))

        val adv = vbox()
        adv.visibility = View.GONE
        addName = edit("Save as (single link only)")
        addReferer = edit("Referer")
        addCookie = edit("Cookie  name=value; other=value")
        addUser = edit("HTTP username")
        addPass = edit("HTTP password", 1, InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD)
        addHeaders = edit("Extra headers, one per line  Name: value", 2)
        addChecksum = edit("Expected checksum (MD5 / SHA-1 / SHA-256)")
        addConns = numberEdit("Connections (0 = default)")
        addLimit = numberEdit("Speed limit KB/s (0 = none)")
        addDelay = numberEdit("Start in N minutes (0 = now)")
        for (v in listOf(addName, addReferer, addCookie, addUser, addPass, addHeaders, addChecksum, addConns, addLimit, addDelay))
            adv.addView(v, lp(top = 8))
        val toggle = button("Advanced options") {
            adv.visibility = if (adv.visibility == View.GONE) View.VISIBLE else View.GONE
        }
        box.addView(toggle, lp(w = ViewGroup.LayoutParams.WRAP_CONTENT, top = 12))
        box.addView(adv)

        box.addView(button("Add to queue", ACCENT) { submitAdd() }.apply { textSize = 16f; minimumHeight = dp(48) }, lp(top = 16))
        return scroll(box)
    }

    private fun submitAdd() {
        val jobs = JSONArray(Native.parseLines(addUrls.text.toString()))
        if (jobs.length() == 0) { toast("Paste or type a link first"); return }

        val headers = JSONArray()
        fun hdr(k: String, v: String) { headers.put(JSONArray().put(k).put(v)) }
        if (addReferer.text.isNotBlank()) hdr("Referer", addReferer.text.toString().trim())
        if (addCookie.text.isNotBlank()) hdr("Cookie", addCookie.text.toString().trim())
        if (addUser.text.isNotBlank()) {
            val cred = addUser.text.toString().trim() + ":" + addPass.text.toString()
            hdr("Authorization", "Basic " + android.util.Base64.encodeToString(cred.toByteArray(), android.util.Base64.NO_WRAP))
        }
        for (line in addHeaders.text.toString().lines()) {
            val i = line.indexOf(':')
            if (i > 0) hdr(line.substring(0, i).trim(), line.substring(i + 1).trim())
        }

        val single = jobs.length() == 1
        var added = 0
        var firstErr: String? = null
        for (n in 0 until jobs.length()) {
            val req = JSONObject()
                .put("urls", jobs.getJSONArray(n))
                .put("headers", headers)
                .put("name", if (single) addName.text.toString() else "")
                .put("checksum", if (single) addChecksum.text.toString() else "")
                .put("conns", intOf(addConns))
                .put("limit_kbps", intOf(addLimit))
                .put("start_in_secs", intOf(addDelay) * 60L)
            val r = Native.add(req.toString())
            if (r.startsWith("ok:")) added++ else if (firstErr == null) firstErr = r.removePrefix("error:")
        }
        toast(
            when {
                added == 0 -> firstErr ?: "Nothing added"
                firstErr != null -> "Added $added. Skipped: $firstErr"
                else -> "Added $added download" + (if (added == 1) "" else "s")
            }
        )
        if (added > 0) {
            for (e in listOf(addUrls, addName, addChecksum)) e.setText("")
            DownloadService.start(this)
            show("downloads")
            refresh()
        }
    }

    // ───────────── link grabber pane ─────────────

    private fun buildGrab(): View {
        val box = vbox(12)
        box.addView(label("Link grabber", 20f, TEXT, true))
        box.addView(label("Scan a web page for downloadable links, tick the ones you want, add them all at once.", 12f, MUTED), lp(top = 4))
        grabUrl = edit("https://example.com/downloads", 1, InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_URI)
        box.addView(grabUrl, lp(top = 8))
        val row = hbox()
        row.addView(button("Paste") {
            val cm = getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
            val t = cm.primaryClip?.getItemAt(0)?.coerceToText(this)?.toString()?.trim()
            if (!t.isNullOrEmpty()) {
                val u = JSONArray(Native.urlsIn(t))
                grabUrl.setText(if (u.length() > 0) u.getString(0) else t)
            }
        })
        row.addView(button("Find links", ACCENT) { findLinks() }, lp(w = ViewGroup.LayoutParams.WRAP_CONTENT).apply { leftMargin = dp(8) })
        box.addView(row, lp(top = 8))
        grabStatus = label("", 13f, MUTED)
        box.addView(grabStatus, lp(top = 8))

        grabFilter = edit("Filter, e.g.  .zip .pdf .mp4")
        grabFilter.addTextChangedListener(object : TextWatcher {
            override fun afterTextChanged(s: Editable?) { renderGrab() }
            override fun beforeTextChanged(s: CharSequence?, a: Int, b: Int, c: Int) {}
            override fun onTextChanged(s: CharSequence?, a: Int, b: Int, c: Int) {}
        })
        box.addView(grabFilter, lp(top = 8))
        val r2 = hbox()
        r2.addView(button("Tick shown") { for (u in shownLinks()) grabTicked.add(u); renderGrab() })
        r2.addView(button("Untick all") { grabTicked.clear(); renderGrab() }, lp(w = ViewGroup.LayoutParams.WRAP_CONTENT).apply { leftMargin = dp(8) })
        r2.addView(button("Add ticked", ACCENT) { addTicked() }, lp(w = ViewGroup.LayoutParams.WRAP_CONTENT).apply { leftMargin = dp(8) })
        box.addView(r2, lp(top = 8))

        grabList = vbox()
        box.addView(grabList, lp(top = 8))
        return scroll(box)
    }

    private fun findLinks() {
        val page = grabUrl.text.toString().trim()
        if (page.isEmpty()) { toast("Enter a page address"); return }
        grabStatus.text = "Fetching page…"
        grabLinks.clear(); grabTicked.clear(); renderGrab()
        Thread {
            val res = try { JSONObject(Native.fetchLinks(page, userAgent)) } catch (e: Exception) { JSONObject().put("error", e.toString()) }
            runOnUiThread {
                if (res.has("error")) {
                    grabStatus.text = "Failed: " + res.getString("error")
                } else {
                    val a = res.getJSONArray("links")
                    for (i in 0 until a.length()) grabLinks.add(a.getString(i))
                    grabStatus.text = "${grabLinks.size} links found"
                    renderGrab()
                }
            }
        }.start()
    }

    private fun shownLinks(): List<String> {
        val terms = grabFilter.text.toString().lowercase().split(Regex("\\s+")).filter { it.isNotEmpty() }
        if (terms.isEmpty()) return grabLinks
        return grabLinks.filter { u -> val l = u.lowercase(); terms.any { l.contains(it) } }
    }

    private fun renderGrab() {
        grabList.removeAllViews()
        val shown = shownLinks()
        for (u in shown.take(400)) {
            val cb = CheckBox(this).apply {
                text = u.substringAfterLast('/').substringBefore('?').ifEmpty { u }
                setTextColor(TEXT)
                isChecked = grabTicked.contains(u)
                setOnCheckedChangeListener { _, on -> if (on) grabTicked.add(u) else grabTicked.remove(u) }
            }
            grabList.addView(cb)
            grabList.addView(label(u, 11f, MUTED), lp(top = 0))
        }
        if (shown.size > 400) grabList.addView(label("…and ${shown.size - 400} more. Narrow the filter to see them.", 12f, MUTED))
    }

    private fun addTicked() {
        var added = 0
        var skipped = 0
        for (u in grabTicked.toList()) {
            val r = Native.add(JSONObject().put("urls", JSONArray().put(u)).toString())
            if (r.startsWith("ok:")) added++ else skipped++
        }
        grabTicked.clear()
        renderGrab()
        toast("Added $added, skipped $skipped")
        if (added > 0) {
            DownloadService.start(this)
            show("downloads")
            refresh()
        }
    }

    // ───────────── settings pane ─────────────

    private fun buildSettings(): View {
        val box = vbox(12)
        box.addView(label("Settings", 20f, TEXT, true))

        box.addView(label("Save new downloads to", 13f, MUTED), lp(top = 12))
        setDir = edit("folder")
        box.addView(setDir, lp(top = 4))
        val fr = hbox()
        fr.addView(button("Public Downloads") { usePublicFolder() })
        fr.addView(button("App folder") {
            val d = getExternalFilesDir(Environment.DIRECTORY_DOWNLOADS) ?: filesDir
            setDir.setText(d.absolutePath)
        }, lp(w = ViewGroup.LayoutParams.WRAP_CONTENT).apply { leftMargin = dp(8) })
        box.addView(fr, lp(top = 8))
        box.addView(label("The public Downloads folder needs \"All files access\" on Android 11+. Tap the button, allow it, come back and tap again, then Save.", 12f, MUTED), lp(top = 4))

        setActive = numberEdit("Downloads at once (1-16)")
        setConns = numberEdit("Connections per file (1-32)")
        setRetries = numberEdit("Retries before giving up")
        setLimit = numberEdit("Total speed limit KB/s (0 = none)")
        for ((t, v) in listOf("Downloads at once" to setActive, "Connections per file" to setConns,
            "Retries before giving up" to setRetries, "Total speed limit, KB/s (0 = none)" to setLimit)) {
            box.addView(label(t, 13f, MUTED), lp(top = 12))
            box.addView(v, lp(top = 4))
        }
        setMetered = CheckBox(this).apply { text = "Wait for Wi-Fi (pause on mobile data)"; setTextColor(TEXT) }
        setClip = CheckBox(this).apply { text = "Offer to download links I copy"; setTextColor(TEXT) }
        setSort = CheckBox(this).apply { text = "Sort finished files into Video / Music / Documents / …"; setTextColor(TEXT) }
        box.addView(setMetered, lp(top = 12))
        box.addView(setClip)
        box.addView(setSort)

        box.addView(label("User-Agent", 13f, MUTED), lp(top = 12))
        setUa = edit("", 2)
        box.addView(setUa, lp(top = 4))

        box.addView(button("Save settings", ACCENT) { saveSettings() }.apply { minimumHeight = dp(48) }, lp(top = 16))
        box.addView(button("Retry all failed / paused") { Native.resumeAll(); DownloadService.start(this) }, lp(top = 8))
        return scroll(box)
    }

    private fun loadSettings() {
        val s = try { JSONObject(Native.getSettings()) } catch (e: Exception) { return }
        settingsJson = s
        clipWatch = s.optBoolean("clipboard_watch", true)
        userAgent = s.optString("user_agent", "")
        setDir.setText(s.optString("dir"))
        setActive.setText(s.optInt("max_active", 3).toString())
        setConns.setText(s.optInt("conns", 8).toString())
        setRetries.setText(s.optInt("retries", 8).toString())
        setLimit.setText(s.optInt("global_limit_kbps", 0).toString())
        setUa.setText(userAgent)
        setMetered.isChecked = s.optBoolean("pause_on_metered", false)
        setClip.isChecked = clipWatch
        setSort.isChecked = s.optBoolean("auto_sort", true)
    }

    private fun saveSettings() {
        val s = JSONObject(settingsJson.toString())
            .put("dir", setDir.text.toString().trim())
            .put("max_active", intOf(setActive, 3))
            .put("conns", intOf(setConns, 8))
            .put("retries", intOf(setRetries, 8))
            .put("global_limit_kbps", intOf(setLimit))
            .put("user_agent", setUa.text.toString().trim())
            .put("pause_on_metered", setMetered.isChecked)
            .put("clipboard_watch", setClip.isChecked)
            .put("auto_sort", setSort.isChecked)
        val err = Native.setSettings(s.toString())
        if (err.isEmpty()) { toast("Settings saved"); loadSettings() } else toast(err)
    }

    @Suppress("DEPRECATION")
    private fun usePublicFolder() {
        val target = Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOWNLOADS).absolutePath + "/RustGet"
        when {
            Build.VERSION.SDK_INT >= 30 -> {
                if (Environment.isExternalStorageManager()) {
                    setDir.setText(target)
                } else {
                    toast("Allow \"All files access\", then come back and tap again")
                    try {
                        startActivity(Intent(Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION, Uri.parse("package:$packageName")))
                    } catch (e: Exception) {
                        startActivity(Intent(Settings.ACTION_MANAGE_ALL_FILES_ACCESS_PERMISSION))
                    }
                }
            }
            Build.VERSION.SDK_INT >= 29 -> toast("Android 10 can't write to the public Downloads folder. Using the app folder.")
            else -> {
                if (checkSelfPermission(Manifest.permission.WRITE_EXTERNAL_STORAGE) != PackageManager.PERMISSION_GRANTED) {
                    requestPermissions(arrayOf(Manifest.permission.WRITE_EXTERNAL_STORAGE), 2)
                    toast("Allow storage access, then tap again")
                } else setDir.setText(target)
            }
        }
    }
}
