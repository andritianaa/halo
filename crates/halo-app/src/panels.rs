//! Hub panels, one per section. Each returns an element built from tokens +
//! gpui-kit components; state that must survive re-renders lives in `PanelState`.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use halo_core::config::ThemePref;
use halo_core::i18n::Lang;
use halo_core::overlay::{Anchor, AutoHide, ExpandOn};
use halo_modules::accent;
use halo_modules::catalog::CATALOG;
use halo_modules::colorpicker::{Format, Rgb};
use halo_ui::hsla;
use halo_ui::tokens::{self, Color, Palette};

use crate::hub::Hub;
use crate::state::Section;
use crate::ui;

pub fn render(hub: &mut Hub, p: &Palette, window: &mut Window, cx: &mut Context<Hub>) -> AnyElement {
    crate::panels2::sync_placeholders(hub, window, cx);
    match hub.section {
        Section::Notes => crate::panels2::notes(hub, p, window, cx),
        Section::Clipboard => crate::panels2::clipboard(hub, p, cx),
        Section::Images => crate::panels2::images(hub, p, cx),
        Section::Activity => crate::panels2::activity(hub, p, cx),
        Section::Productivity => crate::panels2::productivity(hub, p, cx),
        Section::Resources => crate::panels3::resources(hub, p, cx),
        Section::Music => crate::panels3::music(hub, p, cx),
        Section::WinTheme => crate::panels3::wintheme(hub, p, cx),
        Section::Notifications => crate::panels2::notifications(hub, p, cx),
        Section::Capture => crate::panels4::capture(hub, p, cx),
        Section::Vault => crate::panels4::vault(hub, p, cx),
        Section::Network => crate::panels4::network(hub, p, cx),
        Section::WifiKeys => crate::panels4::wifi_keys(hub, p, cx),
        Section::General => general(hub, p, cx),
        Section::Modules => modules(hub, p),
        Section::System => system(hub, p),
        Section::Picker => picker(hub, p),
        Section::Accent => accent_panel(hub, p),
        Section::Spotlight => spotlight(hub, p),
        Section::Startup => startup(hub, p),
    }
}

fn page(p: &Palette, family: &str, title: String, intro: Option<String>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(tokens::SPACE_2)) // Fluent: cards stacked closely
        .w_full()
        .max_w(px(1000.0))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(tokens::SPACE_1))
                .pb(px(tokens::SPACE_4))
                .child(ui::h1(p, family, title))
                .children(intro.map(|i| ui::label(p, i))),
        )
}

fn row() -> Div {
    div().flex().flex_row().flex_wrap().gap(px(tokens::SPACE_2))
}

