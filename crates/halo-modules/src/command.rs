//! Command bar ("Tapez une commande…") — 100 % local evaluation, FR + EN:
//! - math: `12*(3+4)/2`, `2^10`, `sqrt(2)`, `15% de 230`
//! - units: `10 km en miles`, `72 f en c`, `5 kg to lb`
//! - world clocks: `heure à Tokyo`, `time in New York`
//! - `tâche Appeler maman demain`, `rappel 15:00 Réunion`, `rappel dans 10 min pause`
//! - `note …`, and module shortcuts: `pipette`, `capture`, `focus`, `notes`…
//! No currency conversion: it would need online rates (spec: nothing is sent online).

use chrono::{DateTime, Duration as CDuration, Local, NaiveTime, TimeZone};

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Copy this text to the clipboard.
    Copy(String),
    AddTask { text: String, due: Option<i64> },
    AddReminder { text: String, at: i64 },
    AddNote(String),
    Open(&'static str),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    /// Live preview shown on the right of the bar.
    pub preview: String,
    /// Lucide icon name for the preview (UI never uses emoji).
    pub icon: &'static str,
    pub action: Option<Action>,
}

pub fn eval(input: &str, now: DateTime<Local>) -> Option<Outcome> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    let lower = s.to_lowercase();
    task_like(s, &lower, now)
        .or_else(|| open_module(&lower))
        .or_else(|| world_clock(&lower, now))
        .or_else(|| percent_of(&lower))
        .or_else(|| units(&lower))
        .or_else(|| math(&lower))
}

fn fmt_num(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 && v.abs() < 1e15 {
        format!("{}", v.round() as i64)
    } else {
        let s = format!("{v:.6}");
        let s = s.trim_end_matches('0').trim_end_matches('.');
        s.replace('.', ",")
    }
}

fn value(preview: String, copy: String) -> Option<Outcome> {
    Some(Outcome { preview, icon: "calculator", action: Some(Action::Copy(copy)) })
}

// ── math ────────────────────────────────────────────────────────────────────

fn math(s: &str) -> Option<Outcome> {
    let expr = s.trim_start_matches('=').replace(',', ".").replace('×', "*").replace('÷', "/");
    if !expr.chars().any(|c| c.is_ascii_digit()) || !expr.chars().any(|c| "+-*/^()s".contains(c)) {
        return None;
    }
    let mut p = Parser { s: expr.as_bytes(), i: 0 };
    let v = p.expr()?;
    p.ws();
    if p.i != p.s.len() || !v.is_finite() {
        return None;
    }
    let r = fmt_num(v);
    value(format!("= {r}"), r)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i] == b' ' {
            self.i += 1;
        }
    }
    fn eat(&mut self, c: u8) -> bool {
        self.ws();
        if self.s.get(self.i) == Some(&c) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn expr(&mut self) -> Option<f64> {
        let mut v = self.term()?;
        loop {
            if self.eat(b'+') {
                v += self.term()?;
            } else if self.eat(b'-') {
                v -= self.term()?;
            } else {
                return Some(v);
            }
        }
    }
    fn term(&mut self) -> Option<f64> {
        let mut v = self.power()?;
        loop {
            if self.eat(b'*') {
                v *= self.power()?;
            } else if self.eat(b'/') {
                let d = self.power()?;
                if d == 0.0 {
                    return None;
                }
                v /= d;
            } else if self.eat(b'%') {
                v /= 100.0;
            } else {
                return Some(v);
            }
        }
    }
    fn power(&mut self) -> Option<f64> {
        let b = self.unary()?;
        if self.eat(b'^') { Some(b.powf(self.power()?)) } else { Some(b) }
    }
    fn unary(&mut self) -> Option<f64> {
        if self.eat(b'-') {
            return Some(-self.unary()?);
        }
        self.ws();
        if self.s[self.i..].starts_with(b"sqrt") {
            self.i += 4;
            if !self.eat(b'(') {
                return None;
            }
            let v = self.expr()?;
            return self.eat(b')').then(|| v.sqrt());
        }
        if self.eat(b'(') {
            let v = self.expr()?;
            return self.eat(b')').then_some(v);
        }
        let start = self.i;
        while self.i < self.s.len() && (self.s[self.i].is_ascii_digit() || self.s[self.i] == b'.') {
            self.i += 1;
        }
        std::str::from_utf8(&self.s[start..self.i]).ok()?.parse().ok()
    }
}

fn num(s: &str) -> Option<f64> {
    s.trim().replace(',', ".").parse().ok()
}

