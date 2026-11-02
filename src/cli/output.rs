use crate::{Result, Value};
use std::fmt::Write;
use unicode_width::UnicodeWidthStr;

pub(super) fn json(value: &impl serde::Serialize) -> Result<String> {
    let encoded = serde_json::to_string_pretty(value)?;
    let mut output = String::with_capacity(encoded.len() + 1);
    for ch in encoded.chars() {
        if ch.is_ascii() {
            output.push(ch);
        } else {
            for unit in ch.encode_utf16(&mut [0; 2]) {
                write!(output, "\\u{unit:04x}").expect("writing to a string cannot fail");
            }
        }
    }
    output.push('\n');
    Ok(output)
}

pub(super) fn cell(value: &Value) -> String {
    match value {
        Value::Null => "-".into(),
        Value::String(s) if s.is_empty() => "-".into(),
        Value::Array(values) if values.is_empty() => "-".into(),
        Value::Object(values) if values.is_empty() => "-".into(),
        Value::Bool(true) => "yes".into(),
        Value::Bool(false) => "no".into(),
        Value::String(s) => s.clone(),
        Value::Array(values) => values
            .iter()
            .map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_owned))
            .collect::<Vec<_>>()
            .join(", "),
        other => other.to_string(),
    }
}

pub(super) fn table(headers: &[String], rows: &[Vec<String>], empty: &str) -> String {
    if rows.is_empty() {
        return format!("{empty}\n");
    }
    let widths: Vec<_> = headers
        .iter()
        .enumerate()
        .map(|(i, header)| {
            rows.iter()
                .map(|row| row[i].width())
                .chain([header.width()])
                .max()
                .unwrap_or(0)
        })
        .collect();
    let mut output = String::new();
    for row in std::iter::once(headers).chain(rows.iter().map(Vec::as_slice)) {
        for (i, value) in row.iter().enumerate() {
            output.push_str(value);
            if i + 1 < row.len() {
                output.extend(std::iter::repeat_n(' ', widths[i] - value.width() + 2));
            }
        }
        output.push('\n');
    }
    output
}

// Python's general format uses significant digits and switches to scientific notation
// for an exponent below -4 or at least the precision, after rounding.
pub(super) fn general(value: f64, precision: usize) -> String {
    let scientific = format!("{:.*e}", precision - 1, value);
    let (mantissa, exponent) = scientific.split_once('e').expect("finite JSON number");
    let exponent: i32 = exponent.parse().expect("formatted exponent");
    if exponent < -4 || exponent >= i32::try_from(precision).expect("small fixed precision") {
        format!("{}e{exponent:+03}", trim_decimal(mantissa))
    } else {
        let decimals = usize::try_from(
            i32::try_from(precision).expect("small fixed precision") - 1 - exponent,
        )
        .unwrap_or(0);
        trim_decimal(&format!("{value:.decimals$}")).to_owned()
    }
}
fn trim_decimal(value: &str) -> &str {
    if value.contains('.') {
        value.trim_end_matches('0').trim_end_matches('.')
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json;

    #[test]
    fn test_cell() {
        assert_eq!(cell(&Value::Null), "-");
        assert_eq!(cell(&json!(true)), "yes");
        assert_eq!(cell(&json!(["a", "b"])), "a, b");
        assert_eq!(cell(&json!(3)), "3");
    }

    #[test]
    fn test_print_table_keeps_ids_and_values_whole() {
        let headers = ["ID", "TYPE", "PAD"].map(str::to_owned);
        let rows = vec![vec![
            "cmvfsn771000007jp15tn8esm".into(),
            "high_performance".into(),
            "x".repeat(30),
        ]];
        let text = table(&headers, &rows, "none");
        assert!(text.contains("cmvfsn771000007jp15tn8esm"));
        assert!(!text.contains('…'));
    }
}
