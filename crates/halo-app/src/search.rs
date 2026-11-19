//! In-app search: find a feature or a setting from the hub sidebar (Ctrl+K) and the
//! launcher (Ctrl+Space). Accent- and case-insensitive word matching.

use crate::state::Section;

pub struct Entry {
    pub section: Section,
    pub fr: &'static str,
    pub en: &'static str,
    /// Extra words (FR + EN) that should find this entry.
    pub keywords: &'static str,
}

pub struct Hit {
    pub section: Section,
    pub label: String,
    /// Where it lives ("General › Notch").
    pub place: String,
    pub score: i32,
}

use Section::*;

pub const INDEX: &[Entry] = &[
    Entry { section: General, fr: "Position de l'encoche", en: "Notch position", keywords: "position ancre haut bas gauche droite centre anchor top bottom left right" },
    Entry { section: General, fr: "Thème clair / sombre", en: "Light / dark theme", keywords: "theme sombre clair dark light mode apparence" },
    Entry { section: General, fr: "Thème de l'encoche", en: "Notch theme", keywords: "minuit graphite ocean foret crepuscule violet couleur notch" },
    Entry { section: General, fr: "Réduire après inactivité", en: "Slim down when idle", keywords: "masquer auto hide inactivite estomper slim" },
    Entry { section: General, fr: "Indicateurs visibles", en: "Visible indicators", keywords: "indicateurs heure cpu batterie wifi bluetooth volume ram horloge" },
    Entry { section: General, fr: "Ouvrir au survol ou au clic", en: "Open on hover or click", keywords: "survol clic hover click expansion" },
    Entry { section: General, fr: "Clics traversants", en: "Click-through", keywords: "clic traversant transparent click through" },
    Entry { section: General, fr: "Langue", en: "Language", keywords: "langue francais anglais english language" },
    Entry { section: General, fr: "Jeu d'icônes", en: "Icon set", keywords: "icones lucide phosphor hugeicons icons" },
    Entry { section: Modules, fr: "Activer / désactiver des modules", en: "Enable / disable modules", keywords: "modules activer desactiver opt-in" },
    Entry { section: Productivity, fr: "Pomodoro / minuteur focus", en: "Pomodoro / focus timer", keywords: "pomodoro focus minuteur timer concentration productivite" },
    Entry { section: Productivity, fr: "Tâches du jour", en: "Today's tasks", keywords: "taches todo aujourd'hui rappels reminders" },
    Entry { section: Productivity, fr: "Distractions", en: "Distractions", keywords: "distraction discord jeux steam blocage rappel" },
    Entry { section: Notes, fr: "Notes", en: "Notes", keywords: "notes markdown ecrire bloc export" },
    Entry { section: Clipboard, fr: "Historique du presse-papiers", en: "Clipboard history", keywords: "presse-papiers clipboard copier coller historique" },
    Entry { section: Clipboard, fr: "Chiffrer le presse-papiers", en: "Encrypt clipboard", keywords: "chiffrer dpapi securite encrypt" },
    Entry { section: Accent, fr: "Caractères accentués", en: "Accented characters", keywords: "accent quick accent e accent ç touche espace" },
    Entry { section: System, fr: "Moniteurs système", en: "System monitors", keywords: "cpu ram batterie wifi bluetooth volume son" },
    Entry { section: System, fr: "Volume", en: "Volume", keywords: "volume son muet mute audio" },
    Entry { section: Resources, fr: "Ressources par processus", en: "Per-process resources", keywords: "processus gpu cpu historique graphique consommation task manager" },
    Entry { section: Activity, fr: "Suivi d'activité", en: "Activity tracking", keywords: "temps ecran activite applications statistiques screen time" },
    Entry { section: Picker, fr: "Pipette / color picker", en: "Color picker", keywords: "couleur pipette hex rgb hsl color picker" },
    Entry { section: Images, fr: "Redimensionner des images", en: "Resize images", keywords: "image redimensionner compresser webp jpeg png avif resize" },
    Entry { section: Spotlight, fr: "Spotlight souris", en: "Mouse spotlight", keywords: "souris curseur secouer trouver find mouse" },
    Entry { section: Music, fr: "Lecteur de musique", en: "Music player", keywords: "musique mp3 flac playlist lecteur music" },
    Entry { section: Music, fr: "Lire avec Halo (Explorateur)", en: "Play with Halo (Explorer)", keywords: "explorateur menu contextuel ouvrir avec explorer" },
    Entry { section: WinTheme, fr: "Thème Windows clair / sombre", en: "Windows light / dark", keywords: "windows theme sombre clair fond d'ecran wallpaper accent coucher soleil planning" },
    Entry { section: Notifications, fr: "Notifications Windows et Ne pas déranger", en: "Windows notifications and Do Not Disturb", keywords: "notifications ne pas deranger dnd silence nuit do not disturb focus" },
    Entry { section: Startup, fr: "Lancer au démarrage", en: "Launch at startup", keywords: "demarrage startup boot autostart windows session" },
];

/// Lowercase + strip French accents for matching.
pub fn fold(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| match c {
            'à' | 'â' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' => 'i',
            'ô' | 'ö' => 'o',
            'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            'œ' => 'o',
            _ => c,
        })
        .collect()
}

pub fn search(query: &str, fr: bool) -> Vec<Hit> {
    let words: Vec<String> = fold(query).split_whitespace().map(String::from).collect();
    if words.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<Hit> = INDEX
        .iter()
        .filter_map(|e| {
            let label = if fr { e.fr } else { e.en };
            let hay = fold(&format!("{} {} {}", e.fr, e.en, e.keywords));
            if !words.iter().all(|w| hay.contains(w.as_str())) {
                return None;
            }
            let l = fold(label);
            let score = words.iter().map(|w| if l.starts_with(w.as_str()) { 3 } else if l.contains(w.as_str()) { 2 } else { 1 }).sum();
            Some(Hit { section: e.section, label: label.to_string(), place: format!("{:?}", e.section), score })
        })
        .collect();
    hits.sort_by(|a, b| b.score.cmp(&a.score));
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_by_keyword_accent_insensitive() {
        assert_eq!(search("pomodoro", true)[0].section, Section::Productivity);
        assert_eq!(search("theme sombre", true)[0].section, Section::General);
        assert_eq!(search("Thème", true)[0].section, Section::General);
        assert_eq!(search("clipboard", false)[0].section, Section::Clipboard);
        assert!(search("zzzz", true).is_empty());
    }
}