fn general(hub: &mut Hub, p: &Palette, _cx: &mut Context<Hub>) -> AnyElement {
    let s = hub.shared.borrow();
    let (cfg, display) = (s.config.clone(), s.typo.display.clone());
    let t = |k: &str| s.t(k);
    let positions = Anchor::ALL.map(|a| {
        hub.choice(format!("anchor-{a:?}"), t(a.label_key()), a == cfg.overlay.anchor, move |c| c.overlay.anchor = a)
    });
    let themes = [
        (ThemePref::System, "tray.theme.system"),
        (ThemePref::Dark, "tray.theme.dark"),
        (ThemePref::Light, "tray.theme.light"),
    ]
    .map(|(pref, key)| hub.choice(format!("theme-{pref:?}"), t(key), pref == cfg.theme, move |c| c.theme = pref));
    let langs: Vec<_> = Lang::ALL.iter().map(|&l| hub.choice(format!("lang-{l:?}"), l.native_name(), l == cfg.lang, move |c| c.lang = l)).collect();
    let expand = [(ExpandOn::Hover, "general.expand.hover"), (ExpandOn::Peek, "general.expand.peek"), (ExpandOn::Click, "general.expand.click")].map(
        |(e, key)| hub.choice(format!("expand-{e:?}"), t(key), e == cfg.overlay.expand_on, move |c| c.overlay.expand_on = e),
    );
    let icon_sets = [("lucide", "Lucide"), ("phosphor", "Phosphor"), ("hugeicons", "Hugeicons")].map(|(id, name)| {
        let label = if id == "lucide" { name.to_string() } else { format!("{name} · {}", t("general.icons.missing")) };
        hub.choice(format!("icons-{id}"), label, cfg.settings.icon_set == id, move |c| {
            // Only Lucide ships with gpui-kit; others stay selectable once bundled.
            if id == "lucide" {
                c.settings.icon_set = id.into();
            }
        })
    });
    let indicators = ["clock", "cpu", "ram", "battery", "volume", "brightness", "wifi", "bluetooth"].map(|id| {
        let on = cfg.overlay.indicators.iter().any(|i| i == id);
        hub.switch_row(p, format!("ind-{id}"), t(&format!("indicator.{id}")), None, on, move |c, v| {
            c.overlay.indicators.retain(|i| i != id);
            if v {
                c.overlay.indicators.push(id.into());
            }
        })
    });
    let notch_themes: Vec<_> = tokens::NOTCH_THEMES
        .iter()
        .map(|th| {
            let id = th.id;
            hub.choice(format!("nt-{id}"), if s.fr() { th.name_fr.to_string() } else { th.name_en.to_string() }, cfg.overlay.notch_theme == id, move |c| {
                c.overlay.notch_theme = id.into()
            })
        })
        .collect();
    let fade = matches!(cfg.overlay.auto_hide, AutoHide::FadeAfter { .. });
    drop(s);

    page(p, &display, t_(hub, "nav.general"), None)
        .child(ui::card(p).child(ui::h2(p, t_(hub, "settings.position"))).child(row().children(positions)))
        .child(
            ui::card(p)
                .child(ui::h2(p, t_(hub, "general.notch")))
                .child(hub.switch_row(
                    p,
                    "autohide",
                    t_(hub, "general.autohide"),
                    Some(t_(hub, "general.autohide.hint")),
                    fade,
                    |c, v| {
                        c.overlay.auto_hide = if v { AutoHide::default() } else { AutoHide::AlwaysVisible };
                    },
                ))
                .child(hub.switch_row(
                    p,
                    "clickthrough",
                    t_(hub, "general.clickthrough"),
                    None,
                    cfg.overlay.click_through,
                    |c, v| c.overlay.click_through = v,
                ))
                .child(hub.switch_row(
                    p,
                    "hud",
                    t_(hub, "general.hud"),
                    Some(t_(hub, "general.hud.hint")),
                    cfg.overlay.hud,
                    |c, v| c.overlay.hud = v,
                ))
                .child(ui::label(p, t_(hub, "general.expand")))
                .child(row().children(expand)),
        )
        .child(notch_content(hub, p))
        .child(updates_card(hub, p))
        .child(ui::card(p).child(ui::h2(p, t_(hub, "general.notch_theme"))).child(row().children(notch_themes)))
        .child(ui::card(p).child(ui::h2(p, t_(hub, "general.indicators"))).children(indicators))
        .child(ui::card(p).child(ui::h2(p, t_(hub, "tray.theme"))).child(row().children(themes)))
        .child(ui::card(p).child(ui::h2(p, t_(hub, "general.icons"))).child(row().children(icon_sets)))
        .child(ui::card(p).child(ui::h2(p, t_(hub, "general.language"))).child(row().children(langs)))
        .into_any_element()
}

fn t_(hub: &Hub, key: &str) -> String {
    hub.shared.borrow().t(key)
}

