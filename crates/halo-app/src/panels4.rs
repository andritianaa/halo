//! Phase 4–5 hub panels: Capture (M12–M14), Vault (M10), Network (M5).

use std::sync::{Arc, Mutex};

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use halo_capture::{AudioOpts, Format, Source};
use halo_secure::net::{LimiterStatus, Rule};
use halo_ui::hsla;
use halo_ui::tokens::{self, Palette};

use crate::hub::Hub;
use crate::panels3::page;
use crate::state::RegionPurpose;
use crate::ui;

pub struct Phase4State {
    pub vault_name: Entity<InputState>,
    pub vault_pw: Entity<InputState>,
    pub vault_recovery: Entity<InputState>,
    /// Recovery key to show once after creating a vault.
    pub shown_recovery: Arc<Mutex<Option<String>>>,
    pub busy: Arc<Mutex<bool>>,
    pub vault_msg: Arc<Mutex<Option<String>>>,
    /// Microphones found (listed when the capture page opens; refresh button).
    pub mics: Option<Vec<String>>,
    /// Live microphone test (level meter) while running.
    pub mic_test: Option<halo_capture::MicTest>,
    pub mic_test_error: Option<String>,
    /// Saved Wi-Fi networks (read when the page opens; refresh button).
    pub wifi: Option<Vec<halo_modules::wifikeys::WifiKey>>,
    /// Profiles whose password is currently revealed.
    pub wifi_shown: std::collections::HashSet<String>,
}

impl Phase4State {
    pub fn new(window: &mut Window, cx: &mut Context<Hub>) -> Self {
        Self {
            vault_name: cx.new(|cx| InputState::new(window, cx).placeholder("Name")),
            vault_pw: cx.new(|cx| InputState::new(window, cx).masked(true).placeholder("Password")),
            vault_recovery: cx.new(|cx| InputState::new(window, cx).placeholder("XXXXX-XXXXX-…")),
            shown_recovery: Default::default(),
            busy: Default::default(),
            vault_msg: Default::default(),
            mics: None,
            mic_test: None,
            mic_test_error: None,
            wifi: None,
            wifi_shown: Default::default(),
        }
    }
}

fn t(hub: &Hub, key: &str) -> String {
    hub.shared.borrow().t(key)
}

fn row() -> Div {
    div().flex().flex_row().flex_wrap().items_center().gap(px(tokens::SPACE_2))
}

/// Recording options from the settings (pointer, click highlight).
pub fn recording_effects(s: &halo_core::config::ModuleSettings) -> halo_capture::Effects {
    halo_capture::Effects { cursor: s.capture_cursor, clicks: halo_capture::ClickStyle::parse(&s.capture_clicks) }
}

fn primary_monitor_rect() -> halo_core::overlay::Rect {
    halo_capture::screenshot::monitor_bounds().into_iter().next().unwrap_or(halo_platform::monitor_for(0).work_area)
}

// ── Capture ─────────────────────────────────────────────────────────────────

