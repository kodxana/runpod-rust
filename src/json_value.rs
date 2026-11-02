use crate::Value;

// Python compares booleans and integers/floats numerically, including nested JSON values.
// The guarded float-to-i128 conversions are exact for integral values in JSON's i64/u64
// range. Out-of-range floats saturate outside that range and cannot compare equal.
#[allow(clippy::cast_possible_truncation)]
pub(crate) fn equivalent(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => {
            let integer = |v: &serde_json::Number| {
                v.as_i64()
                    .map(i128::from)
                    .or_else(|| v.as_u64().map(i128::from))
            };
            match (integer(a), integer(b)) {
                (Some(a), Some(b)) => a == b,
                (Some(i), None) => b
                    .as_f64()
                    .is_some_and(|f| f.fract() == 0.0 && f as i128 == i),
                (None, Some(i)) => a
                    .as_f64()
                    .is_some_and(|f| f.fract() == 0.0 && f as i128 == i),
                (None, None) => a == b,
            }
        }
        (Value::Bool(a), Value::Number(b)) | (Value::Number(b), Value::Bool(a)) => {
            b.as_f64() == Some(f64::from(u8::from(*a)))
        }
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| equivalent(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(k, a)| b.get(k).is_some_and(|b| equivalent(a, b)))
        }
        _ => a == b,
    }
}