fn modules(hub: &mut Hub, p: &Palette) -> AnyElement {
    let (display, mono, cfg) = {
        let s = hub.shared.borrow();
        (s.typo.display.clone(), s.typo.mono.clone(), s.config.clone())
    };
    let ids: Vec<String> = hub.shared.borrow().registry.manifests().map(|(m, _)| m.id.to_string()).collect();
    let rows = CATALOG.iter().map(|e| {
        let implemented = ids.iter().any(|i| i == e.id);
        let state = hub.shared.borrow().registry.state(e.id);
        let failed = state == Some(halo_core::module::ModuleState::Failed);
        let mut badges = div().flex().flex_row().gap(px(tokens::SPACE_2));
        if e.opt_in {
            badges = badges.child(ui::badge(t_(hub, "module.badge.optin"), p.accent_text, p.accent_soft));
        }
        if e.permissions.contains(&halo_core::module::Permission::Elevation) {
            badges = badges.child(ui::badge(t_(hub, "module.badge.admin"), p.warning_text, p.warning.with_alpha_pct(16)));
        }
        if failed {
            badges = badges.child(ui::badge(t_(hub, "module.badge.failed"), p.danger_text, p.danger.with_alpha_pct(16)));
        }
        let id = e.id;
        let default = !e.opt_in && implemented && module_default(&hub.shared.borrow(), id);
        let on = cfg.module_enabled(id, default);
        let control = if implemented {
            gpui_kit::component::switch::Switch::new(ElementId::Name(format!("mod-{id}").into()))
                .checked(on)
                .on_click({
                    let shared = hub.shared.clone();
                    move |v: &bool, _, cx| {
                        let v = *v;
                        shared.borrow_mut().update_config(|c| {
                            c.modules.insert(id.to_string(), v);
                        });
                        cx.refresh_windows();
                    }
                })
                .into_any_element()
        } else {
            ui::mono(p, &mono, t_(hub, "modules.builtin")).into_any_element()
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(tokens::SPACE_3))
            .px(px(tokens::SPACE_4))
            .py(px(tokens::SPACE_3))
            .rounded(px(tokens::RADIUS_MD))
            .bg(hsla(p.surface))
            .border_1()
            .border_color(hsla(p.line))
            .when(!implemented, |d| d.opacity(0.6))
            .child(ui::icon(e.icon, tokens::ICON_MD, if on && implemented { p.accent_text } else { p.ink_2 }))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(div().text_size(px(tokens::TEXT_LABEL.size)).child(t_(hub, &format!("mod.{}.name", e.id))))
                    // What it does, in plain words (the spec codes M1…M19 stay internal).
                    .child(div().text_size(px(tokens::TEXT_MICRO.size)).text_color(hsla(p.ink_2)).child(t_(hub, &format!("mod.{}.desc", e.id)))),
            )
            .child(badges)
            .child(control)
    });
    page(p, &display, t_(hub, "nav.modules"), Some(t_(hub, "modules.intro"))).children(rows).into_any_element()
}

fn module_default(s: &crate::state::AppState, id: &str) -> bool {
    s.registry.manifests().find(|(m, _)| m.id == id).map(|(m, _)| m.default_enabled).unwrap_or(false)
}