pub fn capture(hub: &mut Hub, p: &Palette, cx: &mut Context<Hub>) -> AnyElement {
    let (display, mono, cfg) = {
        let s = hub.shared.borrow();
        (s.typo.display.clone(), s.typo.mono.clone(), s.config.settings.clone())
    };
    let c = hub.shared.borrow().capture.clone();
    let shot = c.screenshot.clone();
    let st = shot.settings();
    let shared = hub.shared.clone();

    // Screenshot
    let (s1, s2, s3) = (shared.clone(), shot.clone(), shot.clone());
    let shots = ui::card(p)
        .child(div().flex().flex_row().items_center().gap(px(tokens::SPACE_3)).child(ui::icon("camera", 18.0, p.accent_text)).child(ui::h2(p, t(hub, "capture.screenshot"))))
        .child(ui::label(p, t(hub, "capture.screenshot.hint")))
        .child(
            row()
                .child(Button::new("cap-region").icon(ui::lucide("scan")).label(t(hub, "capture.region")).primary().on_click(move |_, _, _| {
                    s1.borrow_mut().want_region = Some(RegionPurpose::Screenshot);
                }))
                .child(Button::new("cap-screen").icon(ui::lucide("monitor")).label(t(hub, "capture.fullscreen")).outline().on_click(move |_, _, _| {
                    let _ = s2.capture_screen(Some(0));
                }))
                .child(Button::new("cap-folder").icon(ui::lucide("folder-open")).label(t(hub, "capture.open_folder")).outline().on_click(move |_, _, cx| {
                    cx.reveal_path(&s3.settings().folder);
                })),
        )
        .child({
            let sh = shot.clone();
            let cur = st.clone();
            div().flex().flex_col().gap(px(tokens::SPACE_2)).child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(tokens::SPACE_4))
                    .child(div().flex_1().text_size(px(14.0)).child(t(hub, "capture.copy_clipboard")))
                    .child(gpui_kit::component::switch::Switch::new("cap-clip").checked(st.copy_to_clipboard).on_click(move |v: &bool, _, cx| {
                        let mut s = cur.clone();
                        s.copy_to_clipboard = *v;
                        sh.set_settings(s);
                        cx.refresh_windows();
                    })),
            )
        })
        .children(shot.last().map(|l| ui::mono(p, &mono, l.display().to_string())));

    // Screen recording
    let v = c.video.state();
    let (vh1, vh2, vh3) = (c.video.clone(), c.video.clone(), c.video.clone());
    let s4 = shared.clone();
    let (mic, sys, fps) = (cfg.capture_mic, cfg.capture_system, cfg.capture_fps);
    let effects = recording_effects(&cfg);
    let video = ui::card(p)
        .child(div().flex().flex_row().items_center().gap(px(tokens::SPACE_3)).child(ui::icon("video", 18.0, p.accent_text)).child(ui::h2(p, t(hub, "capture.video"))))
        .child(ui::label(p, t(hub, "capture.video.hint")))
        .child(if v.recording {
            row()
                .child(ui::badge(format!("REC {}", halo_modules::media::mmss(v.elapsed.as_secs_f64())), p.danger_text, p.danger.with_alpha_pct(16)))
                .child(Button::new("vid-pause").icon(ui::lucide(if v.paused { "play" } else { "pause" })).outline().on_click(move |_, _, cx| {
                    if vh1.state().paused { vh1.resume() } else { vh1.pause() }
                    cx.refresh_windows();
                }))
                .child(Button::new("vid-stop").icon(ui::lucide("square")).label(t(hub, "capture.stop")).primary().on_click(move |_, _, cx| {
                    let _ = vh2.stop();
                    cx.refresh_windows();
                }))
        } else {
            row()
                .child(Button::new("vid-region").icon(ui::lucide("scan")).label(t(hub, "capture.region")).primary().on_click(move |_, _, _| {
                    s4.borrow_mut().want_region = Some(RegionPurpose::Record);
                }))
                .child(Button::new("vid-screen").icon(ui::lucide("monitor")).label(t(hub, "capture.fullscreen")).outline().on_click(move |_, _, cx| {
                    let _ = vh3.start(primary_monitor_rect(), fps, AudioOpts { mic, system: sys }, effects);
                    cx.refresh_windows();
                }))
        })
        .child(hub.switch_row(p, "rec-sys", t(hub, "capture.system_audio"), None, cfg.capture_system, |c, v| c.settings.capture_system = v))
        .child(hub.switch_row(p, "rec-mic", t(hub, "capture.mic"), None, cfg.capture_mic, |c, v| c.settings.capture_mic = v))
        .child(row().children([15u32, 30, 60].map(|f| hub.choice(format!("fps-{f}"), format!("{f} fps"), cfg.capture_fps == f, move |c| c.settings.capture_fps = f))))
        .child(hub.switch_row(p, "rec-cursor", t(hub, "capture.cursor"), None, cfg.capture_cursor, |c, v| c.settings.capture_cursor = v))
        .child(ui::label(p, t(hub, "capture.clicks")))
        .child(row().children([("off", "capture.clicks.off"), ("ring", "capture.clicks.ring"), ("flash", "capture.clicks.flash")].map(|(id, key)| {
            hub.choice(format!("clicks-{id}"), t(hub, key), cfg.capture_clicks == id, move |c| c.settings.capture_clicks = id.into())
        })))
        .children(v.error.clone().map(|e| ui::mono(p, &mono, e)))
        .children(v.path.clone().filter(|_| !v.recording).map(|l| ui::mono(p, &mono, l.display().to_string())));

    // Audio recorder
    let a = c.audio.state();
    let (ah1, ah2, ah3) = (c.audio.clone(), c.audio.clone(), c.audio.clone());
    let flac = cfg.audio_format == "flac";
    let (amic, asys) = (cfg.audio_mic, cfg.audio_system);
    let audio = ui::card(p)
        .child(div().flex().flex_row().items_center().gap(px(tokens::SPACE_3)).child(ui::icon("mic", 18.0, p.accent_text)).child(ui::h2(p, t(hub, "capture.audio"))))
        .child(if a.recording {
            row()
                .child(ui::badge(format!("REC {}", halo_modules::media::mmss(a.elapsed.as_secs_f64())), p.danger_text, p.danger.with_alpha_pct(16)))
                .child(div().w(px(160.0)).child(ui::meter(p, a.level.peak)))
                .child(Button::new("aud-pause").icon(ui::lucide(if a.paused { "play" } else { "pause" })).outline().on_click(move |_, _, cx| {
                    if ah1.state().paused { ah1.resume() } else { ah1.pause() }
                    cx.refresh_windows();
                }))
                .child(Button::new("aud-stop").icon(ui::lucide("square")).label(t(hub, "capture.stop")).primary().on_click(move |_, _, cx| {
                    let _ = ah2.stop();
                    cx.refresh_windows();
                }))
        } else {
            row().child(Button::new("aud-start").icon(ui::lucide("mic")).label(t(hub, "capture.record")).primary().on_click(move |_, _, cx| {
                let _ = ah3.start(Source { mic: amic, system: asys }, if flac { Format::Flac } else { Format::Wav });
                cx.refresh_windows();
            }))
        })
        .child(hub.switch_row(p, "aud-mic", t(hub, "capture.mic"), None, cfg.audio_mic, |c, v| c.settings.audio_mic = v))
        .child(hub.switch_row(p, "aud-sys", t(hub, "capture.system_audio"), None, cfg.audio_system, |c, v| c.settings.audio_system = v))
        .child(row().children([("wav", "WAV"), ("flac", "FLAC")].map(|(id, l)| hub.choice(format!("afmt-{id}"), l, cfg.audio_format == id, move |c| c.settings.audio_format = id.into()))))
        .children(a.error.clone().map(|e| ui::mono(p, &mono, e)))
        .children(a.path.clone().filter(|_| !a.recording).map(|l| ui::mono(p, &mono, l.display().to_string())));

    // Microphone: choice + live test.
    if hub.p4.mics.is_none() {
        hub.p4.mics = Some(halo_capture::microphones());
    }
    let mics = hub.p4.mics.clone().unwrap_or_default();
    let testing = hub.p4.mic_test.as_ref().map(|m| m.level());
    if testing.is_some() {
        // Smooth meter while the test runs (the hub otherwise refreshes once a second).
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(50)).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }
    let mut choices = vec![hub.choice("mic-default".to_string(), t(hub, "capture.mic.default"), cfg.mic_device.is_empty(), |c| c.settings.mic_device.clear())];
    for (i, name) in mics.iter().enumerate() {
        let n = name.clone();
        choices.push(hub.choice(format!("mic-{i}"), name.clone(), cfg.mic_device == *name, move |c| c.settings.mic_device = n.clone()));
    }
    let microphone = ui::card(p)
        .child(div().flex().flex_row().items_center().gap(px(tokens::SPACE_3)).child(ui::icon("audio-lines", 18.0, p.accent_text)).child(ui::h2(p, t(hub, "capture.mic.title"))))
        .child(ui::label(p, t(hub, "capture.mic.hint")))
        .child(row().children(choices).child(Button::new("mic-refresh").icon(ui::lucide("refresh-cw")).ghost().tooltip(t(hub, "capture.mic.refresh")).on_click(cx.listener(|hub, _, _, cx| {
            hub.p4.mics = None;
            cx.notify();
        }))))
        .child(
            row()
                .child({
                    let b = Button::new("mic-test")
                        .icon(ui::lucide(if testing.is_some() { "square" } else { "mic" }))
                        .label(if testing.is_some() { t(hub, "capture.mic.stop_test") } else { t(hub, "capture.mic.test") })
                        .on_click(cx.listener(|hub, _, _, cx| {
                            if hub.p4.mic_test.take().is_none() {
                                let name = hub.shared.borrow().config.settings.mic_device.clone();
                                match halo_capture::MicTest::start((!name.is_empty()).then_some(name.as_str())) {
                                    Ok(m) => {
                                        hub.p4.mic_test = Some(m);
                                        hub.p4.mic_test_error = None;
                                    }
                                    Err(e) => hub.p4.mic_test_error = Some(e.to_string()),
                                }
                            }
                            cx.notify();
                        }));
                    if testing.is_some() { b.primary() } else { b.outline() }
                })
                .children(testing.map(|lvl| div().w(px(260.0)).child(ui::meter(p, lvl)))),
        )
        .when(testing.is_some(), |d| d.child(ui::label(p, t(hub, "capture.mic.speak"))))
        .children(hub.p4.mic_test_error.clone().map(|e| ui::mono(p, &mono, e)));

    page(p, &display, t(hub, "module.capture"), Some(t(hub, "capture.intro")))
        .child(shots)
        .child(video)
        .child(audio)
        .child(microphone)
        .child(ui::mono(p, &mono, "Ctrl+Alt+S · Ctrl+Alt+R"))
        .into_any_element()
}

