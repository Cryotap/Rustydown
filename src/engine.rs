//! The download engine.
//!
//! * Multi-connection segmented downloads with **dynamic re-segmentation**: when a connection
//!   runs out of work it cuts the biggest in-flight segment in half and takes the back half,
//!   so every connection stays busy until the very last byte.
//! * Resume across restarts (segment table persisted), with ETag / Last-Modified / size checks.
//! * Mirrors: extra URLs for the same file are used as automatic fall-backs.
//! * Per-download and global speed limits, exponential back-off retries, queue + scheduling,
//!   optional pause on metered networks, checksum verification, type-based folders.

use crate::logic::{self, Algo, Pacer, UNKNOWN_END};
use crate::net::{self, NetErr};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ───────────────────────── persisted model ─────────────────────────

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    Queued,
    Active,
    Paused,
    Done,
    Failed,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Seg {
    pub start: u64,
    pub pos: u64,
    pub end: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Download {
    pub id: u64,
    pub urls: Vec<String>,
    pub name: String,
    pub name_auto: bool,
    pub dir: String,
    pub status: Status,
    pub error: String,
    pub total: Option<u64>,
    pub ranges_ok: bool,
    pub segs: Vec<Seg>,
    pub bytes_done: u64,
    pub etag: String,
    pub last_modified: String,
    pub conns: u32,
    pub limit_kbps: u32,
    pub headers: Vec<(String, String)>,
    pub checksum: String,
    pub verified: Option<bool>,
    pub start_at: u64,
    pub added: u64,
    pub finished: u64,
    pub final_path: String,
    #[serde(skip)]
    pub phase: String,
    #[serde(skip)]
    pub force: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Settings {
    pub dir: String,
    pub max_active: u32,
    pub conns: u32,
    pub global_limit_kbps: u32,
    pub retries: u32,
    pub user_agent: String,
    pub pause_on_metered: bool,
    pub clipboard_watch: bool,
    pub auto_sort: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            dir: String::new(),
            max_active: 3,
            conns: 8,
            global_limit_kbps: 0,
            retries: 8,
            user_agent:
                "Mozilla/5.0 (Linux; Android 14; Mobile) AppleWebKit/537.36 (KHTML, like Gecko) \
                         Chrome/130.0 Mobile Safari/537.36 RustGet/0.1"
                    .to_string(),
            pause_on_metered: false,
            clipboard_watch: true,
            auto_sort: true,
        }
    }
}

// ───────────────────────── public API types ─────────────────────────

#[derive(Default, Clone)]
pub struct NewDownload {
    /// Primary URL first, then mirrors of the same file.
    pub urls: Vec<String>,
    pub name: String,
    pub headers: Vec<(String, String)>,
    pub checksum: String,
    pub conns: u32,
    pub limit_kbps: u32,
    pub start_in_secs: u64,
}

#[derive(Clone, Debug)]
pub struct View {
    pub id: u64,
    pub name: String,
    pub url: String,
    pub mirrors: usize,
    pub status: Status,
    pub error: String,
    pub phase: String,
    pub total: Option<u64>,
    pub done: u64,
    pub speed: f64,
    pub eta: Option<u64>,
    pub conns: u32,
    pub dir: String,
    pub final_path: String,
    pub limit_kbps: u32,
    pub verified: Option<bool>,
    pub segs: Vec<(u64, u64, u64)>,
    pub start_at: u64,
    pub ranges: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub items: Vec<View>,
    pub total_speed: f64,
    pub active: usize,
    pub queued: usize,
    pub done: usize,
    pub failed: usize,
}

// ───────────────────────── runtime state ─────────────────────────

struct SegRt {
    start: u64,
    pos: u64,
    end: u64,
    owned: bool,
}

struct Run {
    stop: AtomicBool,
    done: AtomicU64,
    err: Mutex<Option<String>>,
    segs: Mutex<Vec<SegRt>>,
    limit_bps: AtomicU64,
    pacer: Mutex<Pacer>,
}

impl Run {
    fn new() -> Self {
        Run {
            stop: AtomicBool::new(false),
            done: AtomicU64::new(0),
            err: Mutex::new(None),
            segs: Mutex::new(vec![]),
            limit_bps: AtomicU64::new(0),
            pacer: Mutex::new(Pacer::new()),
        }
    }
    fn stopped(&self) -> bool {
        self.stop.load(Relaxed)
    }
    fn fail(&self, msg: String) {
        let mut e = self.err.lock().unwrap();
        if e.is_none() {
            *e = Some(msg);
        }
        self.stop.store(true, Relaxed);
    }
}