fn system(hub: &mut Hub, p: &Palette) -> AnyElement {
    let (snap, display, mono) = {
        let s = hub.shared.borrow();
        (s.handles.system.get(), s.typo.display.clone(), s.typo.mono.clone())
    };
    let big = |v: String| {
        div().font_family(SharedString::from(display.clone())).text_size(px(28.0)).font_weight(FontWeight::SEMIBOLD).child(v)
    };
    let stat = |icon: &str, title: String, value: String, frac: Option<f32>, foot: String| {
        ui::card(p)
            .flex_1()
            .min_w(px(220.0))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(9.0))
                    .child(ui::icon(icon, 17.0, p.accent_text))
                    .child(div().text_size(px(14.0)).font_weight(FontWeight::SEMIBOLD).child(title)),
            )
            .child(big(value))
            .children(frac.map(|f| ui::meter(p, f)))
            .child(ui::mono(p, &mono, foot))
    };
    let b = &snap.battery;
    let wifi = snap.wifi.clone().unwrap_or_default();
    let vol = snap.volume.unwrap_or_default();
    let grid = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .gap(px(tokens::SPACE_4))
        .child(stat(
            "cpu",
            t_(hub, "indicator.cpu"),
            format!("{:.0} %", snap.cpu),
            Some(snap.cpu / 100.0),
            String::new(),
        ))
        .child(stat(
            "memory-stick",
            t_(hub, "indicator.ram"),
            halo_modules::system::format_bytes(snap.ram_used),
            Some(snap.ram_percent() / 100.0),
            format!("/ {}", halo_modules::system::format_bytes(snap.ram_total)),
        ))
        .child(stat(
            if b.charging { "battery-charging" } else { "battery" },
            t_(hub, "indicator.battery"),
            b.percent.map(|v| format!("{v} %")).unwrap_or_else(|| "—".into()),
            b.percent.map(|v| v as f32 / 100.0),
            match (b.present, b.on_ac, b.seconds_left) {
                (false, _, _) => t_(hub, "system.nobattery"),
                (_, true, _) => t_(hub, "system.onac"),
                (_, _, Some(s)) => format!("{} h {:02}", s / 3600, (s % 3600) / 60),
                _ => String::new(),
            },
        ))
        .child(stat(
            if vol.muted { "volume-x" } else { "volume-2" },
            t_(hub, "indicator.volume"),
            format!("{:.0}", vol.level * 100.0),
            Some(if vol.muted { 0.0 } else { vol.level }),
            if vol.muted { t_(hub, "system.muted") } else { String::new() },
        ))
        .child(stat(
            if wifi.connected { "wifi" } else { "wifi-off" },
            "Wi-Fi".into(),
            if wifi.connected { format!("{} %", wifi.signal) } else { "—".into() },
            wifi.connected.then_some(wifi.signal as f32 / 100.0),
            if wifi.connected { format!("{} · {} Mb/s", wifi.ssid, wifi.rx_mbps) } else { t_(hub, "system.offline") },
        ))
        .child(stat(
            if snap.bluetooth.enabled { "bluetooth" } else { "bluetooth-off" },
            "Bluetooth".into(),
            if !snap.bluetooth.radio {
                "—".into()
            } else if snap.bluetooth.enabled {
                format!("{}", snap.bluetooth.connected.len())
            } else {
                "off".into()
            },
            None,
            snap.bluetooth.connected.join(", "),
        ));
    let shared = hub.shared.clone();
    let shared2 = hub.shared.clone();
    let shared3 = hub.shared.clone();
    let controls = row()
        .child(Button::new("vol-down").label("−10").outline().on_click(move |_, _, cx| {
            let h = shared.borrow().handles.system.clone();
            let v = h.get().volume.map(|v| v.level).unwrap_or(0.5);
            h.set_volume(v - 0.1);
            cx.refresh_windows();
        }))
        .child(Button::new("vol-up").label("+10").outline().on_click(move |_, _, cx| {
            let h = shared2.borrow().handles.system.clone();
            let v = h.get().volume.map(|v| v.level).unwrap_or(0.5);
            h.set_volume(v + 0.1);
            cx.refresh_windows();
        }))
        .child(Button::new("vol-mute").label(t_(hub, "system.mute")).outline().on_click(move |_, _, cx| {
            shared3.borrow().handles.system.toggle_mute();
            cx.refresh_windows();
        }));
    page(p, &display.clone(), t_(hub, "module.system"), Some(snap.date_label(hub.shared.borrow().config.lang.locale_code())))
        .child(grid)
        .child(ui::card(p).child(ui::h2(p, t_(hub, "indicator.volume"))).child(controls))
        .into_any_element()
}

fn picker(hub: &mut Hub, p: &Palette) -> AnyElement {
    let (store, display, mono, hotkey) = {
        let s = hub.shared.borrow();
        (s.handles.picker.get(), s.typo.display.clone(), s.typo.mono.clone(), s.config.settings.picker_hotkey.clone())
    };
    let shared = hub.shared.clone();
    let launch = Button::new("pick-now").label(t_(hub, "picker.launch")).primary().on_click(move |_, _, _| {
        shared.borrow_mut().want_picker = true;
    });
    let formats = Format::ALL.map(|f| {
        let h = hub.shared.borrow().handles.picker.clone();
        let b = Button::new(ElementId::Name(format!("fmt-{f:?}").into()))
            .label(format!("{f:?}").to_uppercase())
            .on_click(move |_, _, cx| {
                h.update(|s| s.format = f);
                cx.refresh_windows();
            });
        if f == store.format { b.primary() } else { b.outline() }
    });
    let swatches = store.history.iter().enumerate().map(|(i, c)| {
        let c = *c;
        let text = c.format(store.format);
        div()
            .id(("swatch", i))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .w(px(96.0))
            .cursor_pointer()
            .child(div().h(px(56.0)).rounded(px(tokens::RADIUS_MD)).border_1().border_color(hsla(p.line)).bg(hsla(Color::hex(c.as_u32()))))
            .child(ui::mono(p, &mono, text.clone()))
            .on_click(move |_, _, _| {
                ui::copy_text(&text);
            })
    });
    let empty = store.history.is_empty();
    page(p, &display, t_(hub, "module.picker"), Some(format!("{} · {hotkey}", t_(hub, "picker.intro"))))
        .child(row().child(launch))
        .child(ui::card(p).child(ui::h2(p, t_(hub, "picker.format"))).child(row().children(formats)))
        .child(
            ui::card(p)
                .child(ui::h2(p, t_(hub, "picker.history")))
                .when(empty, |d| d.child(ui::label(p, t_(hub, "picker.empty"))))
                .child(div().flex().flex_row().flex_wrap().gap(px(tokens::SPACE_3)).children(swatches)),
        )
        .child(ui::mono(p, &mono, Rgb(0x81, 0x8C, 0xF8).format(store.format)))
        .into_any_element()
}