fn percent_of(s: &str) -> Option<Outcome> {
    let (p, rest) = s.split_once('%')?;
    let rest = rest.trim();
    let base = rest.strip_prefix("de ").or_else(|| rest.strip_prefix("of ")).or_else(|| rest.strip_prefix("d'"))?;
    let v = num(p)? * num(base)? / 100.0;
    let r = fmt_num(v);
    value(format!("= {r}"), r)
}

// ── units ───────────────────────────────────────────────────────────────────

fn unit(u: &str) -> Option<(&'static str, f64)> {
    // (dimension, factor to SI base)
    Some(match u.trim_end_matches('s').trim_end_matches('.') {
        "km" | "kilometre" | "kilometer" => ("len", 1000.0),
        "m" | "metre" | "meter" => ("len", 1.0),
        "cm" => ("len", 0.01),
        "mm" => ("len", 0.001),
        "mi" | "mile" => ("len", 1609.344),
        "ft" | "pied" | "foot" | "feet" => ("len", 0.3048),
        "in" | "inch" | "pouce" => ("len", 0.0254),
        "kg" | "kilo" => ("mass", 1.0),
        "g" | "gramme" | "gram" => ("mass", 0.001),
        "lb" | "livre" | "pound" => ("mass", 0.453_592_37),
        "oz" | "once" | "ounce" => ("mass", 0.028_349_523),
        "l" | "litre" | "liter" => ("vol", 1.0),
        "ml" => ("vol", 0.001),
        "gal" | "gallon" => ("vol", 3.785_411_784),
        "km/h" | "kmh" => ("speed", 1.0 / 3.6),
        "mph" => ("speed", 0.44704),
        "m/" => ("speed", 1.0),
        "go" | "gb" => ("data", 1e9),
        "mo" | "mb" => ("data", 1e6),
        "to" | "tb" => ("data", 1e12),
        _ => return None,
    })
}

fn temp(u: &str) -> Option<char> {
    match u.trim_start_matches('°') {
        "c" | "celsius" => Some('c'),
        "f" | "fahrenheit" => Some('f'),
        "k" | "kelvin" => Some('k'),
        _ => None,
    }
}

fn units(s: &str) -> Option<Outcome> {
    let (lhs, to) = [" en ", " to ", " in ", " -> "].iter().find_map(|sep| s.split_once(sep))?;
    let lhs = lhs.trim();
    let split = lhs.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == ',' || c == '-'))?;
    let v = num(&lhs[..split])?;
    let from = lhs[split..].trim();
    let to = to.trim();
    if let (Some(a), Some(b)) = (temp(from), temp(to)) {
        let c = match a {
            'c' => v,
            'f' => (v - 32.0) * 5.0 / 9.0,
            _ => v - 273.15,
        };
        let r = match b {
            'c' => c,
            'f' => c * 9.0 / 5.0 + 32.0,
            _ => c + 273.15,
        };
        let r = fmt_num((r * 100.0).round() / 100.0);
        return value(format!("= {r} °{}", b.to_ascii_uppercase()), r);
    }
    let (da, fa) = unit(from)?;
    let (db, fb) = unit(to)?;
    if da != db {
        return None;
    }
    let r = fmt_num(((v * fa / fb) * 10_000.0).round() / 10_000.0);
    value(format!("= {r} {to}"), r)
}

// ── world clocks ────────────────────────────────────────────────────────────

const CITIES: &[(&str, chrono_tz::Tz)] = &[
    ("tokyo", chrono_tz::Asia::Tokyo),
    ("new york", chrono_tz::America::New_York),
    ("londres", chrono_tz::Europe::London),
    ("london", chrono_tz::Europe::London),
    ("paris", chrono_tz::Europe::Paris),
    ("montréal", chrono_tz::America::Toronto),
    ("montreal", chrono_tz::America::Toronto),
    ("los angeles", chrono_tz::America::Los_Angeles),
    ("san francisco", chrono_tz::America::Los_Angeles),
    ("sydney", chrono_tz::Australia::Sydney),
    ("dubai", chrono_tz::Asia::Dubai),
    ("pékin", chrono_tz::Asia::Shanghai),
    ("beijing", chrono_tz::Asia::Shanghai),
    ("shanghai", chrono_tz::Asia::Shanghai),
    ("singapour", chrono_tz::Asia::Singapore),
    ("singapore", chrono_tz::Asia::Singapore),
    ("antananarivo", chrono_tz::Indian::Antananarivo),
    ("guadeloupe", chrono_tz::America::Guadeloupe),
    ("réunion", chrono_tz::Indian::Reunion),
    ("dakar", chrono_tz::Africa::Dakar),
    ("berlin", chrono_tz::Europe::Berlin),
    ("moscou", chrono_tz::Europe::Moscow),
    ("bombay", chrono_tz::Asia::Kolkata),
    ("mumbai", chrono_tz::Asia::Kolkata),
    ("são paulo", chrono_tz::America::Sao_Paulo),
    ("sao paulo", chrono_tz::America::Sao_Paulo),
];