// ── Vault ───────────────────────────────────────────────────────────────────

pub fn vault(hub: &mut Hub, p: &Palette, cx: &mut Context<Hub>) -> AnyElement {
    let (display, mono, running) = {
        let s = hub.shared.borrow();
        (s.typo.display.clone(), s.typo.mono.clone(), s.module_running("vault"))
    };
    let pg = page(p, &display, t(hub, "module.vault"), Some(t(hub, "vault.intro")));
    if !running {
        return pg.child(crate::panels2::opt_in_card(hub, p, "vault", "vault.what")).into_any_element();
    }
    let h = hub.shared.borrow().secure.vault.clone();
    let busy = *hub.p4.busy.lock().unwrap();
    let msg = hub.p4.vault_msg.lock().unwrap().clone().or_else(|| h.last_error());
    let recovery = hub.p4.shown_recovery.lock().unwrap().clone();
    let pw = hub.p4.vault_pw.read(cx).value().to_string();
    let strength = halo_secure::vault::password_strength(&pw);

    // Recovery key shown exactly once after creation.
    if let Some(key) = recovery {
        let shown = hub.p4.shown_recovery.clone();
        return pg
            .child(
                ui::card(p)
                    .child(div().flex().flex_row().items_center().gap(px(tokens::SPACE_3)).child(ui::icon("key-round", 20.0, p.warning_text)).child(ui::h2(p, t(hub, "vault.recovery_title"))))
                    .child(ui::label(p, t(hub, "vault.recovery_hint")))
                    .child(div().p(px(tokens::SPACE_4)).rounded(px(tokens::RADIUS_MD)).bg(hsla(p.surface_2)).font_family(SharedString::from(mono.clone())).text_size(px(18.0)).child(key.clone()))
                    .child(row().child(Button::new("vault-copy-key").icon(ui::lucide("copy")).label(t(hub, "common.copy")).outline().on_click(move |_, _, _| {
                        ui::copy_text(&key);
                    })).child(Button::new("vault-key-done").label(t(hub, "vault.recovery_done")).primary().on_click(move |_, _, cx| {
                        *shown.lock().unwrap() = None;
                        cx.refresh_windows();
                    }))),
            )
            .into_any_element();
    }

    let pending = hub.shared.borrow().pending_vault.len();
    match h.unlocked() {
        Some(name) => {
            // Files dropped on the notch while the vault was locked: add them now.
            if pending > 0 {
                let files = std::mem::take(&mut hub.shared.borrow_mut().pending_vault);
                let h = h.clone();
                std::thread::spawn(move || {
                    for f in files {
                        if let Err(e) = h.add_file(&f) {
                            log::warn!("vault: {}: {e}", f.display());
                        }
                    }
                });
            }
            let (a, b) = (h.clone(), h.clone());
            let files = h.list().into_iter().map(|e| {
                let id = e.id.clone();
                let (x, y, z) = (h.clone(), h.clone(), h.clone());
                let id2 = id.clone();
                let id3 = id.clone();
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(tokens::SPACE_3))
                    .px(px(tokens::SPACE_3))
                    .py(px(tokens::SPACE_2))
                    .rounded(px(tokens::RADIUS_SM))
                    .bg(hsla(p.surface_2))
                    .child(ui::icon("file", 16.0, p.ink_2))
                    .child(div().flex_1().text_size(px(13.0)).truncate().child(e.name.clone()))
                    .child(ui::mono(p, &mono, halo_modules::system::format_bytes(e.size)))
                    .child(Button::new(ElementId::Name(format!("v-open-{id}").into())).icon(ui::lucide("eye")).ghost().on_click(move |_, _, cx| {
                        if let Ok(path) = x.extract_temp(&id) {
                            cx.open_with_system(&path);
                        }
                    }))
                    .child(Button::new(ElementId::Name(format!("v-ext-{id2}").into())).icon(ui::lucide("download")).ghost().on_click(cx.listener(move |_, _, _, cx| {
                        let rx = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: None });
                        let (h, id) = (y.clone(), id2.clone());
                        cx.spawn(async move |_, _| {
                            if let Ok(Ok(Some(dirs))) = rx.await {
                                if let Some(d) = dirs.first() {
                                    let _ = h.extract(&id, d);
                                }
                            }
                        })
                        .detach();
                    })))
                    .child(Button::new(ElementId::Name(format!("v-del-{id3}").into())).icon(ui::lucide("trash")).ghost().on_click(move |_, _, cx| {
                        let _ = z.remove(&id3);
                        cx.refresh_windows();
                    }))
            });
            pg.child(
                ui::card(p)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(tokens::SPACE_3))
                            .child(ui::icon("lock-open", 18.0, p.success_text))
                            .child(ui::h2(p, name))
                            .child(div().flex_1())
                            .child(Button::new("vault-add").icon(ui::lucide("plus")).label(t(hub, "vault.add")).outline().on_click(cx.listener(move |_, _, _, cx| {
                                let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: None });
                                let h = a.clone();
                                cx.spawn(async move |this, cx| {
                                    if let Ok(Ok(Some(paths))) = rx.await {
                                        for f in paths {
                                            let _ = h.add_file(&f);
                                        }
                                        let _ = this.update(cx, |_, cx| cx.notify());
                                    }
                                })
                                .detach();
                            })))
                            .child(Button::new("vault-lock").icon(ui::lucide("lock")).label(t(hub, "vault.lock")).primary().on_click(move |_, _, cx| {
                                b.lock();
                                cx.refresh_windows();
                            })),
                    )
                    .child(ui::label(p, t(hub, "vault.autolock")))
                    .children(files),
            )
            .into_any_element()
        }
        None => {
            let vaults = h.vaults();
            let strength_label = ["vault.pw.0", "vault.pw.1", "vault.pw.2", "vault.pw.3", "vault.pw.4"][strength.min(4) as usize];
            let unlock_rows = vaults.iter().map(|name| {
                let (h, name2) = (h.clone(), name.clone());
                let pw_in = hub.p4.vault_pw.clone();
                let rec_in = hub.p4.vault_recovery.clone();
                let (busy_flag, msg_slot) = (hub.p4.busy.clone(), hub.p4.vault_msg.clone());
                let (h2, name3, busy2, msg2) = (h.clone(), name.clone(), hub.p4.busy.clone(), hub.p4.vault_msg.clone());
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(tokens::SPACE_3))
                    .child(ui::icon("lock", 16.0, p.ink_2))
                    .child(div().flex_1().text_size(px(14.0)).child(name.clone()))
                    .child(Button::new(ElementId::Name(format!("v-unlock-{name}").into())).label(t(hub, "vault.unlock")).primary().on_click(cx.listener(move |_, _, _, cx| {
                        let pw = pw_in.read(cx).value().to_string();
                        run_blocking(&busy_flag, &msg_slot, cx, {
                            let (h, n) = (h.clone(), name2.clone());
                            move || h.unlock(&n, &pw).map(|_| ()).map_err(|e| e.to_string())
                        });
                    })))
                    .child(Button::new(ElementId::Name(format!("v-rec-{name}").into())).label(t(hub, "vault.use_recovery")).outline().on_click(cx.listener(move |_, _, _, cx| {
                        let key = rec_in.read(cx).value().to_string();
                        run_blocking(&busy2, &msg2, cx, {
                            let (h, n) = (h2.clone(), name3.clone());
                            move || h.unlock_with_recovery(&n, &key).map(|_| ()).map_err(|e| e.to_string())
                        });
                    })))
            });
            let (h3, name_in, pw_in) = (h.clone(), hub.p4.vault_name.clone(), hub.p4.vault_pw.clone());
            let (busy3, msg3, shown) = (hub.p4.busy.clone(), hub.p4.vault_msg.clone(), hub.p4.shown_recovery.clone());
            let pending_label = (pending > 0).then(|| hub.shared.borrow().tf("vault.pending", &[("n", &pending.to_string())]));
            pg.child(
                ui::card(p)
                    .child(ui::h2(p, t(hub, "vault.unlock_title")))
                    .children(pending_label.map(|l| ui::badge(l, p.accent_text, p.accent_soft)))
                    .child(div().w(px(360.0)).child(Input::new(&hub.p4.vault_pw)))
                    .when(vaults.is_empty(), |d| d.child(ui::label(p, t(hub, "vault.none"))))
                    .children(unlock_rows)
                    .when(!vaults.is_empty(), |d| d.child(ui::label(p, t(hub, "vault.recovery_input"))).child(div().w(px(360.0)).child(Input::new(&hub.p4.vault_recovery)))),
            )
            .child(
                ui::card(p)
                    .child(ui::h2(p, t(hub, "vault.create")))
                    .child(div().w(px(360.0)).child(Input::new(&hub.p4.vault_name)))
                    .child(div().flex().flex_row().items_center().gap(px(tokens::SPACE_3)).child(div().w(px(200.0)).child(ui::meter(p, strength as f32 / 4.0))).child(ui::mono(p, &mono, t(hub, strength_label))))
                    .child(ui::label(p, t(hub, "vault.warning")))
                    .child(row().child(Button::new("vault-create").icon(ui::lucide("shield")).label(t(hub, "vault.create")).primary().on_click(cx.listener(move |_, _, _, cx| {
                        let name = name_in.read(cx).value().to_string();
                        let pw = pw_in.read(cx).value().to_string();
                        if name.trim().is_empty() || pw.len() < 8 {
                            *msg3.lock().unwrap() = Some("8+".into());
                            cx.notify();
                            return;
                        }
                        let shown = shown.clone();
                        run_blocking(&busy3, &msg3, cx, {
                            let h = h3.clone();
                            move || {
                                h.create(&name, &pw).map(|k| *shown.lock().unwrap() = Some(k.as_str().to_string())).map_err(|e| e.to_string())
                            }
                        });
                    })))),
            )
            .when(busy, |d| d.child(ui::label(p, t(hub, "vault.working"))))
            .children(msg.map(|m| ui::mono(p, &mono, m)))
            .into_any_element()
        }
    }
}