fn accent_panel(hub: &mut Hub, p: &Palette) -> AnyElement {
    let (cfg, display, mono) = {
        let s = hub.shared.borrow();
        (s.config.settings.clone(), s.typo.display.clone(), s.typo.mono.clone())
    };
    let activation = [("space", "accent.space"), ("arrows", "accent.arrows"), ("both", "accent.both")].map(|(id, key)| {
        hub.choice(format!("acc-act-{id}"), t_(hub, key), cfg.accent_activation == id, move |c| c.settings.accent_activation = id.into())
    });
    let delays = [0u32, 200, 300, 500, 800].map(|ms| {
        hub.choice(format!("acc-delay-{ms}"), format!("{ms} ms"), cfg.accent_delay_ms == ms, move |c| c.settings.accent_delay_ms = ms)
    });
    let sets = [(false, "accent.fr"), (true, "accent.all")].map(|(all, key)| {
        hub.choice(format!("acc-set-{all}"), t_(hub, key), cfg.accent_all_languages == all, move |c| {
            c.settings.accent_all_languages = all
        })
    });
    let set = if cfg.accent_all_languages { accent::CharSet::All } else { accent::CharSet::Fr };
    let table = ('a'..='z').filter_map(|l| accent::variants(l, set).map(|v| (l, v))).map(|(l, v)| {
        div()
            .flex()
            .flex_row()
            .gap(px(tokens::SPACE_3))
            .child(div().w(px(24.0)).font_family(SharedString::from(mono.clone())).text_color(hsla(p.ink_2)).child(l.to_string()))
            .child(div().text_size(px(16.0)).child(v.iter().collect::<String>()))
    });
    page(p, &display, t_(hub, "module.accent"), Some(t_(hub, "accent.intro")))
        .child(ui::card(p).child(ui::h2(p, t_(hub, "accent.activation"))).child(row().children(activation)))
        .child(ui::card(p).child(ui::h2(p, t_(hub, "accent.delay"))).child(ui::label(p, t_(hub, "accent.delay.hint"))).child(row().children(delays)))
        .child(ui::card(p).child(ui::h2(p, t_(hub, "accent.charset"))).child(row().children(sets)))
        .child(ui::card(p).children(table))
        .into_any_element()
}