fn world_clock(s: &str, now: DateTime<Local>) -> Option<Outcome> {
    let city = ["heure à ", "heure a ", "time in ", "heure ", "time "].iter().find_map(|p| s.strip_prefix(p))?.trim();
    let (name, tz) = CITIES.iter().find(|(n, _)| *n == city)?;
    let there = now.with_timezone(tz);
    let diff = (there.offset().fix().local_minus_utc() - now.offset().local_minus_utc()) / 3600;
    let t = there.format("%H:%M").to_string();
    let name = name[..1].to_uppercase() + &name[1..];
    Some(Outcome { preview: format!("{name} : {t} ({diff:+} h)"), icon: "globe", action: Some(Action::Copy(t)) })
}

use chrono::Offset;

// ── tasks / reminders / notes / modules ─────────────────────────────────────

fn strip_any<'a>(s: &'a str, lower: &str, prefixes: &[&str]) -> Option<&'a str> {
    prefixes.iter().find(|p| lower.starts_with(*p)).map(|p| s[p.len()..].trim())
}

fn task_like(s: &str, lower: &str, now: DateTime<Local>) -> Option<Outcome> {
    if let Some(rest) = strip_any(s, lower, &["tâche ", "tache ", "task ", "todo "]) {
        let (text, due, label) = when(rest, now);
        if text.is_empty() {
            return None;
        }
        return Some(Outcome {
            preview: format!("Tâche : {text}{label}"),
            icon: "list-checks",
            action: Some(Action::AddTask { text, due }),
        });
    }
    if let Some(rest) = strip_any(s, lower, &["rappel ", "remind ", "reminder "]) {
        let (at, text) = reminder_time(rest, now)?;
        let when = Local.timestamp_opt(at, 0).single()?.format("%H:%M");
        return Some(Outcome { preview: format!("Rappel {when} · {text}"), icon: "bell", action: Some(Action::AddReminder { text, at }) });
    }
    if let Some(rest) = strip_any(s, lower, &["note "]) {
        return (!rest.is_empty()).then(|| Outcome { preview: format!("Note : {rest}"), icon: "notebook-pen", action: Some(Action::AddNote(rest.into())) });
    }
    None
}

/// Trailing "demain"/"tomorrow"/"aujourd'hui" → due date at 09:00 / end of today.
fn when(text: &str, now: DateTime<Local>) -> (String, Option<i64>, String) {
    let lower = text.to_lowercase();
    for (kw, days, label) in [(" demain", 1, " (demain)"), (" tomorrow", 1, " (demain)"), (" aujourd'hui", 0, " (aujourd'hui)"), (" today", 0, " (aujourd'hui)")] {
        if lower.ends_with(kw) {
            let t = text[..text.len() - kw.len()].trim().to_string();
            let day = now.date_naive() + CDuration::days(days);
            let due = Local.from_local_datetime(&day.and_hms_opt(9, 0, 0).unwrap()).earliest().map(|d| d.timestamp());
            return (t, due, label.into());
        }
    }
    (text.trim().to_string(), None, String::new())
}

fn reminder_time(rest: &str, now: DateTime<Local>) -> Option<(i64, String)> {
    let lower = rest.to_lowercase();
    // "dans 10 min texte" / "in 10 min text"
    if let Some(r) = lower.strip_prefix("dans ").or_else(|| lower.strip_prefix("in ")) {
        let mut parts = r.splitn(3, ' ');
        let n: i64 = parts.next()?.parse().ok()?;
        let unit = parts.next()?;
        let secs = match unit {
            "s" | "sec" | "secondes" | "seconds" => n,
            "min" | "mins" | "minutes" | "minute" => n * 60,
            "h" | "heure" | "heures" | "hour" | "hours" => n * 3600,
            _ => return None,
        };
        let offset = rest.len() - parts.clone().next().map(str::len).unwrap_or(0);
        let text = rest[offset..].trim().to_string();
        return Some((now.timestamp() + secs, if text.is_empty() { "Rappel".into() } else { text }));
    }
    // "15:00 texte" / "15h texte" / "15h30 texte"
    let (time, text) = rest.split_once(' ').unwrap_or((rest, ""));
    let t = time.replace('h', ":");
    let t = if t.ends_with(':') { format!("{t}00") } else { t };
    let nt = NaiveTime::parse_from_str(&t, "%H:%M").ok()?;
    let mut day = now.date_naive();
    if nt <= now.time() {
        day += CDuration::days(1);
    }
    let at = Local.from_local_datetime(&day.and_time(nt)).earliest()?.timestamp();
    Some((at, if text.trim().is_empty() { "Rappel".into() } else { text.trim().to_string() }))
}

