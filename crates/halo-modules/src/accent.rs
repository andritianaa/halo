//! M8 — Quick Accent: hold a letter, press the activation key, pick a variant.
//!
//! Privacy (spec §3.2): the hook only inspects the current key to drive this state
//! machine; nothing is stored or logged. The engine is pure and fully tested.

use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use halo_core::module::{Category, Manifest, Module, ModuleContext, Permission};
use serde::{Deserialize, Serialize};

pub const MANIFEST: Manifest = Manifest {
    id: "accent",
    cdc: "M8",
    name_key: "module.accent",
    category: Category::Productivity,
    icon: "languages",
    permissions: &[Permission::KeyboardHook],
    opt_in: false,
    default_enabled: true,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Activation {
    #[default]
    Space,
    Arrows,
    /// Space or Left/Right arrows.
    Both,
}

impl Activation {
    pub fn parse(s: &str) -> Self {
        match s {
            "arrows" => Activation::Arrows,
            "both" => Activation::Both,
            _ => Activation::Space,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CharSet {
    /// French first (é è ê à ç…), then other Latin variants.
    #[default]
    Fr,
    All,
}

const VK_BACK: u32 = 0x08;
const VK_SPACE: u32 = 0x20;
const VK_ESCAPE: u32 = 0x1B;
const VK_LEFT: u32 = 0x25;
const VK_RIGHT: u32 = 0x27;
const VK_SHIFT: [u32; 3] = [0x10, 0xA0, 0xA1];

/// Variants for a letter (lowercase). `None` if the letter has none.
pub fn variants(letter: char, set: CharSet) -> Option<&'static [char]> {
    let fr: &[char] = match letter {
        'a' => &['à', 'â', 'æ', 'ä'],
        'c' => &['ç'],
        'e' => &['é', 'è', 'ê', 'ë', '€'],
        'i' => &['î', 'ï'],
        'o' => &['ô', 'œ', 'ö'],
        'u' => &['ù', 'û', 'ü'],
        'y' => &['ÿ'],
        _ => &[],
    };
    let all: &[char] = match letter {
        'a' => &['à', 'â', 'æ', 'ä', 'á', 'ã', 'å', 'ā'],
        'c' => &['ç', 'ć', 'č'],
        'e' => &['é', 'è', 'ê', 'ë', '€', 'ē', 'ę'],
        'i' => &['î', 'ï', 'í', 'ì', 'ī'],
        'n' => &['ñ', 'ń'],
        'o' => &['ô', 'œ', 'ö', 'ó', 'ò', 'õ', 'ø'],
        's' => &['ß', 'š', 'ś'],
        'u' => &['ù', 'û', 'ü', 'ú', 'ū'],
        'y' => &['ÿ', 'ý'],
        'z' => &['ž', 'ź', 'ż'],
        _ => &[],
    };
    let v = match set {
        CharSet::Fr => fr,
        CharSet::All => all,
    };
    (!v.is_empty()).then_some(v)
}

fn upper(c: char) -> char {
    match c {
        'œ' => 'Œ',
        'æ' => 'Æ',
        'ß' | '€' => c,
        _ => c.to_uppercase().next().unwrap_or(c),
    }
}

/// What the UI should do after a key.
#[derive(Clone, Debug, PartialEq)]
pub enum UiEvent {
    Show { options: Vec<char>, index: usize },
    Select(usize),
    /// Hide the chooser; `commit` = character to type (after one Backspace).
    Hide { commit: Option<char> },
}

#[derive(Clone, Debug, Default)]
pub struct Engine {
    pub activation: Activation,
    pub set: CharSet,
    held: Option<u32>,
    /// When the held letter went down (activation only after delay).
    held_at: Option<std::time::Instant>,
    /// Minimum hold before the activation key opens the chooser (avoids pop-ups while typing fast).
    pub delay: std::time::Duration,
    shift: bool,
    active: bool,
    options: Vec<char>,
    index: usize,
}

impl Engine {
    pub fn new(activation: Activation, set: CharSet) -> Self {
        Self { activation, set, ..Self::default() }
    }

    fn letter(vk: u32) -> Option<char> {
        (0x41..=0x5A).contains(&vk).then(|| (b'a' + (vk - 0x41) as u8) as char)
    }

    fn is_activation(&self, vk: u32) -> bool {
        match self.activation {
            Activation::Space => vk == VK_SPACE,
            Activation::Arrows => vk == VK_LEFT || vk == VK_RIGHT,
            Activation::Both => vk == VK_SPACE || vk == VK_LEFT || vk == VK_RIGHT,
        }
    }

    /// Process one key. Returns (swallow, ui event).
    pub fn on_key(&mut self, vk: u32, down: bool, injected: bool) -> (bool, Option<UiEvent>) {
        self.on_key_at(vk, down, injected, std::time::Instant::now())
    }

    /// Same as [on_key] with an explicit clock (tests).
    pub fn on_key_at(&mut self, vk: u32, down: bool, injected: bool, now: std::time::Instant) -> (bool, Option<UiEvent>) {
        if injected {
            return (false, None);
        }
        if VK_SHIFT.contains(&vk) {
            self.shift = down;
            return (false, None);
        }
        let letter = Self::letter(vk);

        if down {
            if self.held == Some(vk) {
                return (true, None); // swallow auto-repeat of the held letter
            }
            if self.active {
                if self.is_activation(vk) {
                    let n = self.options.len();
                    self.index = if vk == VK_LEFT { (self.index + n - 1) % n } else { (self.index + 1) % n };
                    return (true, Some(UiEvent::Select(self.index)));
                }
                if vk == VK_ESCAPE {
                    self.reset();
                    return (true, Some(UiEvent::Hide { commit: None }));
                }
                return (true, None); // other keys are ignored while choosing
            }
            if let Some(l) = letter {
                self.held = variants(l, self.set).map(|_| vk);
                self.held_at = self.held.map(|_| now);
                return (false, None); // the letter itself is typed normally
            }
            let held_long_enough = self.held_at.is_some_and(|t| now.duration_since(t) >= self.delay);
            if self.is_activation(vk) && held_long_enough {
                if let Some(l) = self.held.and_then(Self::letter) {
                    let base = variants(l, self.set).unwrap_or(&[]);
                    self.options = if self.shift { base.iter().map(|&c| upper(c)).collect() } else { base.to_vec() };
                    self.active = true;
                    self.index = if vk == VK_LEFT { self.options.len() - 1 } else { 0 };
                    return (true, Some(UiEvent::Show { options: self.options.clone(), index: self.index }));
                }
            }
            return (false, None);
        }

        // Key up.
        if self.held == Some(vk) {
            let commit = self.active.then(|| self.options[self.index]);
            let was_active = self.active;
            self.reset();
            return if was_active { (true, Some(UiEvent::Hide { commit })) } else { (false, None) };
        }
        if self.active && self.is_activation(vk) {
            return (true, None);
        }
        (false, None)
    }

    fn reset(&mut self) {
        self.held = None;
        self.held_at = None;
        self.active = false;
        self.options.clear();
        self.index = 0;
    }
}

/// Chooser state shown by the notch.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Chooser {
    pub visible: bool,
    pub options: Vec<char>,
    pub index: usize,
}

#[derive(Clone, Default)]
pub struct AccentHandle(Arc<Mutex<Chooser>>);

impl AccentHandle {
    pub fn get(&self) -> Chooser {
        self.0.lock().map(|c| c.clone()).unwrap_or_default()
    }
}

pub struct QuickAccent {
    handle: AccentHandle,
    hook: Option<halo_platform::hooks::HookThread>,
    pub activation: Activation,
    pub set: CharSet,
    pub delay: std::time::Duration,
    /// Test seam: receives committed characters instead of SendInput.
    pub sink: Option<Sender<char>>,
}

impl QuickAccent {
    pub fn new(activation: Activation, set: CharSet) -> Self {
        Self { handle: AccentHandle::default(), hook: None, activation, set, delay: std::time::Duration::from_millis(300), sink: None }
    }

    pub fn handle(&self) -> AccentHandle {
        self.handle.clone()
    }
}

impl Module for QuickAccent {
    fn manifest(&self) -> &Manifest {
        &MANIFEST
    }

    fn start(&mut self, _: &ModuleContext) -> anyhow::Result<()> {
        let mut engine = Engine::new(self.activation, self.set);
        engine.delay = self.delay;
        let chooser = self.handle.0.clone();
        let sink = self.sink.clone();
        self.hook = Some(halo_platform::hooks::keyboard(Box::new(move |ev| {
            let (swallow, ui) = engine.on_key(ev.vk, ev.down, ev.injected);
            if let Some(ui) = ui {
                if let Ok(mut c) = chooser.lock() {
                    match &ui {
                        UiEvent::Show { options, index } => {
                            *c = Chooser { visible: true, options: options.clone(), index: *index }
                        }
                        UiEvent::Select(i) => c.index = *i,
                        UiEvent::Hide { .. } => *c = Chooser::default(),
                    }
                }
                if let UiEvent::Hide { commit: Some(ch) } = ui {
                    match &sink {
                        Some(tx) => {
                            let _ = tx.send(ch);
                        }
                        // Type outside the hook callback to keep it fast.
                        None => {
                            std::thread::spawn(move || {
                                halo_platform::input::tap(VK_BACK as u16);
                                halo_platform::input::type_text(&ch.to_string());
                            });
                        }
                    }
                }
            }
            swallow
        })));
        Ok(())
    }

    fn configure(&mut self, config: &halo_core::config::Config) -> bool {
        let s = &config.settings;
        let activation = Activation::parse(&s.accent_activation);
        let set = if s.accent_all_languages { CharSet::All } else { CharSet::Fr };
        let delay = std::time::Duration::from_millis(s.accent_delay_ms as u64);
        let changed = (activation, set, delay) != (self.activation, self.set, self.delay);
        self.activation = activation;
        self.set = set;
        self.delay = delay;
        changed
    }

    fn stop(&mut self) {
        self.hook = None;
        if let Ok(mut c) = self.handle.0.lock() {
            *c = Chooser::default();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const E: u32 = 0x45;
    const A: u32 = 0x41;
    const K: u32 = 0x4B;

    #[test]
    fn hold_e_space_space_release_commits_second_variant() {
        let mut en = Engine::new(Activation::Space, CharSet::Fr);
        assert_eq!(en.on_key(E, true, false), (false, None)); // 'e' typed
        assert_eq!(en.on_key(E, true, false).0, true); // repeat swallowed
        let (sw, ui) = en.on_key(VK_SPACE, true, false);
        assert!(sw);
        assert_eq!(ui, Some(UiEvent::Show { options: vec!['é', 'è', 'ê', 'ë', '€'], index: 0 }));
        assert!(en.on_key(VK_SPACE, false, false).0);
        assert_eq!(en.on_key(VK_SPACE, true, false), (true, Some(UiEvent::Select(1))));
        assert_eq!(en.on_key(E, false, false), (true, Some(UiEvent::Hide { commit: Some('è') })));
    }

    #[test]
    fn shift_gives_uppercase_and_escape_cancels() {
        let mut en = Engine::new(Activation::Space, CharSet::Fr);
        en.on_key(0xA0, true, false);
        en.on_key(A, true, false);
        let (_, ui) = en.on_key(VK_SPACE, true, false);
        assert_eq!(ui, Some(UiEvent::Show { options: vec!['À', 'Â', 'Æ', 'Ä'], index: 0 }));
        assert_eq!(en.on_key(VK_ESCAPE, true, false), (true, Some(UiEvent::Hide { commit: None })));
        // Releasing the letter afterwards is a normal key-up.
        assert_eq!(en.on_key(A, false, false), (false, None));
    }

    #[test]
    fn letters_without_variants_and_plain_space_pass_through() {
        let mut en = Engine::new(Activation::Space, CharSet::Fr);
        en.on_key(K, true, false);
        assert_eq!(en.on_key(VK_SPACE, true, false), (false, None));
        en.on_key(K, false, false);
        assert_eq!(en.on_key(VK_SPACE, true, false), (false, None));
    }

    #[test]
    fn arrows_activation_wraps_backwards() {
        let mut en = Engine::new(Activation::Arrows, CharSet::Fr);
        en.on_key(E, true, false);
        let (_, ui) = en.on_key(VK_LEFT, true, false);
        assert_eq!(ui, Some(UiEvent::Show { options: vec!['é', 'è', 'ê', 'ë', '€'], index: 4 }));
        assert_eq!(en.on_key(VK_RIGHT, true, false), (true, Some(UiEvent::Select(0))));
    }

    #[test]
    fn fast_typing_does_not_open_the_chooser() {
        let t0 = std::time::Instant::now();
        let ms = |n| t0 + std::time::Duration::from_millis(n);
        let mut en = Engine::new(Activation::Space, CharSet::Fr);
        en.delay = std::time::Duration::from_millis(300);
        en.on_key_at(E, true, false, ms(0));
        // Space 80 ms later (fast typing "e "): passes through, no chooser.
        assert_eq!(en.on_key_at(VK_SPACE, true, false, ms(80)), (false, None));
        en.on_key_at(VK_SPACE, false, false, ms(90));
        // Holding long enough opens it.
        let (sw, ui) = en.on_key_at(VK_SPACE, true, false, ms(400));
        assert!(sw && matches!(ui, Some(UiEvent::Show { .. })));
    }

    #[test]
    fn both_accepts_space_and_arrows() {
        let mut en = Engine::new(Activation::Both, CharSet::Fr);
        en.on_key(E, true, false);
        assert!(matches!(en.on_key(VK_RIGHT, true, false).1, Some(UiEvent::Show { .. })));
        assert_eq!(en.on_key(VK_SPACE, true, false), (true, Some(UiEvent::Select(1))));
    }

    #[test]
    fn injected_keys_are_ignored() {
        let mut en = Engine::new(Activation::Space, CharSet::Fr);
        en.on_key(E, true, true);
        assert_eq!(en.on_key(VK_SPACE, true, false), (false, None));
    }
}
