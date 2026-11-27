//! web/src/i18n.ts: English text is the key; in Portuguese each one is looked up in the table generated from it.

mod table {
    include!("i18n_table.rs");
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Language {
    #[default]
    En,
    Pt,
}

impl Language {
    /// Portuguese for any pt locale, English otherwise.
    pub fn from_locale(value: &str) -> Self {
        if value.to_ascii_lowercase().starts_with("pt") { Self::Pt } else { Self::En }
    }

    /// The system's language, from the usual POSIX variables (the Windows UI language in the integration phase).
    pub fn system() -> Self {
        ["LC_ALL", "LC_MESSAGES", "LANG", "LANGUAGE"]
            .iter()
            .filter_map(|name| std::env::var(name).ok())
            .find(|value| !value.is_empty() && value != "C" && value != "POSIX")
            .map_or(Self::En, |value| Self::from_locale(&value))
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Pt => "pt",
        }
    }
}

/// The text in the chosen language.
pub fn t(language: Language, text: &str) -> &str {
    match language {
        Language::En => text,
        Language::Pt => lookup(text).unwrap_or(text),
    }
}

fn lookup(text: &str) -> Option<&'static str> {
    table::PT.binary_search_by(|(key, _)| (*key).cmp(text)).ok().map(|index| table::PT[index].1)
}

/// `t` with `{name}` placeholders filled in.
pub fn tf(language: Language, text: &str, values: &[(&str, &str)]) -> String {
    fill(t(language, text), values)
}

pub fn fill(text: &str, values: &[(&str, &str)]) -> String {
    values.iter().fold(text.to_owned(), |result, (name, value)| result.replace(&format!("{{{name}}}"), value))
}

/// A message from the capture service or AlbionLab, translated when it is one the meter knows.
pub fn message(language: Language, text: &str) -> String {
    if language == Language::En || lookup(text).is_some() {
        return t(language, text).to_owned();
    }
    if let Some(reason) = text.strip_prefix("AlbionLab is unreachable: ") {
        return tf(language, "AlbionLab is unreachable: {reason}", &[("reason", reason)]);
    }
    let code = |prefix: &str, suffix: &str| {
        text.strip_prefix(prefix)
            .and_then(|rest| rest.strip_suffix(suffix))
            .filter(|code| !code.is_empty() && code.bytes().all(|byte| byte.is_ascii_digit()))
    };
    if let Some(code) = code("AlbionLab did not save the change (", ").") {
        return tf(language, "AlbionLab did not save the change ({code}).", &[("code", code)]);
    }
    if let Some(code) = code("AlbionLab answered ", ".") {
        return tf(language, "AlbionLab answered {code}.", &[("code", code)]);
    }
    text.to_owned()
}

/// CSS `text-transform: capitalize`.
pub fn capitalize(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut start = true;
    for character in text.chars() {
        if start {
            result.extend(character.to_uppercase());
        } else {
            result.push(character);
        }
        start = character.is_whitespace() || character == '-';
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_sorted_and_complete() {
        assert!(table::PT.windows(2).all(|pair| pair[0].0 < pair[1].0));
        // The reference table, when the repository still carries it, must match the generated one entry for entry.
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../web/src/i18n.ts");
        if let Ok(source) = std::fs::read_to_string(source) {
            let start = source.find("const pt: Record<string, string> = {").unwrap();
            let end = start + source[start..].find("\n};").unwrap();
            let lines = source[start..end].lines().filter(|line| line.trim_start().starts_with(['\'', '"'])).count();
            assert_eq!(lines, table::PT.len());
        }
    }

    #[test]
    fn translates_and_fills() {
        assert_eq!(t(Language::Pt, "End session"), "Encerrar sessão");
        assert_eq!(t(Language::En, "End session"), "End session");
        assert_eq!(t(Language::Pt, "Not in the table"), "Not in the table");
        assert_eq!(t(Language::Pt, "Hide other players' names"), "Esconder o nome dos outros jogadores");
        assert_eq!(
            tf(Language::Pt, "{total} total · {rate} party {unit}", &[("total", "10 mi"), ("rate", "2.847"), ("unit", "DPS")]),
            "10 mi no total · 2.847 DPS da party"
        );
    }

    #[test]
    fn translates_server_messages() {
        assert_eq!(message(Language::Pt, "AlbionLab answered 503."), "O AlbionLab respondeu 503.");
        assert_eq!(message(Language::Pt, "AlbionLab is unreachable: timeout"), "Não foi possível falar com o AlbionLab: timeout");
        assert_eq!(message(Language::Pt, "Something else"), "Something else");
        assert_eq!(message(Language::En, "AlbionLab answered 503."), "AlbionLab answered 503.");
    }

    #[test]
    fn capitalizes_like_css() {
        assert_eq!(capitalize("live-capture"), "Live-Capture");
        assert_eq!(capitalize("ao vivo"), "Ao Vivo");
    }
}