fn open_module(s: &str) -> Option<Outcome> {
    let (id, label): (&'static str, &str) = match s {
        "pipette" | "couleur" | "color" | "picker" => ("picker", "Ouvrir la pipette"),
        "capture" | "screenshot" | "screen" => ("screenshot", "Capture d'écran"),
        "enregistrer" | "record" | "rec" => ("recorder", "Enregistrer l'écran"),
        "focus" | "pomodoro" | "concentration" => ("focus", "Démarrer un focus de 25 min"),
        "notes" => ("notes", "Ouvrir les notes"),
        "presse-papiers" | "clipboard" | "clip" => ("clipboard", "Ouvrir le presse-papiers"),
        "images" => ("images", "Ouvrir Images"),
        "musique" | "music" => ("music", "Ouvrir la musique"),
        "coffre" | "vault" => ("vault", "Ouvrir le coffre-fort"),
        "spotlight" => ("spotlight", "Spotlight"),
        "paramètres" | "parametres" | "settings" | "réglages" => ("settings", "Paramètres"),
        "thème" | "theme" => ("theme", "Basculer clair / sombre"),
        _ => return None,
    };
    Some(Outcome { preview: label.to_string(), icon: "app-window", action: Some(Action::Open(id)) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 11, 18, 14, 30, 0).unwrap()
    }

    fn pv(s: &str) -> String {
        eval(s, now()).map(|o| o.preview).unwrap_or_default()
    }

    #[test]
    fn math_and_percent() {
        assert_eq!(pv("12*(3+4)/2"), "= 42");
        assert_eq!(pv("2^10"), "= 1024");
        assert_eq!(pv("1,5 + 2"), "= 3,5");
        assert_eq!(pv("sqrt(16)+1"), "= 5");
        assert_eq!(pv("15% de 230"), "= 34,5");
        assert_eq!(pv("15% of 230"), "= 34,5");
        assert_eq!(pv("1/0"), "");
        assert_eq!(pv("bonjour"), "");
    }

    #[test]
    fn units_and_temperatures() {
        assert_eq!(pv("10 km en mi"), "= 6,2137 mi");
        assert_eq!(pv("5 kg to lb"), "= 11,0231 lb");
        assert_eq!(pv("72 f en c"), "= 22,22 °C");
        assert_eq!(pv("100 c en f"), "= 212 °F");
        assert_eq!(pv("10 km en kg"), "");
    }

    #[test]
    fn clocks_tasks_reminders() {
        assert!(pv("heure à Tokyo").starts_with("Tokyo : "));
        assert!(pv("time in new york").starts_with("New york : "));
        let o = eval("tâche Payer le loyer demain", now()).unwrap();
        assert!(matches!(o.action, Some(Action::AddTask { ref text, due: Some(_) }) if text == "Payer le loyer"));
        let o = eval("rappel 15:00 Réunion équipe", now()).unwrap();
        let Some(Action::AddReminder { at, text }) = o.action else { panic!() };
        assert_eq!(text, "Réunion équipe");
        assert_eq!(at - now().timestamp(), 30 * 60);
        let o = eval("rappel dans 10 min pause café", now()).unwrap();
        assert_eq!(o.action, Some(Action::AddReminder { text: "pause café".into(), at: now().timestamp() + 600 }));
        // A time already passed today is scheduled tomorrow.
        let Some(Action::AddReminder { at, .. }) = eval("rappel 9h stand-up", now()).unwrap().action else { panic!() };
        assert_eq!(at - now().timestamp(), (24 * 60 - 330) * 60);
        assert_eq!(eval("pipette", now()).unwrap().action, Some(Action::Open("picker")));
        assert_eq!(eval("note acheter du pain", now()).unwrap().action, Some(Action::AddNote("acheter du pain".into())));
    }
}