fn spotlight(hub: &mut Hub, p: &Palette) -> AnyElement {
    let (cfg, display) = {
        let s = hub.shared.borrow();
        (s.config.settings.clone(), s.typo.display.clone())
    };
    let sens = [1u8, 2, 3, 4, 5].map(|v| {
        hub.choice(format!("sens-{v}"), v.to_string(), cfg.spotlight_sensitivity == v, move |c| {
            c.settings.spotlight_sensitivity = v
        })
    });
    let radius = [80.0f32, 110.0, 150.0].map(|v| {
        hub.choice(format!("rad-{v}"), format!("{v:.0} px"), cfg.spotlight_radius == v, move |c| c.settings.spotlight_radius = v)
    });
    let dim = [40u8, 60, 80].map(|v| {
        hub.choice(format!("dim-{v}"), format!("{v} %"), cfg.spotlight_dim == v, move |c| c.settings.spotlight_dim = v)
    });
    let shared = hub.shared.clone();
    let test = Button::new("spot-test").label(t_(hub, "spotlight.test")).primary().on_click(move |_, _, _| {
        let c = halo_platform::cursor_pos().unwrap_or_default();
        shared.borrow().handles.spotlight.trigger(c);
    });
    page(p, &display, t_(hub, "module.spotlight"), Some(t_(hub, "spotlight.intro")))
        .child(row().child(test))
        .child(ui::card(p).child(ui::h2(p, t_(hub, "spotlight.sensitivity"))).child(row().children(sens)))
        .child(ui::card(p).child(ui::h2(p, t_(hub, "spotlight.radius"))).child(row().children(radius)))
        .child(ui::card(p).child(ui::h2(p, t_(hub, "spotlight.dim"))).child(row().children(dim)))
        .into_any_element()
}

fn startup(hub: &mut Hub, p: &Palette) -> AnyElement {
    let (on, display, mono) = {
        let s = hub.shared.borrow();
        (s.module_running("autostart"), s.typo.display.clone(), s.typo.mono.clone())
    };
    let registered = halo_modules::autostart::is_enabled(halo_modules::autostart::RUN_KEY);
    page(p, &display, t_(hub, "module.autostart"), Some(t_(hub, "startup.intro")))
        .child(
            ui::card(p)
                .child(hub.switch_row(p, "autostart", t_(hub, "startup.toggle"), None, on, |c, v| {
                    c.modules.insert("autostart".into(), v);
                }))
                .child(ui::mono(
                    p,
                    &mono,
                    // Elevated builds start from a Task Scheduler entry; the Run value is the
                    // non-admin fallback. (Querying the task would spawn schtasks each frame.)
                    if registered {
                        format!("HKCU\\{}\\{}", halo_modules::autostart::RUN_KEY, halo_modules::autostart::VALUE)
                    } else {
                        format!("{} › {}", t_(hub, "startup.task"), halo_modules::autostart::TASK)
                    },
                ))
                .when(on, |d| d.child(ui::label(p, t_(hub, "startup.admin_hint")))),
        )
        .into_any_element()
}

/// "Contenu de l'encoche": choose and order the open-notch cards and bar shortcuts.
fn notch_content(hub: &Hub, p: &Palette) -> Div {
    let cfg = hub.shared.borrow().config.overlay.clone();
    // (id, icon); labels come from `card.<id>` in the locale files.
    const CARDS: &[(&str, &str)] = &[
        ("media", "disc"),
        ("today", "list-checks"),
        ("focus", "timer"),
        ("controls", "sliders-horizontal"),
        ("system", "gauge"),
        ("clock", "clock"),
    ];
    // Enabled cards in their order, then the disabled ones.
    let mut ordered: Vec<&(&str, &str)> = cfg.cards.iter().filter_map(|id| CARDS.iter().find(|c| c.0 == id)).collect();
    ordered.extend(CARDS.iter().filter(|c| !cfg.cards.iter().any(|x| x == c.0)));
    let rows = ordered.into_iter().map(|(id, icon)| {
        let id = *id;
        let label = t_(hub, &format!("card.{id}"));
        let on = cfg.cards.iter().any(|x| x == id);
        let (s1, s2) = (hub.shared.clone(), hub.shared.clone());
        let mv = move |shared: &crate::state::Shared, delta: i32| {
            shared.borrow_mut().update_config(|c| {
                if let Some(i) = c.overlay.cards.iter().position(|x| x == id) {
                    let j = (i as i32 + delta).clamp(0, c.overlay.cards.len() as i32 - 1) as usize;
                    c.overlay.cards.swap(i, j);
                }
            });
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(tokens::SPACE_3))
            .child(ui::icon(icon, 16.0, p.ink_2))
            .child(div().flex_1().text_size(px(tokens::TEXT_LABEL.size)).child(label))
            .when(on, |d| {
                d.child(Button::new(ElementId::Name(format!("card-up-{id}").into())).icon(ui::lucide("chevron-up")).ghost().on_click(move |_, _, cx| {
                    mv(&s1, -1);
                    cx.refresh_windows();
                }))
                .child(Button::new(ElementId::Name(format!("card-down-{id}").into())).icon(ui::lucide("chevron-down")).ghost().on_click(move |_, _, cx| {
                    mv(&s2, 1);
                    cx.refresh_windows();
                }))
            })
            .child(
                gpui_kit::component::switch::Switch::new(ElementId::Name(format!("card-{id}").into())).checked(on).on_click({
                    let shared = hub.shared.clone();
                    move |v: &bool, _, cx| {
                        let v = *v;
                        shared.borrow_mut().update_config(|c| {
                            c.overlay.cards.retain(|x| x != id);
                            if v {
                                c.overlay.cards.push(id.into());
                            }
                        });
                        cx.refresh_windows();
                    }
                }),
            )
    });
    let tabs = crate::overlay::TABS.iter().map(|(id, icon, _)| {
        let id = *id;
        let on = cfg.tabs.iter().any(|x| x == id);
        let shared = hub.shared.clone();
        let b = Button::new(ElementId::Name(format!("tab-opt-{id}").into()))
            .icon(ui::lucide(icon))
            .tooltip(hub.shared.borrow().t(&format!("tab.{id}")))
            .on_click(move |_, _, cx| {
            shared.borrow_mut().update_config(|c| {
                if c.overlay.tabs.iter().any(|x| x == id) {
                    c.overlay.tabs.retain(|x| x != id);
                } else {
                    c.overlay.tabs.push(id.into());
                }
            });
            cx.refresh_windows();
        });
        if on { b.primary() } else { b.outline() }
    });
    ui::card(p)
        .child(ui::h2(p, hub.shared.borrow().t("notch.content")))
        .child(ui::label(p, hub.shared.borrow().t("notch.content.hint")))
        .children(rows)
        .child(ui::label(p, hub.shared.borrow().t("notch.bar")))
        .child(row().children(tabs))
}