/// Run a slow (Argon2) operation on a thread; the hub refreshes every second.
fn run_blocking(
    busy: &Arc<Mutex<bool>>,
    msg: &Arc<Mutex<Option<String>>>,
    cx: &mut Context<Hub>,
    f: impl FnOnce() -> Result<(), String> + Send + 'static,
) {
    if std::mem::replace(&mut *busy.lock().unwrap(), true) {
        return;
    }
    *msg.lock().unwrap() = None;
    let (busy, msg) = (busy.clone(), msg.clone());
    std::thread::spawn(move || {
        if let Err(e) = f() {
            *msg.lock().unwrap() = Some(e);
        }
        *busy.lock().unwrap() = false;
    });
    cx.notify();
}

// ── Network ─────────────────────────────────────────────────────────────────

/// Download is drawn in the accent, upload in the info color (two series, flat fills).
fn traffic_colors(p: &Palette) -> (tokens::Color, tokens::Color) {
    (p.accent, p.info)
}

/// Live speed tile: icon, label, big value, footnote.
fn speed_tile(p: &Palette, display: &str, mono: &str, icon: &str, color: tokens::Color, label: String, value: String, foot: String) -> Div {
    ui::card(p)
        .flex_1()
        .min_w(px(180.0))
        .gap(px(tokens::SPACE_1))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(tokens::SPACE_2))
                .child(ui::icon(icon, 16.0, color))
                .child(ui::label(p, label)),
        )
        .child(div().font_family(SharedString::from(display.to_string())).text_size(px(tokens::TEXT_H1.size)).font_weight(FontWeight::SEMIBOLD).child(value))
        .child(ui::mono(p, mono, foot))
}

