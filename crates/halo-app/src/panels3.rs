//! Phase 3 hub panels: Resources (M3, Pulse-inspired).

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use halo_modules::resources::SortBy;
use halo_modules::system::format_bytes;
use halo_ui::hsla;
use halo_ui::tokens::{self, Palette};

use crate::hub::Hub;
use crate::ui;

pub struct Phase3State {
    /// Window in samples (2 s each); 0 = session.
    pub res_window: usize,
    pub res_sort: SortBy,
}

impl Default for Phase3State {
    fn default() -> Self {
        Self { res_window: 150, res_sort: SortBy::Cpu }
    }
}

fn t(hub: &Hub, key: &str) -> String {
    hub.shared.borrow().t(key)
}

fn row() -> Div {
    div().flex().flex_row().flex_wrap().items_center().gap(px(tokens::SPACE_2))
}

pub fn page(p: &Palette, family: &str, title: String, intro: Option<String>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(tokens::SPACE_2)) // Fluent: cards stacked closely
        .w_full()
        .max_w(px(1000.0))
        .child(div().flex().flex_col().gap(px(tokens::SPACE_1)).pb(px(tokens::SPACE_4)).child(ui::h1(p, family, title)).children(intro.map(|i| ui::label(p, i))))
}

fn stat_card(p: &Palette, display: &str, mono: &str, icon: &str, title: &str, value: String, foot: String, frac: f32) -> Div {
    ui::card(p)
        .flex_1()
        .min_w(px(190.0))
        .flex_row()
        .items_center()
        .child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .child(ui::icon(icon, 14.0, p.ink_2))
                        .child(ui::mono(p, mono, title.to_uppercase())),
                )
                .child(div().font_family(SharedString::from(display.to_string())).text_size(px(30.0)).font_weight(FontWeight::SEMIBOLD).child(value))
                .child(ui::mono(p, mono, foot)),
        )
        .child(ui::ring(56.0, 5.0, frac, p.surface_3, p.accent))
}