/// "Updates": current version, check now, install a newer release from GitHub.
fn updates_card(hub: &Hub, p: &Palette) -> Div {
    use halo_modules::update::{CURRENT, Status};
    let (status, auto) = {
        let s = hub.shared.borrow();
        (s.update.status(), s.config.settings.update_check)
    };
    let t = |k: &str| t_(hub, k);
    let tf = |k: &str, v: &str| hub.shared.borrow().tf(k, &[("v", v)]);
    let (line, fg) = match &status {
        Status::Idle => (tf("update.current", CURRENT), p.ink_2),
        Status::Checking => (t("update.checking"), p.ink_2),
        Status::UpToDate => (tf("update.uptodate", CURRENT), p.success_text),
        Status::Available(r) => (tf("update.available", &r.version), p.accent_text),
        Status::Downloading => (t("update.downloading"), p.ink_2),
        Status::Failed(e) => (format!("{} ({e})", t("update.failed")), p.danger_text),
    };
    let (h1, h2) = (hub.shared.borrow().update.clone(), hub.shared.borrow().update.clone());
    let mut actions = row().child(
        Button::new("upd-check").icon(ui::lucide("refresh-cw")).label(t("update.check")).outline().on_click(move |_, _, cx| {
            h1.check();
            cx.refresh_windows();
        }),
    );
    if let Status::Available(r) = &status {
        let page = r.page.clone();
        actions = actions
            .child(Button::new("upd-install").icon(ui::lucide("download")).label(t("update.install")).primary().on_click(move |_, _, cx| {
                // The installer replaces this version: quit as soon as it has started.
                h2.install(|| std::process::exit(0));
                cx.refresh_windows();
            }))
            .child(Button::new("upd-notes").icon(ui::lucide("external-link")).label(t("update.notes")).ghost().on_click(move |_, _, _| {
                crate::launcher::launch(std::path::Path::new(&page));
            }));
    }
    ui::card(p)
        .child(ui::h2(p, t("update.title")))
        .child(div().text_size(px(tokens::TEXT_LABEL.size)).text_color(hsla(fg)).child(line))
        .child(actions)
        .child(hub.switch_row(p, "upd-auto", t("update.auto"), Some(t("update.auto.hint")), auto, |c, v| c.settings.update_check = v))
}