/// Legend entry: colored dot + text.
fn legend(p: &Palette, color: tokens::Color, text: String) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .child(div().size(px(8.0)).rounded_full().bg(hsla(color)))
        .child(ui::label(p, text))
}

/// Stacked bars (download under upload) for the usage history.
fn usage_bars(p: &Palette, mono: &str, buckets: &[(i64, u64, u64)], hourly: bool, locale: &str) -> Div {
    let (down_c, up_c) = traffic_colors(p);
    let max = buckets.iter().map(|(_, r, t)| r + t).max().unwrap_or(0).max(1) as f32;
    let loc = chrono::Locale::try_from(locale).unwrap_or(chrono::Locale::en_US);
    let label = |start: i64| {
        chrono::DateTime::from_timestamp(start, 0)
            .map(|d| {
                let d = d.with_timezone(&chrono::Local);
                if hourly { d.format("%H:00").to_string() } else { d.format_localized("%-d %b", loc).to_string() }
            })
            .unwrap_or_default()
    };
    let every = if hourly { 6 } else if buckets.len() > 10 { 5 } else { 1 };
    let bars = buckets.iter().enumerate().map(|(i, (start, rx, tx))| {
        let tip = format!("{} · ↓ {} · ↑ {}", label(*start), halo_modules::system::format_bytes(*rx), halo_modules::system::format_bytes(*tx));
        div()
            .id(("usage-bar", i))
            .flex_1()
            .h_full()
            .flex()
            .flex_col()
            .justify_end()
            .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
            .child(div().w_full().h(relative(*tx as f32 / max)).rounded_t(px(2.0)).bg(hsla(up_c)))
            .child(div().w_full().h(relative(*rx as f32 / max)).bg(hsla(down_c)))
            // Empty bucket: faint marker so the time axis stays readable.
            .when(rx + tx == 0, |d| d.child(div().w_full().h(px(2.0)).rounded_t(px(1.0)).bg(hsla(p.surface_3))))
    });
    let ticks = buckets.iter().enumerate().map(|(i, (start, ..))| {
        div().flex_1().overflow_hidden().child(if i % every == 0 { ui::mono(p, mono, label(*start)) } else { div() })
    });
    div()
        .flex()
        .flex_col()
        .gap(px(tokens::SPACE_1))
        .child(div().h(px(140.0)).flex().flex_row().items_end().gap(px(2.0)).border_b_1().border_color(hsla(p.line)).children(bars))
        .child(div().flex().flex_row().gap(px(2.0)).children(ticks))
}