pub fn resources(hub: &mut Hub, p: &Palette, cx: &mut Context<Hub>) -> AnyElement {
    let (display, mono, snap) = {
        let s = hub.shared.borrow();
        (s.typo.display.clone(), s.typo.mono.clone(), s.handles.system.get())
    };
    let res = hub.shared.borrow().handles.resources.clone();
    let win = hub.p3.res_window;
    let sort = hub.p3.res_sort;
    let (points, cores, cpu_name, top, peaks) = res.with(|h| {
        (h.window(win), h.cores.clone(), h.cpu_name.clone(), h.top(win, sort, 15), h.peaks.iter().rev().take(5).cloned().collect::<Vec<_>>())
    });
    let last = points.last().copied().unwrap_or_default();

    let overview = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .gap(px(tokens::SPACE_4))
        .child(stat_card(p, &display, &mono, "cpu", "CPU", format!("{:.0}%", last.cpu), hub.shared.borrow().tf("res.cores", &[("n", &cores.len().to_string())]), last.cpu / 100.0))
        .child(stat_card(
            p,
            &display,
            &mono,
            "memory-stick",
            &t(hub, "indicator.ram"),
            format!("{:.0}%", last.ram),
            format!("{} / {}", format_bytes(snap.ram_used), format_bytes(snap.ram_total)),
            last.ram / 100.0,
        ))
        .child(stat_card(p, &display, &mono, "monitor", "GPU", format!("{:.0}%", last.gpu), "GPU Engine".into(), last.gpu / 100.0));

    let windows = [(30usize, "1 min"), (150, "5 min"), (1800, "1 h"), (0, "Session")].map(|(n, label)| {
        let b = Button::new(ElementId::Name(format!("res-win-{n}").into())).label(label).on_click(cx.listener(move |hub, _, _, cx| {
            hub.p3.res_window = n;
            cx.notify();
        }));
        if n == win { b.primary() } else { b.outline() }
    });

    let history = ui::card(p)
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(tokens::SPACE_3))
                .child(ui::icon("cpu", 18.0, p.accent_text))
                .child(div().flex().flex_col().child(ui::h2(p, "CPU")).child(ui::mono(p, &mono, cpu_name)))
                .child(div().flex_1())
                .child(row().children(windows)),
        )
        .child(ui::chart(points.iter().map(|x| x.cpu).collect(), 140.0, p.accent, p.line_2))
        .child(div().flex().flex_row().gap(px(tokens::SPACE_4)).child(ui::mono(p, &mono, "GPU")))
        .child(ui::chart(points.iter().map(|x| x.gpu).collect(), 70.0, p.info_text, p.line_2))
        .child(
            div().flex().flex_row().flex_wrap().gap(px(tokens::SPACE_3)).children(cores.iter().enumerate().map(|(i, c)| {
                div()
                    .w(px(120.0))
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .child(ui::mono(p, &mono, hub.shared.borrow().tf("res.core", &[("i", &i.to_string()), ("v", &format!("{c:.0}"))])))
                    .child(ui::meter(p, c / 100.0))
            })),
        );

    let sorts = [(SortBy::Cpu, "CPU"), (SortBy::Gpu, "GPU"), (SortBy::Mem, "RAM")].map(|(s, label)| {
        let b = Button::new(ElementId::Name(format!("res-sort-{label}").into())).label(label).on_click(cx.listener(move |hub, _, _, cx| {
            hub.p3.res_sort = s;
            cx.notify();
        }));
        if s == sort { b.primary() } else { b.outline() }
    });
    let header = div()
        .flex()
        .flex_row()
        .gap(px(tokens::SPACE_3))
        .child(div().flex_1().child(ui::mono(p, &mono, t(hub, "res.process"))))
        .child(div().w(px(140.0)).child(ui::mono(p, &mono, t(hub, "res.history"))))
        .child(div().w(px(80.0)).child(ui::mono(p, &mono, t(hub, "res.cpu_avg_max"))))
        .child(div().w(px(60.0)).child(ui::mono(p, &mono, "GPU")))
        .child(div().w(px(80.0)).child(ui::mono(p, &mono, "RAM")));
    let rows = top.iter().map(|s| {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(tokens::SPACE_3))
            .py(px(4.0))
            .child(div().flex_1().text_size(px(13.0)).truncate().child(s.name.trim_end_matches(".exe").to_string()))
            .child(div().w(px(140.0)).child(ui::chart(s.history.clone(), 22.0, p.accent, p.line)))
            .child(div().w(px(80.0)).child(ui::mono(p, &mono, format!("{:.1} / {:.0}", s.avg_cpu, s.max_cpu))))
            .child(div().w(px(60.0)).child(ui::mono(p, &mono, format!("{:.1}", s.avg_gpu))))
            .child(div().w(px(80.0)).child(ui::mono(p, &mono, format_bytes(s.mem))))
    });

    let peak_rows = peaks.iter().map(|pk| {
        let when = chrono::DateTime::from_timestamp(pk.at, 0)
            .map(|d| d.with_timezone(&chrono::Local).format("%H:%M:%S").to_string())
            .unwrap_or_default();
        div()
            .flex()
            .flex_row()
            .gap(px(tokens::SPACE_3))
            .child(ui::icon("zap", 14.0, p.warning_text))
            .child(ui::mono(p, &mono, format!("{when} · {} · {:.0} %", pk.name, pk.cpu)))
    });

    page(p, &display, t(hub, "module.resources"), Some(t(hub, "res.intro")))
        .child(overview)
        .child(history)
        .child(
            ui::card(p)
                .child(div().flex().flex_row().items_center().child(ui::h2(p, t(hub, "res.top"))).child(div().flex_1()).child(row().children(sorts)))
                .child(header)
                .children(rows),
        )
        .child(
            ui::card(p)
                .child(ui::h2(p, t(hub, "res.peaks")))
                .when(peaks.is_empty(), |d| d.child(ui::label(p, t(hub, "res.no_peaks"))))
                .children(peak_rows),
        )
        .into_any_element()
}

