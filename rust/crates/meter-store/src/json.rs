//! JSON written the way System.Text.Json writes it (JsonSerializerDefaults.Web with the default encoder), so files and
//! rows written by the Rust meter are byte-identical to the C# ones: non-ASCII and HTML-sensitive characters escaped as
//! `\uXXXX`, doubles in .NET's shortest round-trip form (`5` not `5.0`, `1E+15` not `1e15`).

use serde::Serialize;
use serde_json::ser::{CharEscape, CompactFormatter, Formatter, PrettyFormatter};
use std::io::{self, Write};

pub struct CsFormatter<F> {
    inner: F,
    raw_dates: bool,
    /// With `raw_dates`, the string being written, held until its end shows whether it is a date: a typed date
    /// arrives in pieces (`collect_str`).
    string: String,
    holding: bool,
}

impl<F> CsFormatter<F> {
    fn new(inner: F, raw_dates: bool) -> Self {
        Self { inner, raw_dates, string: String::new(), holding: false }
    }
}

/// `JsonSerializer.Serialize` of a typed value. Its `DateTimeOffset` properties are written unescaped
/// (`+00:00`), unlike strings, so a string that is a date is written that way too.
pub fn to_string<T: Serialize + ?Sized>(value: &T) -> serde_json::Result<String> {
    write(value, CsFormatter::new(CompactFormatter, true))
}

/// `JsonNode.ToJsonString()`: every string escaped alike, dates included (`\u002B00:00`), as C# writes a snapshot
/// it rewrote as a JSON tree (identity merges and labels).
pub fn to_string_node<T: Serialize + ?Sized>(value: &T) -> serde_json::Result<String> {
    write(value, CsFormatter::new(CompactFormatter, false))
}

/// `WriteIndented = true`: two spaces, as capture-settings.json.
pub fn to_string_indented<T: Serialize + ?Sized>(value: &T) -> serde_json::Result<String> {
    write(value, CsFormatter::new(PrettyFormatter::with_indent(b"  "), true))
}

/// [`to_string`] into a buffer of `capacity` bytes: given the right size, a large snapshot is written without the
/// buffer doubling (and briefly holding the old and the new copy).
pub fn to_string_with_capacity<T: Serialize + ?Sized>(value: &T, capacity: usize) -> serde_json::Result<String> {
    write_into(value, CsFormatter::new(CompactFormatter, true), capacity)
}

fn write<T: Serialize + ?Sized, F: Formatter>(value: &T, formatter: CsFormatter<F>) -> serde_json::Result<String> {
    write_into(value, formatter, 128)
}

fn write_into<T: Serialize + ?Sized, F: Formatter>(value: &T, formatter: CsFormatter<F>, capacity: usize) -> serde_json::Result<String> {
    let mut out = Vec::with_capacity(capacity);
    let mut serializer = serde_json::Serializer::with_formatter(&mut out, formatter);
    value.serialize(&mut serializer)?;
    Ok(String::from_utf8(out).expect("serde_json writes UTF-8"))
}

/// .NET Core 3.0+ `double.ToString("R")`: shortest round-trip digits, scientific when the decimal point is more than
/// 15 places right of the first digit or more than 3 left of it.
pub fn format_double(value: f64) -> String {
    if value == 0.0 {
        return if value.is_sign_negative() { "-0".into() } else { "0".into() };
    }
    // `{:e}` gives the shortest digit count but breaks an exact decimal tie at the last digit upwards; formatting
    // exactly to that many digits rounds half to even, as .NET does (161806.428955078125 → …812, not …813).
    let shortest = format!("{value:e}");
    let count = shortest.split_once('e').expect("{:e} has an exponent").0.bytes().filter(u8::is_ascii_digit).count();
    let scientific = format!("{value:.precision$e}", precision = count - 1);
    let (mantissa, exponent) = scientific.split_once('e').expect("{:e} has an exponent");
    let exponent: i32 = exponent.parse().expect("integer exponent");
    let (sign, mantissa) = mantissa.strip_prefix('-').map_or(("", mantissa), |rest| ("-", rest));
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let scale = exponent + 1;
    if !(-3..=15).contains(&scale) {
        let rest = &digits[1..];
        let dot = if rest.is_empty() { String::new() } else { format!(".{rest}") };
        let sign_e = if exponent < 0 { '-' } else { '+' };
        return format!("{sign}{}{dot}E{sign_e}{:02}", &digits[..1], exponent.abs());
    }
    if scale <= 0 {
        format!("{sign}0.{}{digits}", "0".repeat((-scale) as usize))
    } else if scale as usize >= digits.len() {
        format!("{sign}{digits}{}", "0".repeat(scale as usize - digits.len()))
    } else {
        let (whole, fraction) = digits.split_at(scale as usize);
        format!("{sign}{whole}.{fraction}")
    }
}

fn escape_fragment<W: ?Sized + Write>(writer: &mut W, fragment: &str) -> io::Result<()> {
    let mut start = 0;
    for (index, character) in fragment.char_indices() {
        let plain = character.is_ascii() && !matches!(character, '&' | '\'' | '+' | '<' | '>' | '`' | '\x7f');
        if plain {
            continue;
        }
        writer.write_all(&fragment.as_bytes()[start..index])?;
        let mut units = [0u16; 2];
        for unit in character.encode_utf16(&mut units) {
            write!(writer, "\\u{unit:04X}")?;
        }
        start = index + character.len_utf8();
    }
    writer.write_all(&fragment.as_bytes()[start..])
}

macro_rules! delegate {
    ($($name:ident($($arg:ident: $ty:ty),*)),* $(,)?) => {
        $(fn $name<W: ?Sized + Write>(&mut self, writer: &mut W $(, $arg: $ty)*) -> io::Result<()> { self.inner.$name(writer $(, $arg)*) })*
    };
}