struct Cfg {
    agent: ureq::Agent,
    urls: Vec<String>,
    headers: Vec<(String, String)>,
    file: File,
    ranges: bool,
    can_split: bool,
    retries: u32,
}

#[derive(Default, Clone)]
struct Live {
    last_done: u64,
    speed: f64,
    conns: u32,
}

enum Fin {
    Stopped,
    Complete,
    Failed(String),
}

struct Inner {
    dl: Mutex<Vec<Download>>,
    runs: Mutex<HashMap<u64, Arc<Run>>>,
    live: Mutex<HashMap<u64, Live>>,
    settings: Mutex<Settings>,
    next_id: AtomicU64,
    pacer: Mutex<Pacer>,
    global_limit: AtomicU64,
    epoch: Instant,
    data_dir: PathBuf,
    dirty: AtomicBool,
    metered: AtomicBool,
}

#[derive(Clone)]
pub struct Manager {
    inner: Arc<Inner>,
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn backoff(attempt: u32) -> Duration {
    Duration::from_millis((500u64 << attempt.min(6)).min(20_000))
}

/// Sleep in small slices so pause/cancel is instant. Returns false if stopped meanwhile.
fn sleep_stoppable(run: &Run, total: Duration) -> bool {
    let end = Instant::now() + total;
    loop {
        if run.stopped() {
            return false;
        }
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return true;
        }
        thread::sleep(left.min(Duration::from_millis(100)));
    }
}

fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    for i in 1..10_000 {
        let p = dir.join(format!("{stem} ({i}){ext}"));
        if !p.exists() {
            return p;
        }
    }
    first
}

fn hash_with<D: Digest>(mut d: D, f: &mut File) -> std::io::Result<String> {
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        d.update(&buf[..n]);
    }
    Ok(logic::hex(&d.finalize()[..]))
}

fn hash_file(path: &Path, algo: Algo) -> std::io::Result<String> {
    let mut f = File::open(path)?;
    match algo {
        Algo::Md5 => hash_with(md5::Md5::new(), &mut f),
        Algo::Sha1 => hash_with(sha1::Sha1::new(), &mut f),
        Algo::Sha256 => hash_with(sha2::Sha256::new(), &mut f),
    }
}

// ───────────────────────── manager ─────────────────────────