pub fn network(hub: &mut Hub, p: &Palette, cx: &mut Context<Hub>) -> AnyElement {
    use halo_secure::traffic::{Period, format_rate};
    use halo_modules::system::format_bytes;
    let (display, mono, running, locale) = {
        let s = hub.shared.borrow();
        (s.typo.display.clone(), s.typo.mono.clone(), s.module_running("netlimit"), s.config.lang.locale_code())
    };
    let pg = page(p, &display, t(hub, "module.netlimit"), Some(t(hub, "net.intro")));
    if !running {
        return pg.child(crate::panels2::opt_in_card(hub, p, "netlimit", "net.what")).into_any_element();
    }
    let h = hub.shared.borrow().secure.net.clone();
    let snap = h.snapshot();
    let tr = h.traffic();
    let apps = h.app_traffic();
    let rules = h.rules();
    let status = h.limiter_status();
    let (down_c, up_c) = traffic_colors(p);
    let tf = |k: &str, args: &[(&str, &str)]| hub.shared.borrow().tf(k, args);

    // ── Live speed tiles ────────────────────────────────────────────────────
    let tiles = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .gap(px(tokens::SPACE_4))
        .child(speed_tile(p, &display, &mono, "arrow-down", down_c, t(hub, "net.download"), format_rate(tr.rate.0), tf("net.session", &[("v", &format_bytes(tr.session.0))])))
        .child(speed_tile(p, &display, &mono, "arrow-up", up_c, t(hub, "net.upload"), format_rate(tr.rate.1), tf("net.session", &[("v", &format_bytes(tr.session.1))])))
        .child(speed_tile(
            p,
            &display,
            &mono,
            "chart-column",
            p.ink_2,
            t(hub, tr.period.key()),
            format_bytes(tr.total.0 + tr.total.1),
            format!("↓ {} · ↑ {}", format_bytes(tr.total.0), format_bytes(tr.total.1)),
        ));

    // ── Live graph (5 min) ──────────────────────────────────────────────────
    let scale = tr.peak.max(64 * 1024) as f32;
    let series = |pick: fn(&(u64, u64)) -> u64| tr.live.iter().map(|v| pick(v) as f32 / scale * 100.0).collect::<Vec<f32>>();
    let live = ui::card(p)
        .child(
            row()
                .child(ui::h2(p, t(hub, "net.live")))
                .child(div().flex_1())
                .children(tr.connection.clone().map(|c| ui::badge(c, p.ink_2, p.surface_2)))
                .child(legend(p, down_c, t(hub, "net.download")))
                .child(legend(p, up_c, t(hub, "net.upload"))),
        )
        .child(
            div()
                .relative()
                .h(px(150.0))
                .child(ui::chart(series(|v| v.0), 150.0, down_c, p.line))
                .child(div().absolute().inset_0().child(ui::chart(series(|v| v.1), 150.0, up_c, p.line))),
        )
        .child(row().child(ui::mono(p, &mono, t(hub, "net.last5")) ).child(div().flex_1()).child(ui::mono(p, &mono, tf("net.peak", &[("v", &format_rate(tr.peak))]))))
        .when(!tr.available, |d| d.child(ui::label(p, t(hub, "net.no_counters"))));

    // ── Usage history ───────────────────────────────────────────────────────
    let chips = Period::ALL.map(|period| {
        let h = h.clone();
        let b = Button::new(ElementId::Name(format!("net-period-{period:?}").into())).label(t(hub, period.key())).compact().on_click(move |_, _, cx| {
            h.set_period(period);
            cx.refresh_windows();
        });
        if period == tr.period { b.primary() } else { b.ghost() }
    });
    let usage = ui::card(p)
        .child(row().child(ui::h2(p, t(hub, "net.usage"))).child(div().flex_1()).children(chips))
        .child(
            row()
                .child(legend(p, down_c, format!("{} · {}", t(hub, "net.download"), format_bytes(tr.total.0))))
                .child(legend(p, up_c, format!("{} · {}", t(hub, "net.upload"), format_bytes(tr.total.1)))),
        )
        .child(if tr.buckets.is_empty() {
            ui::label(p, tr.history_error.clone().map(|_| t(hub, "net.no_history")).unwrap_or_else(|| t(hub, "common.loading")))
        } else {
            usage_bars(p, &mono, &tr.buckets, tr.hourly, locale)
        });

    // ── Apps (NetLimiter-style table) ──────────────────────────────────────
    let max_total = apps.iter().map(|a| a.total()).max().unwrap_or(0).max(1) as f32;
    let speed = |v: Option<u64>| v.map(format_rate).unwrap_or_else(|| "—".into());
    // Fixed numeric columns stay narrow so the app name keeps room at 125–150 % scaling.
    const SPEED_W: f32 = 70.0;
    const CONN_W: f32 = 40.0;
    const ACTIONS_W: f32 = 76.0;
    let col = |w: f32, el: Div| div().w(px(w)).flex_none().child(el);
    let header = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(tokens::SPACE_2))
        .pb(px(tokens::SPACE_2))
        .border_b_1()
        .border_color(hsla(p.line))
        .child(div().flex_1().child(ui::mono(p, &mono, t(hub, "net.col.app"))))
        .child(col(SPEED_W, ui::mono(p, &mono, "↓ /s")))
        .child(col(SPEED_W, ui::mono(p, &mono, "↑ /s")))
        .child(col(SPEED_W, ui::mono(p, &mono, format!("↓ {}", t(hub, "net.col.total")))))
        .child(col(SPEED_W, ui::mono(p, &mono, format!("↑ {}", t(hub, "net.col.total")))))
        .child(col(CONN_W, ui::mono(p, &mono, t(hub, "net.col.conn"))))
        .child(col(ACTIONS_W, div()));
    let rows = apps.into_iter().take(40).enumerate().map(|(i, a)| {
        let rule = a.exe.as_ref().and_then(|e| rules.iter().find(|r| r.matches(e)).cloned());
        let blocked = rule.as_ref().is_some_and(|r| r.block);
        let limited = rule.as_ref().and_then(|r| r.down_kbps);
        let share = a.total() as f32 / max_total;
        let actions = match a.exe.clone() {
            Some(exe) => {
                let (h1, h2, e1, e2) = (h.clone(), h.clone(), exe.clone(), exe);
                let lim = Button::new(ElementId::Name(format!("net-lim-{i}").into()))
                    .icon(ui::lucide("gauge"))
                    .when_some(limited, |b, k| b.label(if k >= 1024 { format!("{}M", k / 1024) } else { format!("{k}k") }))
                    .tooltip(match limited { Some(k) => format!("{k} kb/s"), None => t(hub, "net.no_limit") })
                    .compact()
                    .on_click(move |_, _, cx| {
                        // Cycle: none → 1 Mb/s → 256 kb/s → none.
                        let next = match limited { None => Some(1024), Some(1024) => Some(256), _ => None };
                        let _ = h1.set_rule(Rule { exe: e1.clone(), down_kbps: next, up_kbps: next, block: blocked });
                        cx.refresh_windows();
                    });
                let block = Button::new(ElementId::Name(format!("net-block-{i}").into())).icon(ui::lucide("ban")).compact().tooltip(t(hub, "net.block")).on_click(move |_, _, cx| {
                    let _ = h2.set_rule(Rule { exe: e2.clone(), down_kbps: limited, up_kbps: limited, block: !blocked });
                    cx.refresh_windows();
                });
                row().child(if limited.is_some() { lim.primary() } else { lim.ghost() }).child(if blocked { block.danger() } else { block.ghost() })
            }
            None => div(),
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(tokens::SPACE_2))
            .py(px(6.0))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(div().text_size(px(tokens::TEXT_LABEL.size)).truncate().child(if a.name.is_empty() { t(hub, "net.system") } else { a.name.clone() }))
                    .child(div().h(px(3.0)).w_full().rounded_full().bg(hsla(p.surface_3)).child(div().h_full().w(relative(share)).rounded_full().bg(hsla(down_c)))),
            )
            .child(col(SPEED_W, ui::mono(p, &mono, speed(a.rate_in))))
            .child(col(SPEED_W, ui::mono(p, &mono, speed(a.rate_out))))
            .child(col(SPEED_W, ui::mono(p, &mono, format_bytes(a.rx))))
            .child(col(SPEED_W, ui::mono(p, &mono, format_bytes(a.tx))))
            .child(col(CONN_W, ui::mono(p, &mono, if a.connections == 0 { "—".into() } else { a.connections.to_string() })))
            .child(col(ACTIONS_W, actions))
    });
    let apps_card = ui::card(p)
        .child(row().child(ui::h2(p, t(hub, "net.apps"))).child(div().flex_1()).child(ui::mono(p, &mono, t(hub, tr.period.key()))))
        .when(!snap.bytes_available, |d| d.child(ui::label(p, t(hub, "net.bytes_admin"))))
        .child(header)
        .children(rows);

    // ── Limiter + events ────────────────────────────────────────────────────
    let (status_key, fg) = match &status {
        LimiterStatus::DriverMissing => ("net.driver_missing", p.warning_text),
        LimiterStatus::NeedsElevation => ("net.needs_admin", p.warning_text),
        LimiterStatus::Ready => ("net.ready", p.success_text),
        LimiterStatus::Running => ("net.running", p.success_text),
        LimiterStatus::Error(_) => ("net.error", p.danger_text),
    };
    let ready = matches!(status, LimiterStatus::Ready);
    let running_lim = matches!(status, LimiterStatus::Running);
    let hl = h.clone();
    let limiter = ui::card(p)
        .child(row().child(ui::h2(p, t(hub, "net.limiter"))).child(ui::badge(t(hub, status_key), fg, fg.with_alpha_pct(16))).child(div().flex_1()).when(ready || running_lim, |d| {
            d.child(Button::new("net-start").label(if running_lim { t(hub, "net.stop") } else { t(hub, "net.start") }).outline().on_click(move |_, _, cx| {
                if running_lim { hl.stop_limiter() } else { let _ = hl.start_limiter(); }
                cx.refresh_windows();
            }))
        }))
        .child(ui::label(p, t(hub, "net.limiter_hint")));
    let events: Vec<_> = h.events().into_iter().rev().take(8).map(|e| {
        let when = chrono::DateTime::from_timestamp(e.at, 0).map(|d| d.with_timezone(&chrono::Local).format("%H:%M:%S").to_string()).unwrap_or_default();
        div().flex().flex_row().gap(px(tokens::SPACE_3)).child(ui::icon("globe", 14.0, p.info_text)).child(ui::mono(p, &mono, format!("{when} · {} · {}", e.exe, e.remote.map(|r| r.to_string()).unwrap_or_default())))
    }).collect();
    let _ = cx;
    pg.child(tiles)
        .child(live)
        .child(usage)
        .child(apps_card)
        .child(limiter)
        .when(!events.is_empty(), |d| d.child(ui::card(p).child(ui::h2(p, t(hub, "net.events"))).children(events)))
        .into_any_element()
}