impl<F: Formatter> Formatter for CsFormatter<F> {
    delegate!(
        begin_array(), end_array(), begin_array_value(first: bool), end_array_value(),
        begin_object(), end_object(), begin_object_key(first: bool), end_object_key(),
        begin_object_value(), end_object_value(),
    );

    fn write_f64<W: ?Sized + Write>(&mut self, writer: &mut W, value: f64) -> io::Result<()> {
        writer.write_all(format_double(value).as_bytes())
    }

    fn write_f32<W: ?Sized + Write>(&mut self, writer: &mut W, value: f32) -> io::Result<()> {
        self.write_f64(writer, value as f64)
    }

    fn begin_string<W: ?Sized + Write>(&mut self, writer: &mut W) -> io::Result<()> {
        if self.raw_dates {
            self.string.clear();
            self.holding = true;
        }
        writer.write_all(b"\"")
    }

    fn end_string<W: ?Sized + Write>(&mut self, writer: &mut W) -> io::Result<()> {
        if std::mem::take(&mut self.holding) {
            let date = self.string.len() >= 25 && self.string.is_ascii() && crate::time::DateTimeOffset::parse(&self.string).is_some();
            if date { writer.write_all(self.string.as_bytes())? } else { escape_fragment(writer, &self.string)? }
        }
        writer.write_all(b"\"")
    }

    fn write_string_fragment<W: ?Sized + Write>(&mut self, writer: &mut W, fragment: &str) -> io::Result<()> {
        if self.holding {
            self.string.push_str(fragment);
            return Ok(());
        }
        escape_fragment(writer, fragment)
    }

    fn write_char_escape<W: ?Sized + Write>(&mut self, writer: &mut W, escape: CharEscape) -> io::Result<()> {
        // A string with a quote, backslash or control character is no date.
        if std::mem::take(&mut self.holding) {
            escape_fragment(writer, &self.string)?;
        }
        let text: &[u8] = match escape {
            CharEscape::Quote => b"\\u0022",
            CharEscape::ReverseSolidus => b"\\\\",
            CharEscape::Solidus => b"/",
            CharEscape::Backspace => b"\\b",
            CharEscape::FormFeed => b"\\f",
            CharEscape::LineFeed => b"\\n",
            CharEscape::CarriageReturn => b"\\r",
            CharEscape::Tab => b"\\t",
            CharEscape::AsciiControl(byte) => return write!(writer, "\\u{byte:04X}"),
        };
        writer.write_all(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::excessive_precision)] // 161806.428955078125 is the double's exact value, written out on purpose
    fn doubles_match_dotnet() {
        for (value, text) in [
            (0.0, "0"), (5.0, "5"), (964539.0, "964539"), (15.881, "15.881"), (42.89914972010524, "42.89914972010524"),
            (1e14, "100000000000000"), (1e15, "1E+15"), (1.5e15, "1.5E+15"), (0.0001, "0.0001"), (0.00001, "1E-05"),
            (-2.5, "-2.5"), (123456789012345.6, "123456789012345.6"), (1e-7, "1E-07"), (0.1, "0.1"),
            // Exact decimal ties at the shortest digit count round to even.
            (161806.428955078125, "161806.42895507812"), (0.5, "0.5"), (2.5, "2.5"),
        ] {
            assert_eq!(format_double(value), text, "{value}");
        }
    }

    #[test]
    fn doubles_round_trip_and_ties_go_to_even() {
        // Every value still parses back to itself, and wherever the shortest form ends on a tie the last digit is even.
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut ties = 0;
        for _ in 0..200_000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            // Values with few binary fraction digits are where exact decimal ties happen.
            let value = (state % 1_000_000_000) as f64 / [1.0, 8.0, 1024.0, 65536.0, 1048576.0][(state >> 40) as usize % 5];
            let text = format_double(value);
            assert_eq!(text.parse::<f64>().unwrap(), value, "{text}");
            let shortest = format!("{value:e}");
            if shortest != format!("{value:.precision$e}", precision = shortest.split_once('e').unwrap().0.bytes().filter(u8::is_ascii_digit).count() - 1) {
                ties += 1;
            }
        }
        assert!(ties > 0, "the sample has ties");
    }

    #[test]
    fn dotnet_shortest_digits_for_more_values() {
        for (value, text) in [(1.0 / 3.0, "0.3333333333333333"), (2.0f64.powi(60), "1.152921504606847E+18"), (5e-324, "5E-324"), (f64::MAX, "1.7976931348623157E+308")] {
            assert_eq!(format_double(value), text, "{value}");
        }
    }

    #[test]
    fn strings_match_dotnet() {
        let value = serde_json::json!({"a": "Expert's \"Dual\" · <T5> & +`\u{7f}\n\u{1}é😀"});
        assert_eq!(
            to_string(&value).unwrap(),
            r#"{"a":"Expert\u0027s \u0022Dual\u0022 \u00B7 \u003CT5\u003E \u0026 \u002B\u0060\u007F\n\u0001\u00E9\uD83D\uDE00"}"#
        );
        let date = serde_json::json!(["2026-11-27T01:29:04.2745512+00:00", "a+b"]);
        assert_eq!(to_string(&date).unwrap(), r#"["2026-11-27T01:29:04.2745512+00:00","a\u002Bb"]"#);
        assert_eq!(to_string_node(&date).unwrap(), r#"["2026-11-27T01:29:04.2745512\u002B00:00","a\u002Bb"]"#);
        assert_eq!(to_string_indented(&serde_json::json!({"device": "raw:any"})).unwrap(), "{\n  \"device\": \"raw:any\"\n}");
    }
}