impl Manager {
    pub fn new(data_dir: PathBuf, default_download_dir: String) -> Manager {
        let _ = fs::create_dir_all(&data_dir);

        let mut settings: Settings = fs::read(data_dir.join("settings.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        if settings.dir.is_empty() {
            settings.dir = default_download_dir;
        }

        let mut list: Vec<Download> = fs::read(data_dir.join("downloads.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        // anything that was running when we last exited simply continues
        for d in list.iter_mut() {
            if d.status == Status::Active {
                d.status = Status::Queued;
            }
        }
        let next = list.iter().map(|d| d.id).max().unwrap_or(0) + 1;

        let inner = Arc::new(Inner {
            dl: Mutex::new(list),
            runs: Mutex::new(HashMap::new()),
            live: Mutex::new(HashMap::new()),
            global_limit: AtomicU64::new(settings.global_limit_kbps as u64 * 1024),
            settings: Mutex::new(settings),
            next_id: AtomicU64::new(next),
            pacer: Mutex::new(Pacer::new()),
            epoch: Instant::now(),
            data_dir,
            dirty: AtomicBool::new(true),
            metered: AtomicBool::new(false),
        });

        let sched = inner.clone();
        thread::Builder::new()
            .name("scheduler".into())
            .spawn(move || {
                let mut last = Instant::now();
                let mut since_save = 0u32;
                loop {
                    let dt = last.elapsed().as_secs_f64().max(0.05);
                    last = Instant::now();
                    sched.tick(dt);
                    since_save += 1;
                    if since_save >= 5 {
                        since_save = 0;
                        let any_active = sched
                            .dl
                            .lock()
                            .unwrap()
                            .iter()
                            .any(|d| d.status == Status::Active);
                        if any_active || sched.dirty.load(Relaxed) {
                            sched.save();
                        }
                    }
                    thread::sleep(Duration::from_millis(400));
                }
            })
            .expect("scheduler thread");

        Manager { inner }
    }

    pub fn settings(&self) -> Settings {
        self.inner.settings.lock().unwrap().clone()
    }

    pub fn set_settings(&self, mut s: Settings) {
        s.max_active = s.max_active.clamp(1, 16);
        s.conns = s.conns.clamp(1, 32);
        s.retries = s.retries.min(50);
        self.inner
            .global_limit
            .store(s.global_limit_kbps as u64 * 1024, Relaxed);
        if let Ok(json) = serde_json::to_vec_pretty(&s) {
            let _ = fs::write(self.inner.data_dir.join("settings.json"), json);
        }
        *self.inner.settings.lock().unwrap() = s;
    }

    pub fn set_metered(&self, metered: bool) {
        self.inner.metered.store(metered, Relaxed);
    }

    pub fn add(&self, n: NewDownload) -> Result<u64, String> {
        if n.urls.is_empty() {
            return Err("No link given".into());
        }
        for u in &n.urls {
            let low = u.to_ascii_lowercase();
            if !(low.starts_with("http://") || low.starts_with("https://")) {
                return Err(format!("Only http(s) links are supported: {u}"));
            }
            if url::Url::parse(u).is_err() {
                return Err(format!("Not a valid link: {u}"));
            }
        }
        let st = self.settings();
        let mut dl = self.inner.dl.lock().unwrap();
        if dl
            .iter()
            .any(|d| d.status != Status::Done && d.urls.first() == n.urls.first())
        {
            return Err("Already in the list".into());
        }
        let id = self.inner.next_id.fetch_add(1, Relaxed);
        let (name, name_auto) = if n.name.trim().is_empty() {
            (logic::filename_from_url(&n.urls[0]), true)
        } else {
            (logic::sanitize_filename(&n.name), false)
        };
        let now = now_secs();
        dl.push(Download {
            id,
            urls: n.urls,
            name,
            name_auto,
            dir: st.dir,
            status: Status::Queued,
            conns: n.conns,
            limit_kbps: n.limit_kbps,
            headers: n.headers,
            checksum: n.checksum.trim().to_string(),
            start_at: if n.start_in_secs > 0 {
                now + n.start_in_secs
            } else {
                0
            },
            added: now,
            ..Default::default()
        });
        self.inner.dirty.store(true, Relaxed);
        Ok(id)
    }

    pub fn pause(&self, id: u64) {
        self.inner.update(id, |d| {
            if matches!(d.status, Status::Active | Status::Queued) {
                d.status = Status::Paused;
                d.phase.clear();
                d.force = false;
            }
        });
        self.inner.stop_run(id);
    }

    /// Resume / retry / "start now": bypasses the queue limit and any schedule.
    pub fn resume(&self, id: u64) {
        self.inner.update(id, |d| {
            if matches!(d.status, Status::Paused | Status::Failed | Status::Queued) {
                d.status = Status::Queued;
                d.force = true;
                d.start_at = 0;
                d.error.clear();
            }
        });
    }

    pub fn pause_all(&self) {
        let ids: Vec<u64> = self
            .inner
            .dl
            .lock()
            .unwrap()
            .iter()
            .filter(|d| matches!(d.status, Status::Active | Status::Queued))
            .map(|d| d.id)
            .collect();
        for id in ids {
            self.pause(id);
        }
    }

    pub fn resume_all(&self) {
        let ids: Vec<u64> = self
            .inner
            .dl
            .lock()
            .unwrap()
            .iter()
            .filter(|d| matches!(d.status, Status::Paused | Status::Failed))
            .map(|d| d.id)
            .collect();
        for id in ids {
            self.inner.update(id, |d| {
                d.status = Status::Queued;
                d.start_at = 0;
                d.error.clear();
            });
        }
    }

    pub fn remove(&self, id: u64, delete_file: bool) {
        self.inner.stop_run(id);
        let removed = {
            let mut dl = self.inner.dl.lock().unwrap();
            dl.iter().position(|d| d.id == id).map(|i| dl.remove(i))
        };
        if let Some(d) = removed {
            let _ = fs::remove_file(
                Path::new(&d.dir)
                    .join(".rustget")
                    .join(format!("{id}.part")),
            );
            if delete_file && !d.final_path.is_empty() {
                let _ = fs::remove_file(&d.final_path);
            }
        }
        self.inner.dirty.store(true, Relaxed);
    }

    pub fn clear_finished(&self) {
        self.inner
            .dl
            .lock()
            .unwrap()
            .retain(|d| d.status != Status::Done);
        self.inner.dirty.store(true, Relaxed);
    }

    pub fn set_limit(&self, id: u64, kbps: u32) {
        self.inner.update(id, |d| d.limit_kbps = kbps);
        if let Some(run) = self.inner.runs.lock().unwrap().get(&id) {
            run.limit_bps.store(kbps as u64 * 1024, Relaxed);
        }
        self.inner.dirty.store(true, Relaxed);
    }

    pub fn snapshot(&self) -> Snapshot {
        let dl = self.inner.dl.lock().unwrap();
        let live = self.inner.live.lock().unwrap();
        let mut snap = Snapshot::default();
        for d in dl.iter() {
            let l = live.get(&d.id).cloned().unwrap_or_default();
            let active = d.status == Status::Active;
            let speed = if active { l.speed } else { 0.0 };
            let eta = match d.total {
                Some(t) if active && speed > 1.0 => {
                    Some((t.saturating_sub(d.bytes_done) as f64 / speed) as u64)
                }
                _ => None,
            };
            match d.status {
                Status::Active => snap.active += 1,
                Status::Queued => snap.queued += 1,
                Status::Done => snap.done += 1,
                Status::Failed => snap.failed += 1,
                Status::Paused => {}
            }
            snap.total_speed += speed;
            snap.items.push(View {
                id: d.id,
                name: d.name.clone(),
                url: d.urls.first().cloned().unwrap_or_default(),
                mirrors: d.urls.len().saturating_sub(1),
                status: d.status,
                error: d.error.clone(),
                phase: d.phase.clone(),
                total: d.total,
                done: d.bytes_done,
                speed,
                eta,
                conns: if active { l.conns } else { 0 },
                dir: d.dir.clone(),
                final_path: d.final_path.clone(),
                limit_kbps: d.limit_kbps,
                verified: d.verified,
                segs: d.segs.iter().map(|s| (s.start, s.pos, s.end)).collect(),
                start_at: d.start_at,
                ranges: d.ranges_ok,
            });
        }
        snap
    }
}

// ───────────────────────── scheduler ─────────────────────────

impl Inner {
    fn update<R>(&self, id: u64, f: impl FnOnce(&mut Download) -> R) -> Option<R> {
        let mut dl = self.dl.lock().unwrap();
        dl.iter_mut().find(|d| d.id == id).map(f)
    }

    fn get(&self, id: u64) -> Option<Download> {
        self.dl.lock().unwrap().iter().find(|d| d.id == id).cloned()
    }

    fn set_phase(&self, id: u64, p: &str) {
        self.update(id, |d| d.phase = p.to_string());
    }

    fn stop_run(&self, id: u64) {
        if let Some(run) = self.runs.lock().unwrap().get(&id) {
            run.stop.store(true, Relaxed);
        }
    }

    fn save(&self) {
        self.dirty.store(false, Relaxed);
        let json = {
            let dl = self.dl.lock().unwrap();
            serde_json::to_vec(&*dl)
        };
        if let Ok(json) = json {
            let tmp = self.data_dir.join("downloads.json.tmp");
            if fs::write(&tmp, json).is_ok() {
                let _ = fs::rename(&tmp, self.data_dir.join("downloads.json"));
            }
        }
    }

    fn tick(self: &Arc<Self>, dt: f64) {
        // 1. copy live progress from running downloads into the model and compute speeds
        let runs: Vec<(u64, Arc<Run>)> = self
            .runs
            .lock()
            .unwrap()
            .iter()
            .map(|(k, v)| (*k, v.clone()))
            .collect();
        {
            let mut dl = self.dl.lock().unwrap();
            let mut live = self.live.lock().unwrap();
            for (id, run) in &runs {
                let Some(d) = dl.iter_mut().find(|d| d.id == *id) else {
                    continue;
                };
                if d.status != Status::Active {
                    continue;
                }
                let done = run.done.load(Relaxed);
                let (segs, owned) = {
                    let s = run.segs.lock().unwrap();
                    (
                        s.iter()
                            .map(|x| Seg {
                                start: x.start,
                                pos: x.pos,
                                end: x.end,
                            })
                            .collect::<Vec<_>>(),
                        s.iter().filter(|x| x.owned).count() as u32,
                    )
                };
                d.segs = segs;
                d.bytes_done = done;
                let l = live.entry(*id).or_default();
                let inst = done.saturating_sub(l.last_done) as f64 / dt;
                l.speed = if l.speed <= 0.0 {
                    inst
                } else {
                    0.6 * l.speed + 0.4 * inst
                };
                l.last_done = done;
                l.conns = owned;
            }
        }

        // 2. decide what may start
        let st = self.settings.lock().unwrap().clone();
        let blocked = st.pause_on_metered && self.metered.load(Relaxed);
        let now = now_secs();
        let mut to_start = vec![];
        let mut to_stop = vec![];
        {
            let mut dl = self.dl.lock().unwrap();
            if blocked {
                for d in dl
                    .iter_mut()
                    .filter(|d| d.status == Status::Active && !d.force)
                {
                    d.status = Status::Queued;
                    d.phase = "Waiting for Wi-Fi".into();
                    to_stop.push(d.id);
                }
            }
            let active = dl.iter().filter(|d| d.status == Status::Active).count() as u32;
            let mut slots = st.max_active.saturating_sub(active);
            let running: Vec<u64> = self.runs.lock().unwrap().keys().copied().collect();
            for d in dl.iter_mut() {
                if d.status != Status::Queued || d.start_at > now {
                    continue;
                }
                if running.contains(&d.id) {
                    continue; // previous run still winding down
                }
                if d.force || (!blocked && slots > 0) {
                    if !d.force {
                        slots -= 1;
                    }
                    d.status = Status::Active;
                    d.force = false;
                    d.error.clear();
                    d.phase = "Starting…".into();
                    to_start.push(d.id);
                } else if blocked && d.phase.is_empty() {
                    d.phase = "Waiting for Wi-Fi".into();
                }
            }
        }
        for id in to_stop {
            self.stop_run(id);
        }
        for id in to_start {
            self.dirty.store(true, Relaxed);
            let me = self.clone();
            let _ = thread::Builder::new()
                .name(format!("dl-{id}"))
                .spawn(move || me.orchestrate(id));
        }
    }

    // ───────────────────── one download, start to finish ─────────────────────

    fn orchestrate(self: &Arc<Self>, id: u64) {
        let run = Arc::new(Run::new());
        self.runs.lock().unwrap().insert(id, run.clone());
        let fin = self.orchestrate_inner(id, &run);
        self.sync_from_run(id, &run);
        match fin {
            Fin::Stopped | Fin::Complete => {}
            Fin::Failed(msg) => {
                self.update(id, |d| {
                    if d.status == Status::Active {
                        d.status = Status::Failed;
                        d.error = msg;
                    }
                    d.phase.clear();
                });
            }
        }
        self.runs.lock().unwrap().remove(&id);
        self.live.lock().unwrap().remove(&id);
        self.dirty.store(true, Relaxed);
    }

    fn sync_from_run(&self, id: u64, run: &Run) {
        let segs: Vec<Seg> = run
            .segs
            .lock()
            .unwrap()
            .iter()
            .map(|x| Seg {
                start: x.start,
                pos: x.pos,
                end: x.end,
            })
            .collect();
        let done = run.done.load(Relaxed);
        self.update(id, |d| {
            if d.status != Status::Done {
                d.segs = segs;
                d.bytes_done = done;
            }
        });
    }

    fn orchestrate_inner(self: &Arc<Self>, id: u64, run: &Arc<Run>) -> Fin {
        let Some(d0) = self.get(id) else {
            return Fin::Stopped;
        };
        let st = self.settings.lock().unwrap().clone();
        let agent = net::agent(&st.user_agent);

        // 1. probe (size, range support, name), trying every mirror, with back-off
        self.set_phase(id, "Connecting…");
        let max_probe_tries = st.retries.min(5);
        let mut probe = None;
        let mut last_err = String::from("Could not connect");
        'probing: for attempt in 0..=max_probe_tries {
            let mut any_retryable = false;
            for u in &d0.urls {
                if run.stopped() {
                    return Fin::Stopped;
                }
                match net::probe(&agent, u, &d0.headers) {
                    Ok(p) => {
                        probe = Some(p);
                        break 'probing;
                    }
                    Err(e) => {
                        any_retryable |= e.retryable();
                        last_err = e.msg().to_string();
                    }
                }
            }
            if !any_retryable {
                break;
            }
            if attempt < max_probe_tries && !sleep_stoppable(run, backoff(attempt)) {
                return Fin::Stopped;
            }
        }
        let Some(probe) = probe else {
            return Fin::Failed(last_err);
        };

        // 2. is what's on disk still a valid partial of what the server has?
        self.update(id, |d| {
            let changed = !d.segs.is_empty()
                && (d.total != probe.total
                    || (!d.etag.is_empty() && !probe.etag.is_empty() && d.etag != probe.etag)
                    || (d.etag.is_empty()
                        && !d.last_modified.is_empty()
                        && !probe.last_modified.is_empty()
                        && d.last_modified != probe.last_modified));
            if changed || (!probe.ranges && d.bytes_done > 0) {
                d.segs.clear();
                d.bytes_done = 0;
            }
            if d.name_auto {
                if let Some(n) = &probe.name {
                    d.name = n.clone();
                }
            }
            d.total = probe.total;
            d.ranges_ok = probe.ranges;
            d.etag = probe.etag.clone();
            d.last_modified = probe.last_modified.clone();
        });
        let Some(d) = self.get(id) else {
            return Fin::Stopped;
        };

        // 3. open the part file
        let parts_dir = Path::new(&d.dir).join(".rustget");
        let part = parts_dir.join(format!("{id}.part"));
        let disk_err = |e: std::io::Error| {
            Fin::Failed(format!(
                "Cannot write to {}: {e}. Grant all-files access in Settings or pick another folder.",
                d.dir
            ))
        };
        if let Err(e) = fs::create_dir_all(&parts_dir) {
            return disk_err(e);
        }
        let _ = File::create(parts_dir.join(".nomedia"));
        let fresh = d.segs.is_empty() || !part.exists();
        let file = match OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(fresh)
            .open(&part)
        {
            Ok(f) => f,
            Err(e) => return disk_err(e),
        };
        if let Some(t) = d.total {
            if file.metadata().map(|m| m.len()).unwrap_or(0) != t {
                if let Err(e) = file.set_len(t) {
                    return disk_err(e);
                }
            }
        }

        // 4. segment table
        let conns = if d.conns > 0 { d.conns } else { st.conns }.clamp(1, 32);
        let table: Vec<(u64, u64, u64)> = if !d.segs.is_empty() && !fresh {
            d.segs.iter().map(|s| (s.start, s.pos, s.end)).collect()
        } else {
            match (d.total, d.ranges_ok) {
                (Some(t), true) => logic::plan_segments(t, conns)
                    .into_iter()
                    .map(|(s, e)| (s, s, e))
                    .collect(),
                (Some(t), false) if t > 0 => vec![(0, 0, t)],
                (Some(_), false) => vec![],
                (None, _) => vec![(0, 0, UNKNOWN_END)],
            }
        };
        let done: u64 = table.iter().map(|(s, p, _)| p - s).sum();
        *run.segs.lock().unwrap() = table
            .iter()
            .map(|&(start, pos, end)| SegRt {
                start,
                pos,
                end,
                owned: false,
            })
            .collect();
        run.done.store(done, Relaxed);
        run.limit_bps.store(d.limit_kbps as u64 * 1024, Relaxed);
        self.update(id, |x| {
            x.bytes_done = done;
            x.phase.clear();
        });

        // 5. run the connections
        let can_split = d.ranges_ok && d.total.is_some();
        let workers = if can_split { conns as usize } else { 1 };
        let cfg = Arc::new(Cfg {
            agent,
            urls: d.urls.clone(),
            headers: d.headers.clone(),
            file,
            ranges: d.ranges_ok,
            can_split,
            retries: st.retries,
        });
        let mut handles = Vec::new();
        for w in 0..workers {
            let (me, run, cfg) = (self.clone(), run.clone(), cfg.clone());
            if let Ok(h) = thread::Builder::new()
                .name(format!("dl-{id}-{w}"))
                .spawn(move || worker(me, run, cfg, w))
            {
                handles.push(h);
            }
        }
        for h in handles {
            if h.join().is_err() {
                run.fail("Internal error in a download thread".into());
            }
        }

        if let Some(e) = run.err.lock().unwrap().clone() {
            return Fin::Failed(e);
        }
        if run.stopped() {
            return Fin::Stopped;
        }
        let complete = run.segs.lock().unwrap().iter().all(|s| s.pos >= s.end);
        if !complete {
            return Fin::Failed("Download ended before all data arrived".into());
        }

        // 6. finish: flush, move into place, verify
        self.set_phase(id, "Finishing…");
        let _ = cfg.file.sync_all();
        let size = run.done.load(Relaxed);
        let Some(d) = self.get(id) else {
            return Fin::Stopped;
        };
        let target_dir = if st.auto_sort {
            Path::new(&d.dir).join(logic::category_for(&d.name))
        } else {
            PathBuf::from(&d.dir)
        };
        if let Err(e) = fs::create_dir_all(&target_dir) {
            return Fin::Failed(format!("Cannot create {}: {e}", target_dir.display()));
        }
        let target = unique_path(&target_dir, &d.name);
        if fs::rename(&part, &target).is_err() {
            if let Err(e) = fs::copy(&part, &target) {
                return Fin::Failed(format!("Cannot save {}: {e}", target.display()));
            }
            let _ = fs::remove_file(&part);
        }

        let mut verified = None;
        let mut note = String::new();
        if let Some((algo, want)) = logic::parse_checksum(&d.checksum) {
            self.set_phase(id, "Verifying checksum…");
            match hash_file(&target, algo) {
                Ok(got) if got == want => verified = Some(true),
                Ok(got) => {
                    verified = Some(false);
                    note = format!("Checksum mismatch: expected {want}, got {got}");
                }
                Err(e) => note = format!("Could not verify checksum: {e}"),
            }
        }
        self.update(id, |x| {
            x.status = Status::Done;
            x.final_path = target.to_string_lossy().into_owned();
            x.finished = now_secs();
            x.verified = verified;
            x.error = note;
            x.total = Some(size);
            x.bytes_done = size;
            x.segs.clear();
            x.phase.clear();
        });
        Fin::Complete
    }
}

// ───────────────────────── connections ─────────────────────────

/// Take an unowned segment with work left; failing that, cut the largest busy one in half.
fn claim(run: &Run, can_split: bool) -> Option<usize> {
    let mut segs = run.segs.lock().unwrap();
    if let Some(i) = segs.iter().position(|s| !s.owned && s.pos < s.end) {
        segs[i].owned = true;
        return Some(i);
    }
    if !can_split {
        return None;
    }
    let mut best: Option<(usize, u64)> = None;
    for (i, s) in segs.iter().enumerate() {
        if s.owned && logic::split_point(s.pos, s.end).is_some() {
            let rem = s.end - s.pos;
            if best.map_or(true, |(_, r)| rem > r) {
                best = Some((i, rem));
            }
        }
    }
    let (i, _) = best?;
    let mid = logic::split_point(segs[i].pos, segs[i].end)?;
    let old_end = segs[i].end;
    segs[i].end = mid;
    segs.push(SegRt {
        start: mid,
        pos: mid,
        end: old_end,
        owned: true,
    });
    Some(segs.len() - 1)
}

fn worker(inner: Arc<Inner>, run: Arc<Run>, cfg: Arc<Cfg>, wid: usize) {
    let mut buf = vec![0u8; 128 * 1024];
    let mut fails = 0u32;
    let mut mirror = wid % cfg.urls.len().max(1);
    loop {
        if run.stopped() {
            return;
        }
        let Some(idx) = claim(&run, cfg.can_split) else {
            return;
        };
        let before = run.segs.lock().unwrap()[idx].pos;
        let res = fetch_segment(&inner, &run, &cfg, idx, &cfg.urls[mirror], &mut buf);
        let after = {
            let mut s = run.segs.lock().unwrap();
            s[idx].owned = false;
            s[idx].pos
        };
        if let Err(e) = res {
            if run.stopped() {
                return;
            }
            if after > before {
                fails = 0;
            }
            let has_mirrors = cfg.urls.len() > 1;
            if !e.retryable() && !has_mirrors {
                run.fail(e.msg().to_string());
                return;
            }
            fails += 1;
            if fails > cfg.retries {
                run.fail(format!(
                    "{} (gave up after {} retries)",
                    e.msg(),
                    cfg.retries
                ));
                return;
            }
            mirror = (mirror + 1) % cfg.urls.len();
            if !sleep_stoppable(&run, backoff(fails)) {
                return;
            }
        } else {
            fails = 0;
        }
    }
}

fn throttle(inner: &Inner, run: &Run, n: u64) {
    let now = inner.epoch.elapsed().as_nanos() as u64;
    let g = inner.global_limit.load(Relaxed);
    let l = run.limit_bps.load(Relaxed);
    let wg = if g > 0 {
        inner.pacer.lock().unwrap().reserve(now, n, g)
    } else {
        0
    };
    let wl = if l > 0 {
        run.pacer.lock().unwrap().reserve(now, n, l)
    } else {
        0
    };
    let wait = wg.max(wl);
    if wait > 0 {
        sleep_stoppable(run, Duration::from_nanos(wait));
    }
}

/// Download one segment until it is complete, taken over by another connection, or stopped.
fn fetch_segment(
    inner: &Inner,
    run: &Run,
    cfg: &Cfg,
    idx: usize,
    url: &str,
    buf: &mut [u8],
) -> Result<(), NetErr> {
    let (pos0, end0) = {
        let s = run.segs.lock().unwrap();
        (s[idx].pos, s[idx].end)
    };
    if pos0 >= end0 {
        return Ok(());
    }
    let mut req = cfg.agent.get(url).set("Accept-Encoding", "identity");
    for (k, v) in &cfg.headers {
        req = req.set(k, v);
    }
    if cfg.ranges {
        let last = if end0 == UNKNOWN_END {
            String::new()
        } else {
            (end0 - 1).to_string()
        };
        req = req.set("Range", &format!("bytes={pos0}-{last}"));
    }
    let resp = match req.call() {
        Ok(r) => r,
        Err(ureq::Error::Status(c, _)) => return Err(net::status_error(c)),
        Err(e) => return Err(NetErr::Retry(e.to_string())),
    };
    if cfg.ranges {
        if resp.status() != 206 {
            return Err(NetErr::Retry("Server ignored the range request".into()));
        }
        if let Some((s, _, _)) = resp
            .header("content-range")
            .and_then(logic::parse_content_range)
        {
            if s != pos0 {
                return Err(NetErr::Retry("Server returned the wrong range".into()));
            }
        }
    }
    let mut reader = resp.into_reader();

    loop {
        if run.stopped() {
            return Ok(());
        }
        let (pos, end) = {
            let s = run.segs.lock().unwrap();
            (s[idx].pos, s[idx].end)
        };
        if pos >= end {
            return Ok(());
        }
        let limited = run.limit_bps.load(Relaxed) > 0 || inner.global_limit.load(Relaxed) > 0;
        let cap = if limited { 16 * 1024 } else { buf.len() };
        let want = (end - pos).min(cap as u64) as usize;
        let n = match reader.read(&mut buf[..want]) {
            Ok(n) => n,
            Err(e) => return Err(NetErr::Retry(e.to_string())),
        };
        if n == 0 {
            if end == UNKNOWN_END {
                // size was unknown: EOF is the end of the file
                let mut s = run.segs.lock().unwrap();
                s[idx].end = s[idx].pos;
                return Ok(());
            }
            return Err(NetErr::Retry("Connection closed early".into()));
        }
        throttle(inner, run, n as u64);

        // another connection may have taken the back half of this segment while we were reading
        let allowed = {
            let s = run.segs.lock().unwrap();
            s[idx].end.saturating_sub(pos).min(n as u64) as usize
        };
        if allowed == 0 {
            return Ok(());
        }
        if let Err(e) = cfg.file.write_all_at(&buf[..allowed], pos) {
            run.fail(format!("Write failed: {e}"));
            return Ok(());
        }
        let mut s = run.segs.lock().unwrap();
        let credit = (allowed as u64).min(s[idx].end.saturating_sub(s[idx].pos));
        s[idx].pos += credit;
        run.done.fetch_add(credit, Relaxed);
    }
}
