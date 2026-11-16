//! Touch-first egui interface: Downloads / Add / Grabber / Settings.

use crate::engine::{now_secs, Manager, NewDownload, Settings, Snapshot, Status, View};
use crate::{logic, net, platform};
use eframe::egui::{self, Color32, RichText};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const ACCENT: Color32 = Color32::from_rgb(232, 89, 12);
const CARD: Color32 = Color32::from_rgb(30, 32, 38);
const GOOD: Color32 = Color32::from_rgb(70, 190, 120);
const BAD: Color32 = Color32::from_rgb(235, 100, 100);
const MUTED: Color32 = Color32::from_gray(150);

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Downloads,
    Add,
    Grab,
    Settings,
}

#[derive(PartialEq, Clone, Copy)]
enum Filter {
    All,
    Active,
    Done,
    Failed,
}

#[derive(Default)]
struct AddForm {
    urls: String,
    name: String,
    referer: String,
    cookie: String,
    user: String,
    pass: String,
    headers: String,
    checksum: String,
    conns: u32,
    limit: u32,
    delay_min: u32,
}

#[derive(Default)]
struct Grab {
    url: String,
    status: String,
    busy: bool,
    links: Vec<(String, bool)>,
    filter: String,
}

pub struct App {
    mgr: Manager,
    tab: Tab,
    filter: Filter,
    add: AddForm,
    grab: Arc<Mutex<Grab>>,
    settings: Settings,
    snap: Snapshot,
    toast: Option<(String, Instant)>,
    clip_seen: String,
    clip_banner: Option<String>,
    last_clip: Instant,
    last_net: Instant,
    expanded: HashSet<u64>,
    confirm_remove: Option<u64>,
    remove_file: bool,
    kbd_shown: bool,
    app_dir: String,
    public_dir: String,
}

/// Every http(s) token inside arbitrary text (shared messages often wrap the link in prose).
fn urls_in(text: &str) -> Vec<String> {
    text.split_whitespace()
        .filter(|t| {
            let l = t.to_ascii_lowercase();
            l.starts_with("http://") || l.starts_with("https://")
        })
        .map(|t| {
            t.trim_matches(|c: char| matches!(c, '"' | '\'' | '<' | '>' | ',' | ')' | '('))
                .to_string()
        })
        .collect()
}

fn field(
    ui: &mut egui::Ui,
    text: &mut String,
    hint: &str,
    rows: usize,
    kbd: &mut bool,
) -> egui::Response {
    let r = if rows <= 1 {
        ui.add(
            egui::TextEdit::singleline(text)
                .hint_text(hint)
                .desired_width(f32::INFINITY),
        )
    } else {
        ui.add(
            egui::TextEdit::multiline(text)
                .hint_text(hint)
                .desired_rows(rows)
                .desired_width(f32::INFINITY),
        )
    };
    if r.gained_focus() && !*kbd {
        platform::show_keyboard();
        *kbd = true;
    }
    r
}

fn button(ui: &mut egui::Ui, text: &str) -> bool {
    ui.add(egui::Button::new(text).min_size(egui::vec2(84.0, 34.0)))
        .clicked()
}

fn big_button(ui: &mut egui::Ui, text: &str) -> bool {
    ui.add_sized(
        [ui.available_width(), 46.0],
        egui::Button::new(RichText::new(text).strong().size(16.0)).fill(ACCENT),
    )
    .clicked()
}