/// Pulse-style mini gauges for the hub sidebar footer.
pub fn sidebar_gauges(hub: &Hub, p: &Palette) -> Div {
    let mono = hub.shared.borrow().typo.mono.clone();
    let last = hub.shared.borrow().handles.resources.with(|h| h.global.back().copied()).unwrap_or_default();
    let snap = hub.shared.borrow().handles.system.get();
    let line = |label: &str, v: f32| {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .child(div().w(px(34.0)).child(ui::mono(p, &mono, label.to_string())))
            .child(div().flex_1().child(ui::meter(p, v / 100.0)))
            .child(div().w(px(34.0)).child(ui::mono(p, &mono, format!("{v:.0}%"))))
    };
    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .p(px(tokens::SPACE_3))
        .rounded(px(tokens::RADIUS_MD))
        .bg(hsla(p.surface_2))
        .child(line("CPU", snap.cpu))
        .child(line("RAM", snap.ram_percent()))
        .child(line("GPU", last.gpu))
}

// ── M15 Music ───────────────────────────────────────────────────────────────

pub fn music(hub: &mut Hub, p: &Palette, cx: &mut Context<Hub>) -> AnyElement {
    let (display, mono, _fr) = {
        let s = hub.shared.borrow();
        (s.typo.display.clone(), s.typo.mono.clone(), s.fr())
    };
    let m = hub.shared.borrow().handles.music.clone();
    let st = m.get();
    let cur = st.queue.current().cloned();
    let (m1, m2, m3, m4, m5) = (m.clone(), m.clone(), m.clone(), m.clone(), m.clone());
    let art = match &st.cover {
        Some(b) => img(std::sync::Arc::new(Image::from_bytes(crate::ui::image_format(b), b.clone()))).size(px(120.0)).rounded(px(tokens::RADIUS_LG)).into_any_element(),
        None => div().size(px(120.0)).flex_none().rounded(px(tokens::RADIUS_LG)).bg(hsla(p.surface_2)).flex().items_center().justify_center().child(ui::icon("music", 40.0, p.ink_3)).into_any_element(),
    };
    let dur = cur.as_ref().map(|t| t.duration_s).unwrap_or(0.0);
    let now_playing = ui::card(p)
        .flex_row()
        .gap(px(tokens::SPACE_6))
        .child(art)
        .child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(tokens::SPACE_2))
                .child(div().font_family(SharedString::from(display.clone())).text_size(px(22.0)).font_weight(FontWeight::SEMIBOLD).child(cur.as_ref().map(|t| t.title.clone()).unwrap_or_else(|| t(hub, "music.nothing"))))
                .child(ui::label(p, cur.as_ref().map(|t| format!("{} · {}", t.artist, t.album)).unwrap_or_default()))
                .child(div().flex().flex_row().items_center().gap(px(8.0))
                    .child(ui::mono(p, &mono, halo_modules::media::mmss(st.position_s)))
                    .child(div().flex_1().child(ui::meter(p, if dur > 0.0 { (st.position_s / dur) as f32 } else { 0.0 })))
                    .child(ui::mono(p, &mono, halo_modules::media::mmss(dur))))
                .child(row()
                    .child(Button::new("mu-prev").icon(ui::lucide("skip-back")).outline().on_click(move |_, _, _| m1.previous()))
                    .child(Button::new("mu-play").icon(ui::lucide(if st.playing { "pause" } else { "play" })).primary().on_click(move |_, _, _| m2.toggle()))
                    .child(Button::new("mu-next").icon(ui::lucide("skip-forward")).outline().on_click(move |_, _, _| m3.next()))
                    .child({
                        let on = st.queue.shuffle;
                        let b = Button::new("mu-shuffle").icon(ui::lucide("shuffle")).label(t(hub, "music.shuffle")).on_click(move |_, _, _| m4.set_shuffle(!on));
                        if on { b.primary() } else { b.outline() }
                    })
                    .child({
                        let r = st.queue.repeat;
                        let label = match r { halo_modules::music::Repeat::Off => t(hub, "music.repeat_off"), halo_modules::music::Repeat::All => t(hub, "music.repeat_all"), halo_modules::music::Repeat::One => "1".to_string() };
                        let b = Button::new("mu-repeat").icon(ui::lucide(if r == halo_modules::music::Repeat::One { "repeat-1" } else { "repeat" })).label(label).on_click(move |_, _, _| m5.set_repeat(r.next()));
                        if r == halo_modules::music::Repeat::Off { b.outline() } else { b.primary() }
                    }))
                .children(st.error.clone().map(|e| ui::mono(p, &mono, e))),
        );

    let lib = m.library("");
    let m6 = m.clone();
    let scan = Button::new("mu-scan").label(t(hub, "music.add_folder")).outline().on_click(cx.listener(move |_, _, _, cx| {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: true, prompt: None });
        let m = m6.clone();
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(dirs))) = rx.await {
                std::thread::spawn(move || m.scan(&dirs));
                let _ = this.update(cx, |_, cx| cx.notify());
            }
        })
        .detach();
    }));
    let m7 = m.clone();
    let scan_music = Button::new("mu-scan-default").label(t(hub, "music.scan_default")).outline().on_click(move |_, _, _| {
        if let Some(home) = std::env::var_os("USERPROFILE") {
            let dir = std::path::PathBuf::from(home).join("Music");
            let m = m7.clone();
            std::thread::spawn(move || m.scan(&[dir]));
        }
    });
    let registered = halo_modules::music::explorer_verb_registered();
    let explorer = Button::new("mu-explorer")
        .label(if registered { t(hub, "music.explorer_off") } else { t(hub, "music.explorer_on") })
        .outline()
        .on_click(move |_, _, cx| {
            if registered {
                halo_modules::music::unregister_explorer_verb();
            } else if let Ok(exe) = std::env::current_exe() {
                halo_modules::music::register_explorer_verb(&exe);
            }
            cx.refresh_windows();
        });
    let lib_rows = lib.iter().take(200).enumerate().map(|(i, tr)| {
        let all = lib.clone();
        let mm = m.clone();
        div()
            .id(("mu-track", i))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(tokens::SPACE_3))
            .px(px(tokens::SPACE_2))
            .py(px(6.0))
            .rounded(px(tokens::RADIUS_SM))
            .cursor_pointer()
            .hover(|s| s.bg(hsla(p.surface_2)))
            .child(div().flex_1().text_size(px(13.0)).truncate().child(tr.title.clone()))
            .child(div().w(px(200.0)).text_size(px(12.0)).text_color(hsla(p.ink_2)).truncate().child(tr.artist.clone()))
            .child(div().w(px(50.0)).child(ui::mono(p, &mono, halo_modules::media::mmss(tr.duration_s))))
            .on_click(move |_, _, _| mm.play_tracks(all.clone(), i))
    });
    let playlists = m.playlists();
    let queue_tracks = st.queue.tracks.clone();
    let m8 = m.clone();
    let save = Button::new("mu-save-pl").label(t(hub, "music.save_queue")).outline().on_click(move |_, _, cx| {
        if !queue_tracks.is_empty() {
            let name = format!("Playlist {}", chrono::Local::now().format("%d/%m %H:%M"));
            m8.save_playlist(&name, &queue_tracks);
            cx.refresh_windows();
        }
    });
    let pl_rows = playlists.into_iter().map(|(id, name, n)| {
        let (a, b) = (m.clone(), m.clone());
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(tokens::SPACE_3))
            .child(ui::icon("list-music", 16.0, p.ink_2))
            .child(div().flex_1().text_size(px(13.0)).child(format!("{name} ({n})")))
            .child(Button::new(ElementId::Name(format!("pl-play-{id}").into())).icon(ui::lucide("play")).outline().on_click(move |_, _, _| a.play_tracks(a.playlist_tracks(id), 0)))
            .child(Button::new(ElementId::Name(format!("pl-del-{id}").into())).icon(ui::lucide("trash")).ghost().on_click(move |_, _, cx| {
                b.delete_playlist(id);
                cx.refresh_windows();
            }))
    });

    page(p, &display, t(hub, "module.music"), Some(t(hub, "music.intro")))
        .child(now_playing)
        .child(ui::card(p).child(ui::h2(p, t(hub, "music.playlists"))).child(row().child(save)).children(pl_rows))
        .child(
            ui::card(p)
                .child(div().flex().flex_row().items_center().child(ui::h2(p, format!("{} ({})", t(hub, "music.library"), lib.len()))).child(div().flex_1()).child(row().child(scan_music).child(scan).child(explorer)))
                .when(lib.is_empty(), |d| d.child(ui::label(p, t(hub, "music.empty"))))
                .children(lib_rows),
        )
        .into_any_element()
}

