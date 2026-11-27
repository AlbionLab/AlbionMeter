//! The number and time formats of web/src/types.ts (Intl in en / pt-BR).

use crate::i18n::Language;

fn separators(language: Language) -> (char, char) {
    match language {
        Language::En => (',', '.'),
        Language::Pt => ('.', ','),
    }
}

fn group(integer: u64, separator: char) -> String {
    let digits = integer.to_string();
    let mut result = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            result.push(separator);
        }
        result.push(digit);
    }
    result
}

/// `value` with `digits` decimals (half away from zero, like Intl), grouped or not.
pub fn fixed(language: Language, value: f64, digits: usize, grouping: bool) -> String {
    let (thousands, decimal) = separators(language);
    let scale = 10f64.powi(digits as i32);
    let scaled = (value.abs() * scale).round();
    let integer = (scaled / scale).trunc() as u64;
    let fraction = (scaled - integer as f64 * scale).round() as u64;
    let mut result = String::new();
    if value < 0.0 && scaled > 0.0 {
        result.push('-');
    }
    result.push_str(&if grouping { group(integer, thousands) } else { integer.to_string() });
    if digits > 0 {
        result.push(decimal);
        result.push_str(&format!("{fraction:0digits$}"));
    }
    result
}

/// Intl.NumberFormat({ maximumFractionDigits: 0 }).
pub fn precise(language: Language, value: f64) -> String {
    fixed(language, value, 0, true)
}

/// At most `digits` decimals, trailing zeros dropped.
fn trimmed(language: Language, value: f64, digits: usize) -> String {
    let text = fixed(language, value, digits, true);
    let decimal = separators(language).1;
    if digits == 0 || !text.contains(decimal) {
        return text;
    }
    text.trim_end_matches('0').trim_end_matches(decimal).to_owned()
}

/// Intl.NumberFormat({ notation: 'compact', maximumFractionDigits: 1 }): 837.5K / 1.3M in English, 837,5 mil / 1,3 mi in
/// Portuguese.
pub fn compact(language: Language, value: f64) -> String {
    const EN: [&str; 5] = ["", "K", "M", "B", "T"];
    const PT: [&str; 5] = ["", " mil", " mi", " bi", " tri"];
    let mut exponent = 0;
    let mut scaled = value.abs();
    while scaled >= 1000.0 && exponent < 4 {
        scaled /= 1000.0;
        exponent += 1;
    }
    // 999,960 rounds to 1000K, which Intl writes as 1M.
    if (scaled * 10.0).round() / 10.0 >= 1000.0 && exponent < 4 {
        scaled /= 1000.0;
        exponent += 1;
    }
    let sign = if value < 0.0 { "-" } else { "" };
    let suffix = match language {
        Language::En => EN[exponent],
        Language::Pt => PT[exponent],
    };
    format!("{sign}{}{suffix}", trimmed(language, scaled, 1))
}

/// mm:ss, or h:mm:ss from an hour on.
pub fn duration(seconds: f64) -> String {
    let value = seconds.max(0.0).floor() as u64;
    let hours = value / 3600;
    if hours > 0 { format!("{hours}:{:02}:{:02}", value / 60 % 60, value % 60) } else { format!("{:02}:{:02}", value / 60, value % 60) }
}

/// A share with one decimal and the percent sign, in the language's format.
pub fn percent(language: Language, value: f64) -> String {
    format!("{}%", fixed(language, value, 1, true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use Language::{En, Pt};

    #[test]
    fn precise_groups_thousands() {
        assert_eq!(precise(En, 2847.4), "2,847");
        assert_eq!(precise(Pt, 2847.5), "2.848");
        assert_eq!(precise(En, 1_234_567.0), "1,234,567");
        assert_eq!(precise(En, 0.0), "0");
        assert_eq!(precise(En, 999.0), "999");
    }

    #[test]
    fn compact_matches_intl() {
        assert_eq!(compact(En, 10_000_000.0), "10M");
        assert_eq!(compact(En, 1_300_000.0), "1.3M");
        assert_eq!(compact(En, 837_500.0), "837.5K");
        assert_eq!(compact(En, 8_700.0), "8.7K");
        assert_eq!(compact(En, 999_960.0), "1M");
        assert_eq!(compact(En, 837.46), "837.5");
        assert_eq!(compact(En, 0.0), "0");
        assert_eq!(compact(Pt, 1_300_000.0), "1,3 mi");
        assert_eq!(compact(Pt, 837_500.0), "837,5 mil");
        assert_eq!(compact(Pt, 2_000_000_000.0), "2 bi");
    }

    #[test]
    fn durations() {
        assert_eq!(duration(0.0), "00:00");
        assert_eq!(duration(3518.9), "58:38");
        assert_eq!(duration(3725.0), "1:02:05");
        assert_eq!(duration(-3.0), "00:00");
    }

    #[test]
    fn fixed_decimals() {
        assert_eq!(fixed(Pt, 14.4, 2, false), "14,40");
        assert_eq!(fixed(Pt, 1234.5, 2, false), "1234,50");
        assert_eq!(percent(En, 13.04), "13.0%");
        assert_eq!(percent(Pt, 0.05), "0,1%");
    }
}