// ── Wi-Fi passwords ─────────────────────────────────────────────────────────

pub fn wifi_keys(hub: &mut Hub, p: &Palette, cx: &mut Context<Hub>) -> AnyElement {
    let (display, mono, running) = {
        let s = hub.shared.borrow();
        (s.typo.display.clone(), s.typo.mono.clone(), s.module_running("wifikeys"))
    };
    let pg = page(p, &display, t(hub, "module.wifikeys"), Some(t(hub, "wifikeys.intro")));
    if !running {
        hub.p4.wifi = None;
        hub.p4.wifi_shown.clear();
        return pg.child(crate::panels2::opt_in_card(hub, p, "wifikeys", "wifikeys.what")).into_any_element();
    }
    if hub.p4.wifi.is_none() {
        hub.p4.wifi = Some(halo_modules::wifikeys::saved());
    }
    let list = hub.p4.wifi.clone().unwrap_or_default();
    let current = hub.shared.borrow().handles.system.get().wifi.filter(|w| w.connected).map(|w| w.ssid);
    let any_protected = list.iter().any(|k| k.protected);

    let rows = list.into_iter().enumerate().map(|(i, k)| {
        let shown = hub.p4.wifi_shown.contains(&k.name);
        let is_current = current.as_deref().is_some_and(|c| !c.is_empty() && c == k.ssid);
        let (name1, key1) = (k.name.clone(), k.key.clone());
        let secret: AnyElement = match (&k.key, k.protected) {
            (Some(key), _) => ui::mono(p, &mono, if shown { key.clone() } else { "•".repeat(key.chars().count().clamp(8, 16)) }).into_any_element(),
            (None, true) => ui::label(p, t(hub, "wifikeys.encrypted")).into_any_element(),
            (None, false) => ui::label(p, t(hub, "wifikeys.no_password")).into_any_element(),
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(tokens::SPACE_3))
            .px(px(tokens::SPACE_3))
            .py(px(tokens::SPACE_2))
            .rounded(px(tokens::RADIUS_MD))
            .bg(hsla(p.surface))
            .border_1()
            .border_color(hsla(p.line))
            .child(ui::icon("wifi", 18.0, if is_current { p.accent_text } else { p.ink_2 }))
            .child(
                div()
                    .w(px(260.0))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(div().text_size(px(tokens::TEXT_LABEL.size)).truncate().child(k.ssid.clone()))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(tokens::SPACE_2))
                            .child(ui::mono(p, &mono, k.security().to_string()))
                            .when(is_current, |d| d.child(ui::badge(t(hub, "wifikeys.connected"), p.success_text, p.success.with_alpha_pct(16)))),
                    ),
            )
            .child(div().flex_1().min_w(px(0.0)).truncate().child(secret))
            .when(k.key.is_some(), |d| {
                d.child(
                    Button::new(ElementId::Name(format!("wk-eye-{i}").into()))
                        .icon(ui::lucide(if shown { "eye-off" } else { "eye" }))
                        .ghost()
                        .tooltip(if shown { t(hub, "wifikeys.hide") } else { t(hub, "wifikeys.show") })
                        .on_click(cx.listener(move |hub, _, _, cx| {
                            if !hub.p4.wifi_shown.remove(&name1) {
                                hub.p4.wifi_shown.insert(name1.clone());
                            }
                            cx.notify();
                        })),
                )
                .child(Button::new(ElementId::Name(format!("wk-copy-{i}").into())).icon(ui::lucide("copy")).ghost().tooltip(t(hub, "wifikeys.copy")).on_click(move |_, _, _| {
                    if let Some(key) = &key1 {
                        ui::copy_secret(key);
                    }
                }))
            })
    });

    pg.child(
        row()
            .child(ui::label(p, t(hub, "wifikeys.local")))
            .child(div().flex_1())
            .child(Button::new("wk-refresh").icon(ui::lucide("refresh-cw")).label(t(hub, "wifikeys.refresh")).outline().on_click(cx.listener(|hub, _, _, cx| {
                hub.p4.wifi = None;
                cx.notify();
            })))
            .child(Button::new("wk-hide").icon(ui::lucide("eye-off")).label(t(hub, "wifikeys.hide_all")).ghost().on_click(cx.listener(|hub, _, _, cx| {
                hub.p4.wifi_shown.clear();
                cx.notify();
            }))),
    )
    .when(any_protected, |d| d.child(ui::card(p).child(ui::label(p, t(hub, "wifikeys.need_admin")))))
    .child(div().flex().flex_col().gap(px(tokens::SPACE_2)).children(rows))
    .into_any_element()
}