// ── M17 Windows theme ───────────────────────────────────────────────────────

pub fn wintheme(hub: &mut Hub, p: &Palette, cx: &mut Context<Hub>) -> AnyElement {
    use halo_modules::wintheme as wt;
    let (display, mono, cfg, _fr) = {
        let s = hub.shared.borrow();
        (s.typo.display.clone(), s.typo.mono.clone(), s.config.settings.clone(), s.fr())
    };
    let light = wt::is_light(wt::PERSONALIZE).unwrap_or(false);
    let h = hub.shared.borrow().handles.wintheme.clone();
    let (h1, h2) = (h.clone(), h.clone());
    let now = ui::card(p)
        .flex_row()
        .items_center()
        .gap(px(tokens::SPACE_4))
        .child(ui::icon(if light { "sun" } else { "moon" }, 28.0, p.accent_text))
        .child(div().flex_1().flex().flex_col().child(ui::h2(p, if light { t(hub, "wt.light_now") } else { t(hub, "wt.dark_now") })).child(ui::label(p, t(hub, "wt.applies"))))
        .child(Button::new("wt-light").icon(ui::lucide("sun")).label(t(hub, "wt.to_light")).outline().on_click(move |_, _, cx| {
            h1.set(true);
            cx.refresh_windows();
        }))
        .child(Button::new("wt-dark").icon(ui::lucide("moon")).label(t(hub, "wt.to_dark")).primary().on_click(move |_, _, cx| {
            h2.set(false);
            cx.refresh_windows();
        }));
    let sched = [("off", "wt.manual"), ("times", "wt.times"), ("sun", "wt.sun")].map(|(id, k)| {
        hub.choice(format!("wt-s-{id}"), t(hub, k), cfg.wintheme_schedule == id, move |c| c.settings.wintheme_schedule = id.into())
    });
    let light_times = ["06:00", "07:00", "08:00"].map(|v| hub.choice(format!("wt-l-{v}"), v, cfg.wintheme_light_at == v, move |c| c.settings.wintheme_light_at = v.into()));
    let dark_times = ["18:00", "19:00", "20:00", "21:00"].map(|v| hub.choice(format!("wt-d-{v}"), v, cfg.wintheme_dark_at == v, move |c| c.settings.wintheme_dark_at = v.into()));
    let places: [(&str, f64, f64); 5] = [("Paris", 48.8566, 2.3522), ("Antananarivo", -18.8792, 47.5079), ("Pointe-à-Pitre", 16.2411, -61.5331), ("Montréal", 45.5017, -73.5673), ("Dakar", 14.7167, -17.4677)];
    let place_btns = places.map(|(n, la, lo)| {
        hub.choice(format!("wt-p-{n}"), n, (cfg.wintheme_lat - la).abs() < 0.01 && (cfg.wintheme_lon - lo).abs() < 0.01, move |c| {
            c.settings.wintheme_lat = la;
            c.settings.wintheme_lon = lo;
        })
    });
    let now_l = chrono::Local::now();
    let sun = wt::sun_times(chrono::Datelike::ordinal(&now_l), cfg.wintheme_lat, cfg.wintheme_lon, now_l.offset().local_minus_utc() as f64 / 3600.0)
        .map(|(r, s)| format!("{:02}:{:02} / {:02}:{:02}", r / 60, r % 60, s / 60, s % 60))
        .unwrap_or_else(|| "—".into());
    let wall = |light: bool| {
        let cur = if light { cfg.wallpaper_light.clone() } else { cfg.wallpaper_dark.clone() };
        let shown = if cur.is_empty() {
            wt::default_wallpaper(light).map(|p| format!("{} ({})", p.display(), t(hub, "wt.default"))).unwrap_or_default()
        } else {
            cur
        };
        let id = if light { "wt-wl" } else { "wt-wd" };
        let b = Button::new(id).icon(ui::lucide("image")).label(t(hub, "wt.choose")).outline().on_click(cx.listener(move |hub, _, _, cx| {
            let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: None });
            let shared = hub.shared.clone();
            cx.spawn(async move |this, cx| {
                if let Ok(Ok(Some(paths))) = rx.await {
                    if let Some(f) = paths.into_iter().next() {
                        let v = f.display().to_string();
                        shared.borrow_mut().update_config(|c| if light { c.settings.wallpaper_light = v } else { c.settings.wallpaper_dark = v });
                        let _ = this.update(cx, |_, cx| cx.notify());
                    }
                }
            })
            .detach();
        }));
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(tokens::SPACE_3))
            .child(ui::icon(if light { "sun" } else { "moon" }, 16.0, p.ink_2))
            .child(div().flex_1().min_w(px(0.0)).child(ui::mono(p, &mono, shown)))
            .child(b)
    };
    let accents = [("", "Windows"), ("#818CF8", "Halo"), ("#0078D4", "Bleu"), ("#10893E", "Vert"), ("#E81123", "Rouge")];
    let accent_row = |light: bool| {
        row().children(accents.map(|(hex, name)| {
            let cur = if light { &cfg.accent_light } else { &cfg.accent_dark };
            hub.choice(format!("wt-a-{light}-{name}"), name, cur == hex, move |c| {
                if light { c.settings.accent_light = hex.into() } else { c.settings.accent_dark = hex.into() }
            })
        }))
    };
    page(p, &display, t(hub, "module.wintheme"), Some(t(hub, "wt.intro")))
        .child(now)
        .child(
            ui::card(p)
                .child(ui::h2(p, t(hub, "wt.schedule")))
                .child(row().children(sched))
                .when(cfg.wintheme_schedule == "times", |d| {
                    d.child(ui::label(p, t(hub, "wt.light_from"))).child(row().children(light_times)).child(ui::label(p, t(hub, "wt.dark_from"))).child(row().children(dark_times))
                })
                .when(cfg.wintheme_schedule == "sun", |d| d.child(ui::label(p, t(hub, "wt.place"))).child(row().children(place_btns)).child(ui::mono(p, &mono, format!("{} {sun}", t(hub, "wt.sun_today"))))),
        )
        .child(ui::card(p).child(ui::h2(p, t(hub, "wt.wallpapers"))).child(wall(true)).child(wall(false)))
        .child(ui::card(p).child(ui::h2(p, t(hub, "wt.accent"))).child(ui::label(p, t(hub, "wt.light_mode"))).child(accent_row(true)).child(ui::label(p, t(hub, "wt.dark_mode"))).child(accent_row(false)))
        .into_any_element()
}
