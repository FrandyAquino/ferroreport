//! Locale data needed by the number and date formatters (a small CLDR subset).

/// Symbols and names for one locale.
#[derive(Debug, Clone)]
pub struct Locale {
    pub tag: &'static str,
    pub decimal_separator: char,
    pub grouping_separator: char,
    pub currency_symbol: &'static str,
    pub months: [&'static str; 12],
    pub short_months: [&'static str; 12],
    /// Sunday first, like `java.util.Calendar`.
    pub weekdays: [&'static str; 7],
    pub short_weekdays: [&'static str; 7],
    pub am_pm: [&'static str; 2],
}

const EN_MONTHS: [&str; 12] =
    ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const EN_SHORT_MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const EN_WEEKDAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const EN_SHORT_WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

const ES_MONTHS: [&str; 12] =
    ["enero", "febrero", "marzo", "abril", "mayo", "junio", "julio", "agosto", "septiembre", "octubre", "noviembre", "diciembre"];
const ES_SHORT_MONTHS: [&str; 12] = ["ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sept", "oct", "nov", "dic"];
const ES_WEEKDAYS: [&str; 7] = ["domingo", "lunes", "martes", "miércoles", "jueves", "viernes", "sábado"];
const ES_SHORT_WEEKDAYS: [&str; 7] = ["dom", "lun", "mar", "mié", "jue", "vie", "sáb"];

pub const EN_US: Locale = Locale {
    tag: "en_US",
    decimal_separator: '.',
    grouping_separator: ',',
    currency_symbol: "$",
    months: EN_MONTHS,
    short_months: EN_SHORT_MONTHS,
    weekdays: EN_WEEKDAYS,
    short_weekdays: EN_SHORT_WEEKDAYS,
    am_pm: ["AM", "PM"],
};

/// Spanish with the Latin-American number symbols used in DO, MX and US.
pub const ES_DO: Locale = Locale {
    tag: "es_DO",
    decimal_separator: '.',
    grouping_separator: ',',
    currency_symbol: "RD$",
    months: ES_MONTHS,
    short_months: ES_SHORT_MONTHS,
    weekdays: ES_WEEKDAYS,
    short_weekdays: ES_SHORT_WEEKDAYS,
    am_pm: ["a.\u{202F}m.", "p.\u{202F}m."], // Java 21 (CLDR) uses a narrow no-break space
};

pub const ES_MX: Locale = Locale { tag: "es_MX", currency_symbol: "$", ..ES_DO };

/// Spanish (Spain): comma decimal separator, dot grouping.
pub const ES_ES: Locale =
    Locale { tag: "es_ES", decimal_separator: ',', grouping_separator: '.', currency_symbol: "€", ..ES_DO };

impl Locale {
    /// Resolves a Java-style locale tag (`en_US`, `es-DO`, `es`). Unknown tags fall back to `en_US`.
    pub fn from_tag(tag: &str) -> &'static Locale {
        let norm = tag.replace('-', "_");
        match norm.as_str() {
            "es_DO" => &ES_DO,
            "es_MX" | "es_US" | "es_419" => &ES_MX,
            "es" | "es_ES" => &ES_ES,
            _ if norm.starts_with("es_") => &ES_DO,
            _ => &EN_US,
        }
    }
}

/// The language and country of a Java locale tag (`es_DO`, `es-DO`, `en`, `es_419`), or None
/// when `tag` is not one.
pub(crate) fn split_tag(tag: &str) -> Option<(String, String)> {
    let mut parts = tag.split(['_', '-']);
    let language = parts.next()?;
    let country = parts.next().unwrap_or("");
    let language_ok = (2..=3).contains(&language.len()) && language.chars().all(|c| c.is_ascii_alphabetic());
    let country_ok = country.is_empty()
        || (country.len() == 2 && country.chars().all(|c| c.is_ascii_alphabetic()))
        || (country.len() == 3 && country.chars().all(|c| c.is_ascii_digit()));
    (language_ok && country_ok && parts.next().is_none()).then(|| (language.to_ascii_lowercase(), country.to_ascii_uppercase()))
}

/// `Locale.getDisplayLanguage(in)`: the name of `tag`'s language in the language of `display`
/// (English, Spanish, or the language itself; English for others).
pub(crate) fn display_language(tag: &str, display: &str) -> String {
    let Some((language, _)) = split_tag(tag) else { return String::new() };
    let shown = split_tag(display).map(|d| d.0).unwrap_or_default();
    match super::locale_names::LANGUAGES.iter().find(|l| l.0 == language) {
        Some(l) if shown == language => l.3.to_string(),
        Some(l) if shown == "es" => l.2.to_string(),
        Some(l) => l.1.to_string(),
        None => language,
    }
}

/// `Locale.getDisplayCountry(in)`: the name of `tag`'s country in the language of `display`.
pub(crate) fn display_country(tag: &str, display: &str) -> String {
    let Some((language, country)) = split_tag(tag) else { return String::new() };
    if country.is_empty() {
        return country;
    }
    let shown = split_tag(display).map(|d| d.0).unwrap_or_default();
    if shown == language {
        let own = format!("{language}_{country}");
        if let Some(c) = super::locale_names::OWN_COUNTRIES.iter().find(|c| c.0 == own) {
            return c.1.to_string();
        }
    }
    match super::locale_names::COUNTRIES.iter().find(|c| c.0 == country) {
        Some(c) if shown == "es" => c.2.to_string(),
        Some(c) => c.1.to_string(),
        None => country,
    }
}

/// `Locale.getDisplayName(in)`: `language (country)`, as Java writes it.
pub(crate) fn display_name(tag: &str, display: &str) -> String {
    let language = display_language(tag, display);
    let country = display_country(tag, display);
    match (language.is_empty(), country.is_empty()) {
        (_, true) => language,
        (true, false) => country,
        (false, false) => format!("{language} ({country})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_names_are_javas() {
        // Locale.getDisplayName on Java 21, for the locales of JasperReports' i18n sample.
        let cases = [
            ("en_US", "en_US", "English (United States)"),
            ("en_US", "es_DO", "inglés (Estados Unidos)"),
            ("es_DO", "en_US", "Spanish (Dominican Republic)"),
            ("es_DO", "es_DO", "español (República Dominicana)"),
            ("cs_CZ", "cs_CZ", "čeština (Česko)"),
            ("cs_CZ", "en", "Czech (Czechia)"),
            ("de_DE", "de_DE", "Deutsch (Deutschland)"),
            ("de_AT", "es", "alemán (Austria)"),
            ("fr_FR", "fr_FR", "français (France)"),
            ("pt_PT", "pt_PT", "português (Portugal)"),
            ("ro_RO", "ro_RO", "română (România)"),
            ("sk_SK", "sk_SK", "slovenčina (Slovensko)"),
            ("zh_TW", "zh_TW", "中文 (台灣)"),
            ("es-419", "es", "español (Latinoamérica)"),
            ("en", "en_US", "English"),
            ("xx_YY", "en_US", "xx (YY)"),
        ];
        for (tag, display, want) in cases {
            assert_eq!(display_name(tag, display), want, "{tag} in {display}");
        }
        assert_eq!(display_language("de_DE", "en_US"), "German");
        assert_eq!(display_country("de_DE", "es"), "Alemania");
        assert_eq!(display_country("en", "en"), "");
        assert_eq!(split_tag("hello world"), None);
    }
}