fn seg_map(ui: &mut egui::Ui, total: u64, segs: &[(u64, u64, u64)]) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 12.0), egui::Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 2.0, Color32::from_gray(55));
    if total == 0 {
        return;
    }
    for &(start, pos, _) in segs {
        let x0 = rect.left() + rect.width() * (start as f32 / total as f32);
        let x1 = rect.left() + rect.width() * (pos as f32 / total as f32);
        if x1 > x0 {
            p.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(x0, rect.top()),
                    egui::pos2(x1.max(x0 + 1.0), rect.bottom()),
                ),
                0.0,
                ACCENT,
            );
        }
    }
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        data_dir: PathBuf,
        default_dir: String,
        app_dir: String,
    ) -> Self {
        let mut v = egui::Visuals::dark();
        v.selection.bg_fill = ACCENT;
        v.hyperlink_color = ACCENT;
        v.panel_fill = Color32::from_rgb(18, 19, 23);
        cc.egui_ctx.set_visuals(v);
        let mut style = (*cc.egui_ctx.style()).clone();
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(12.0, 8.0);
        style.spacing.interact_size.y = 34.0;
        cc.egui_ctx.set_style(style);
        cc.egui_ctx.set_zoom_factor(1.1);

        let public_dir = "/storage/emulated/0/Download/RustGet".to_string();
        let mgr = Manager::new(data_dir, default_dir);
        let settings = mgr.settings();
        let mut app = App {
            mgr,
            tab: Tab::Downloads,
            filter: Filter::All,
            add: AddForm::default(),
            grab: Arc::new(Mutex::new(Grab::default())),
            settings,
            snap: Snapshot::default(),
            toast: None,
            clip_seen: String::new(),
            clip_banner: None,
            last_clip: Instant::now() - Duration::from_secs(10),
            last_net: Instant::now() - Duration::from_secs(10),
            expanded: HashSet::new(),
            confirm_remove: None,
            remove_file: false,
            kbd_shown: false,
            app_dir,
            public_dir,
        };
        // opened from a browser link or the Share sheet
        if let Some(text) = platform::launch_url() {
            let urls = urls_in(&text);
            if !urls.is_empty() {
                app.add.urls = urls.join("\n");
                app.tab = Tab::Add;
            }
        }
        app
    }

    fn toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now()));
    }

    // ───────────── background housekeeping ─────────────

    fn housekeeping(&mut self, ctx: &egui::Context) {
        if ctx.memory(|m| m.focused()).is_none() {
            self.kbd_shown = false;
        }
        if self.last_net.elapsed() > Duration::from_secs(5) {
            self.last_net = Instant::now();
            self.mgr
                .set_metered(self.settings.pause_on_metered && platform::is_metered());
        }
        if self.settings.clipboard_watch
            && ctx.input(|i| i.focused)
            && self.last_clip.elapsed() > Duration::from_millis(1500)
        {
            self.last_clip = Instant::now();
            if let Some(t) = platform::clipboard_text() {
                if t != self.clip_seen {
                    self.clip_seen = t.clone();
                    if let Some(u) = urls_in(&t).into_iter().next() {
                        let known = self.snap.items.iter().any(|i| i.url == u);
                        self.clip_banner = if known { None } else { Some(u) };
                    }
                }
            }
        }
        platform::set_wake_lock(self.snap.active > 0);
        if let Some((_, t)) = &self.toast {
            if t.elapsed() > Duration::from_secs(3) {
                self.toast = None;
            }
        }
    }

    // ───────────── chrome ─────────────

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        if cfg!(target_os = "android") {
            ui.add_space(28.0); // status bar
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new("RustGet").heading().color(ACCENT));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let s = &self.snap;
                let txt = if s.active > 0 {
                    format!(
                        "{}  ·  {} active",
                        logic::format_speed(s.total_speed),
                        s.active
                    )
                } else if s.queued > 0 {
                    format!("{} queued", s.queued)
                } else {
                    "Idle".to_string()
                };
                ui.label(RichText::new(txt).color(MUTED));
            });
        });
        ui.add_space(2.0);
    }

    fn nav_bar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        let tabs = [
            (Tab::Downloads, "Downloads"),
            (Tab::Add, "Add"),
            (Tab::Grab, "Grabber"),
            (Tab::Settings, "Settings"),
        ];
        ui.columns(4, |cols| {
            for (i, (t, label)) in tabs.iter().enumerate() {
                let sel = self.tab == *t;
                let b = egui::Button::new(RichText::new(*label).size(14.0).color(if sel {
                    Color32::WHITE
                } else {
                    MUTED
                }))
                .fill(if sel { ACCENT } else { Color32::TRANSPARENT });
                let w = cols[i].available_width();
                if cols[i].add_sized([w, 40.0], b).clicked() {
                    self.tab = *t;
                }
            }
        });
        if cfg!(target_os = "android") {
            ui.add_space(14.0); // gesture bar
        }
    }

    // ───────────── downloads tab ─────────────

    fn downloads_tab(&mut self, ui: &mut egui::Ui) {
        if let Some(url) = self.clip_banner.clone() {
            egui::Frame::none()
                .fill(Color32::from_rgb(40, 44, 56))
                .rounding(8.0)
                .inner_margin(egui::Margin::same(10.0))
                .show(ui, |ui| {
                    ui.label(RichText::new("Link on your clipboard").strong());
                    ui.label(RichText::new(&url).small().color(MUTED));
                    ui.horizontal(|ui| {
                        if button(ui, "Download") {
                            let n = NewDownload {
                                urls: vec![url.clone()],
                                ..Default::default()
                            };
                            match self.mgr.add(n) {
                                Ok(_) => self.toast("Added to queue"),
                                Err(e) => self.toast(e),
                            }
                            self.clip_banner = None;
                        }
                        if button(ui, "Dismiss") {
                            self.clip_banner = None;
                        }
                    });
                });
        }

        let s = &self.snap;
        let counts = [
            (Filter::All, format!("All {}", s.items.len())),
            (Filter::Active, format!("Active {}", s.active + s.queued)),
            (Filter::Done, format!("Done {}", s.done)),
            (Filter::Failed, format!("Failed {}", s.failed)),
        ];
        ui.horizontal_wrapped(|ui| {
            for (f, label) in counts {
                ui.selectable_value(&mut self.filter, f, label);
            }
        });
        ui.horizontal_wrapped(|ui| {
            if button(ui, "Pause all") {
                self.mgr.pause_all();
            }
            if button(ui, "Resume all") {
                self.mgr.resume_all();
            }
            if button(ui, "Clear done") {
                self.mgr.clear_finished();
            }
        });
        ui.add_space(2.0);

        let filter = self.filter;
        let items: Vec<View> = self
            .snap
            .items
            .iter()
            .filter(|v| match filter {
                Filter::All => true,
                Filter::Active => {
                    matches!(v.status, Status::Active | Status::Queued | Status::Paused)
                }
                Filter::Done => v.status == Status::Done,
                Filter::Failed => v.status == Status::Failed,
            })
            .rev() // newest first
            .cloned()
            .collect();

        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                if items.is_empty() {
                    ui.add_space(40.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("Nothing here yet").size(18.0).color(MUTED));
                        ui.label(RichText::new("Copy a link, or open the Add tab").color(MUTED));
                    });
                }
                for v in &items {
                    self.card(ui, v);
                }
                ui.add_space(8.0);
            });
    }

    fn card(&mut self, ui: &mut egui::Ui, v: &View) {
        egui::Frame::none()
            .fill(CARD)
            .rounding(10.0)
            .inner_margin(egui::Margin::same(12.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(&v.name).strong().size(16.0));

                let size = |n: u64| logic::format_bytes(n);
                let now = now_secs();
                let (line, color) = match v.status {
                    Status::Active => {
                        let text = if !v.phase.is_empty() {
                            v.phase.clone()
                        } else {
                            let mut t = match v.total {
                                Some(tot) => format!("{} / {}", size(v.done), size(tot)),
                                None => size(v.done),
                            };
                            t += &format!("  ·  {}", logic::format_speed(v.speed));
                            if let Some(e) = v.eta {
                                t += &format!("  ·  {} left", logic::format_eta(e));
                            }
                            if v.conns > 1 {
                                t += &format!("  ·  {} connections", v.conns);
                            }
                            t
                        };
                        (text, MUTED)
                    }
                    Status::Queued if v.start_at > now => (
                        format!("Scheduled: starts in {}", logic::format_eta(v.start_at - now)),
                        MUTED,
                    ),
                    Status::Queued => (if v.phase.is_empty() { "Queued".into() } else { v.phase.clone() }, MUTED),
                    Status::Paused => (
                        match v.total {
                            Some(t) => format!("Paused  ·  {} / {}", size(v.done), size(t)),
                            None => format!("Paused  ·  {}", size(v.done)),
                        },
                        MUTED,
                    ),
                    Status::Failed => (v.error.clone(), BAD),
                    Status::Done => {
                        let mut t = size(v.total.unwrap_or(v.done));
                        match v.verified {
                            Some(true) => t += "  ·  checksum OK",
                            Some(false) => t += "  ·  CHECKSUM MISMATCH",
                            None => {}
                        }
                        (t, if v.verified == Some(false) { BAD } else { GOOD })
                    }
                };
                ui.label(RichText::new(line).color(color).size(13.0));

                if v.status != Status::Done {
                    if let Some(t) = v.total.filter(|t| *t > 0) {
                        let frac = (v.done as f32 / t as f32).clamp(0.0, 1.0);
                        let col = if v.status == Status::Failed { BAD } else { ACCENT };
                        ui.add(egui::ProgressBar::new(frac).desired_height(10.0).fill(col));
                    }
                }
                if v.status == Status::Done && !v.error.is_empty() {
                    ui.label(RichText::new(&v.error).small().color(BAD));
                }

                ui.horizontal_wrapped(|ui| {
                    match v.status {
                        Status::Active | Status::Queued => {
                            if button(ui, "Pause") {
                                self.mgr.pause(v.id);
                            }
                            if v.status == Status::Queued && button(ui, "Start now") {
                                self.mgr.resume(v.id);
                            }
                        }
                        Status::Paused => {
                            if button(ui, "Resume") {
                                self.mgr.resume(v.id);
                            }
                        }
                        Status::Failed => {
                            if button(ui, "Retry") {
                                self.mgr.resume(v.id);
                            }
                        }
                        Status::Done => {}
                    }
                    let open = self.expanded.contains(&v.id);
                    if button(ui, if open { "Hide" } else { "Details" }) {
                        if open {
                            self.expanded.remove(&v.id);
                        } else {
                            self.expanded.insert(v.id);
                        }
                    }
                    if button(ui, "Remove") {
                        self.confirm_remove = Some(v.id);
                        self.remove_file = false;
                    }
                });

                if self.expanded.contains(&v.id) {
                    ui.separator();
                    ui.label(RichText::new(&v.url).monospace().size(11.0).color(MUTED));
                    if v.mirrors > 0 {
                        ui.label(format!("{} mirror(s) used automatically if this one fails", v.mirrors));
                    }
                    if !v.final_path.is_empty() {
                        ui.label(format!("Saved to {}", v.final_path));
                    } else {
                        ui.label(format!("Saving to {}", v.dir));
                    }
                    if v.status != Status::Done {
                        if v.total.is_some() && !v.ranges {
                            ui.label(
                                RichText::new("This server can't resume or split downloads: single connection.")
                                    .color(MUTED),
                            );
                        }
                        if let (Some(t), false) = (v.total, v.segs.is_empty()) {
                            ui.label(RichText::new(format!("{} segments", v.segs.len())).small().color(MUTED));
                            seg_map(ui, t, &v.segs);
                        }
                        ui.horizontal(|ui| {
                            ui.label("Speed limit KB/s (0 = none)");
                            let mut lim = v.limit_kbps;
                            if ui.add(egui::DragValue::new(&mut lim).range(0..=1_000_000).speed(10)).changed() {
                                self.mgr.set_limit(v.id, lim);
                            }
                        });
                    }
                }
            });
        ui.add_space(8.0);
    }

    // ───────────── add tab ─────────────

    fn add_tab(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                ui.label(RichText::new("Add downloads").heading());
                ui.label(
                    RichText::new(
                        "One link per line.  a | b  adds b as a mirror of a.  \
                     img[001-020].jpg  expands to a numbered batch.",
                    )
                    .small()
                    .color(MUTED),
                );
                field(
                    ui,
                    &mut self.add.urls,
                    "https://example.com/file.zip",
                    6,
                    &mut self.kbd_shown,
                );
                ui.horizontal_wrapped(|ui| {
                    if button(ui, "Paste") {
                        match platform::clipboard_text() {
                            Some(t) if !t.trim().is_empty() => {
                                if !self.add.urls.is_empty() && !self.add.urls.ends_with('\n') {
                                    self.add.urls.push('\n');
                                }
                                self.add.urls.push_str(t.trim());
                            }
                            _ => self.toast("Clipboard is empty"),
                        }
                    }
                    if button(ui, "Keyboard") {
                        platform::show_keyboard();
                        self.kbd_shown = true;
                    }
                    if button(ui, "Clear") {
                        self.add.urls.clear();
                    }
                });

                egui::CollapsingHeader::new("Advanced").show(ui, |ui| {
                    let k = &mut self.kbd_shown;
                    ui.label("Save as (single link only)");
                    field(ui, &mut self.add.name, "auto from server", 1, k);
                    ui.label("Referer");
                    field(
                        ui,
                        &mut self.add.referer,
                        "https://site-that-linked-it",
                        1,
                        k,
                    );
                    ui.label("Cookie");
                    field(ui, &mut self.add.cookie, "name=value; other=value", 1, k);
                    ui.label("Username / password (HTTP Basic)");
                    field(ui, &mut self.add.user, "username", 1, k);
                    field(ui, &mut self.add.pass, "password", 1, k);
                    ui.label("Extra headers (Name: value, one per line)");
                    field(ui, &mut self.add.headers, "", 2, k);
                    ui.label("Expected checksum (MD5 / SHA-1 / SHA-256)");
                    field(
                        ui,
                        &mut self.add.checksum,
                        "paste hash to verify after download",
                        1,
                        k,
                    );
                    ui.horizontal(|ui| {
                        ui.label("Connections (0 = default)");
                        ui.add(egui::DragValue::new(&mut self.add.conns).range(0..=32));
                    });
                    ui.horizontal(|ui| {
                        ui.label("Speed limit KB/s (0 = none)");
                        ui.add(
                            egui::DragValue::new(&mut self.add.limit)
                                .range(0..=1_000_000)
                                .speed(10),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label("Start in (minutes, 0 = now)");
                        ui.add(egui::DragValue::new(&mut self.add.delay_min).range(0..=10_080));
                    });
                });

                ui.add_space(6.0);
                if big_button(ui, "Add to queue") {
                    self.submit_add();
                }
                ui.add_space(20.0);
            });
    }

    fn submit_add(&mut self) {
        let mut headers: Vec<(String, String)> = Vec::new();
        let a = &self.add;
        if !a.referer.trim().is_empty() {
            headers.push(("Referer".into(), a.referer.trim().into()));
        }
        if !a.cookie.trim().is_empty() {
            headers.push(("Cookie".into(), a.cookie.trim().into()));
        }
        if !a.user.trim().is_empty() {
            let cred = format!("{}:{}", a.user.trim(), a.pass);
            headers.push((
                "Authorization".into(),
                format!("Basic {}", logic::base64(cred.as_bytes())),
            ));
        }
        for line in a.headers.lines() {
            if let Some((k, v)) = line.split_once(':') {
                if !k.trim().is_empty() {
                    headers.push((k.trim().into(), v.trim().into()));
                }
            }
        }

        let mut jobs: Vec<Vec<String>> = Vec::new();
        for line in a.urls.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let parts: Vec<String> = line
                .split('|')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .map(|s| {
                    if s.to_ascii_lowercase().starts_with("http") {
                        s
                    } else {
                        urls_in(&s).into_iter().next().unwrap_or(s)
                    }
                })
                .collect();
            if parts.len() == 1 {
                jobs.extend(
                    logic::expand_sequence(&parts[0])
                        .into_iter()
                        .map(|u| vec![u]),
                );
            } else if !parts.is_empty() {
                jobs.push(parts);
            }
        }
        if jobs.is_empty() {
            self.toast("Paste or type a link first");
            return;
        }
        let single = jobs.len() == 1;
        let (mut added, mut first_err) = (0, None);
        for urls in jobs {
            let n = NewDownload {
                urls,
                name: if single {
                    a.name.clone()
                } else {
                    String::new()
                },
                headers: headers.clone(),
                checksum: if single {
                    a.checksum.clone()
                } else {
                    String::new()
                },
                conns: a.conns,
                limit_kbps: a.limit,
                start_in_secs: a.delay_min as u64 * 60,
            };
            match self.mgr.add(n) {
                Ok(_) => added += 1,
                Err(e) => {
                    first_err.get_or_insert(e);
                }
            }
        }
        let msg = match (added, first_err) {
            (0, Some(e)) => e,
            (n, Some(e)) => format!("Added {n}. Skipped: {e}"),
            (n, None) => format!("Added {n} download{}", if n == 1 { "" } else { "s" }),
        };
        if added > 0 {
            self.add = AddForm::default();
            self.tab = Tab::Downloads;
        }
        self.toast(msg);
    }

    // ───────────── link grabber tab ─────────────

    fn grab_tab(&mut self, ui: &mut egui::Ui) {
        let grab = self.grab.clone();
        let mut g = grab.lock().unwrap();
        ui.label(RichText::new("Link grabber").heading());
        ui.label(
            RichText::new("Scan a web page for downloadable links, tick the ones you want, add them all at once.")
                .small()
                .color(MUTED),
        );
        field(
            ui,
            &mut g.url,
            "https://example.com/downloads",
            1,
            &mut self.kbd_shown,
        );
        ui.horizontal(|ui| {
            if button(ui, "Paste") {
                if let Some(t) = platform::clipboard_text() {
                    g.url = urls_in(&t)
                        .into_iter()
                        .next()
                        .unwrap_or_else(|| t.trim().to_string());
                }
            }
            let go = ui.add_enabled(
                !g.busy,
                egui::Button::new("Find links").min_size(egui::vec2(110.0, 34.0)),
            );
            if go.clicked() && !g.busy {
                g.busy = true;
                g.status = "Fetching page…".into();
                g.links.clear();
                let (page, ua, shared) = (
                    g.url.trim().to_string(),
                    self.settings.user_agent.clone(),
                    self.grab.clone(),
                );
                std::thread::spawn(move || {
                    let agent = net::agent(&ua);
                    let result = net::fetch_page(&agent, &page);
                    let mut g = shared.lock().unwrap();
                    match result {
                        Ok((final_url, html)) => {
                            let mut out: Vec<String> = Vec::new();
                            for raw in logic::extract_links(&html) {
                                if let Some(u) = net::resolve(&final_url, &raw) {
                                    if !out.contains(&u) {
                                        out.push(u);
                                    }
                                }
                            }
                            let n = out.len();
                            g.links = out.into_iter().map(|u| (u, false)).collect();
                            g.status = format!("{n} links found");
                        }
                        Err(e) => g.status = format!("Failed: {e}"),
                    }
                    g.busy = false;
                });
            }
        });
        if !g.status.is_empty() {
            ui.label(RichText::new(g.status.clone()).color(MUTED));
        }
        if g.links.is_empty() {
            return;
        }

        field(
            ui,
            &mut g.filter,
            "filter, e.g.  .zip .pdf .mp4",
            1,
            &mut self.kbd_shown,
        );
        let terms: Vec<String> = g
            .filter
            .split_whitespace()
            .map(|t| t.to_ascii_lowercase())
            .collect();
        let shown =
            |u: &str| terms.is_empty() || terms.iter().any(|t| u.to_ascii_lowercase().contains(t));
        ui.horizontal_wrapped(|ui| {
            if button(ui, "Tick shown") {
                for (u, on) in g.links.iter_mut() {
                    if shown(u) {
                        *on = true;
                    }
                }
            }
            if button(ui, "Untick all") {
                for (_, on) in g.links.iter_mut() {
                    *on = false;
                }
            }
        });
        let ticked = g.links.iter().filter(|(_, on)| *on).count();
        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .max_height((ui.available_height() - 64.0).max(120.0))
            .show(ui, |ui| {
                for (u, on) in g.links.iter_mut() {
                    if shown(u) {
                        let name = logic::filename_from_url(u);
                        ui.checkbox(on, name).on_hover_text(u.as_str());
                        ui.label(RichText::new(u.as_str()).small().color(MUTED));
                    }
                }
            });
        if ticked > 0 && big_button(ui, &format!("Add {ticked} selected")) {
            let (mut added, mut skipped) = (0, 0);
            for (u, on) in g.links.iter_mut().filter(|(_, on)| *on) {
                let n = NewDownload {
                    urls: vec![u.clone()],
                    ..Default::default()
                };
                match self.mgr.add(n) {
                    Ok(_) => added += 1,
                    Err(_) => skipped += 1,
                }
                *on = false;
            }
            drop(g);
            self.toast(format!("Added {added}, skipped {skipped}"));
            if added > 0 {
                self.tab = Tab::Downloads;
            }
        }
    }

    // ───────────── settings tab ─────────────

    fn settings_tab(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
            ui.label(RichText::new("Settings").heading());

            ui.label(RichText::new("Save new downloads to").strong());
            changed |= field(ui, &mut self.settings.dir, "folder", 1, &mut self.kbd_shown).changed();
            let has_access = platform::has_all_files_access();
            ui.horizontal_wrapped(|ui| {
                if button(ui, "Public Downloads") {
                    if has_access {
                        self.settings.dir = self.public_dir.clone();
                        changed = true;
                    } else {
                        platform::request_all_files_access();
                        self.toast("Allow \"All files access\", then come back and tap again");
                    }
                }
                if button(ui, "App folder") {
                    self.settings.dir = self.app_dir.clone();
                    changed = true;
                }
            });
            if cfg!(target_os = "android") {
                ui.label(
                    RichText::new(if has_access {
                        "All-files access: granted (Android 11+)"
                    } else {
                        "All-files access: not granted. Needed to save into the public Downloads folder (Android 11+)."
                    })
                    .small()
                    .color(MUTED),
                );
                if !has_access && button(ui, "Grant access") {
                    platform::request_all_files_access();
                }
            }

            ui.separator();
            ui.horizontal(|ui| {
                ui.label("Downloads at once");
                changed |= ui.add(egui::Slider::new(&mut self.settings.max_active, 1..=10)).changed();
            });
            ui.horizontal(|ui| {
                ui.label("Connections per file");
                changed |= ui.add(egui::Slider::new(&mut self.settings.conns, 1..=32)).changed();
            });
            ui.horizontal(|ui| {
                ui.label("Retries before giving up");
                changed |= ui.add(egui::Slider::new(&mut self.settings.retries, 0..=30)).changed();
            });
            ui.horizontal(|ui| {
                ui.label("Total speed limit KB/s (0 = none)");
                changed |= ui
                    .add(egui::DragValue::new(&mut self.settings.global_limit_kbps).range(0..=10_000_000).speed(25))
                    .changed();
            });

            ui.separator();
            changed |= ui
                .checkbox(&mut self.settings.pause_on_metered, "Wait for Wi-Fi (pause on mobile data)")
                .changed();
            changed |= ui
                .checkbox(&mut self.settings.clipboard_watch, "Offer to download links I copy")
                .changed();
            changed |= ui
                .checkbox(&mut self.settings.auto_sort, "Sort finished files into Video / Music / Documents / …")
                .changed();

            ui.separator();
            ui.label("User-Agent");
            changed |= field(ui, &mut self.settings.user_agent, "", 2, &mut self.kbd_shown).changed();
            if button(ui, "Retry all failed") {
                self.mgr.resume_all();
            }
            ui.add_space(20.0);
        });
        if changed {
            self.mgr.set_settings(self.settings.clone());
            self.settings = self.mgr.settings();
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.snap = self.mgr.snapshot();
        self.housekeeping(ctx);

        egui::TopBottomPanel::top("top").show(ctx, |ui| self.top_bar(ui));
        egui::TopBottomPanel::bottom("nav").show(ctx, |ui| self.nav_bar(ui));
        egui::CentralPanel::default().show(ctx, |ui| match self.tab {
            Tab::Downloads => self.downloads_tab(ui),
            Tab::Add => self.add_tab(ui),
            Tab::Grab => self.grab_tab(ui),
            Tab::Settings => self.settings_tab(ui),
        });

        if let Some(id) = self.confirm_remove {
            egui::Window::new("Remove download?")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    ui.checkbox(&mut self.remove_file, "Also delete the file from storage");
                    ui.horizontal(|ui| {
                        if button(ui, "Remove") {
                            self.mgr.remove(id, self.remove_file);
                            self.expanded.remove(&id);
                            self.confirm_remove = None;
                        }
                        if button(ui, "Cancel") {
                            self.confirm_remove = None;
                        }
                    });
                });
        }

        if let Some((msg, _)) = &self.toast {
            egui::Area::new(egui::Id::new("toast"))
                .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -90.0))
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.label(msg.as_str());
                    });
                });
        }

        let ms = if self.snap.active > 0 { 250 } else { 700 };
        ctx.request_repaint_after(Duration::from_millis(ms));
    }
}
