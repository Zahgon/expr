//! JSON encoding and decoding of values, as Go's `encoding/json` does it for `any`.

use std::rc::Rc;

use crate::gostd::{base64, json, strconv};
use crate::types::Type;
use crate::value::{MapValue, Value};

/// `json.Marshal(v)`: the compact encoding, or Go's error text.
pub fn marshal(v: &Value) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    encode(v, &mut out)?;
    Ok(out)
}

fn unsupported(t: &Type) -> String {
    format!("json: unsupported type: {t}")
}

fn encode(v: &Value, out: &mut Vec<u8>) -> Result<(), String> {
    match v {
        Value::Nil => out.extend_from_slice(b"null"),
        Value::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        Value::Uint(u) | Value::Uint64(u) => out.extend_from_slice(u.to_string().as_bytes()),
        Value::Float32(f) => out.extend_from_slice(&json::encode_float(f64::from(*f), 32)?),
        Value::Float64(f) => out.extend_from_slice(&json::encode_float(*f, 64)?),
        Value::String(s) => out.extend_from_slice(&json::encode_string(s, true)),
        Value::Slice(seq) if seq.nil => out.extend_from_slice(b"null"),
        Value::Slice(seq) if seq.ty == Type::bytes() => {
            let bytes: Vec<u8> = seq.items.iter().filter_map(|b| b.as_i64().map(|x| x as u8)).collect();
            out.push(b'"');
            out.extend_from_slice(base64::std_encode(&bytes).as_bytes());
            out.push(b'"');
        }
        Value::Slice(seq) | Value::Array(seq) => {
            out.push(b'[');
            for (i, item) in seq.items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                encode(item, out)?;
            }
            out.push(b']');
        }
        Value::Map(m) => {
            if m.ty.key() != Some(&Type::String) {
                return Err(unsupported(&m.ty));
            }
            let mut entries: Vec<(&[u8], &Value)> = m
                .entries()
                .iter()
                .map(|(k, v)| (if let Value::String(s) = k { &s[..] } else { &b""[..] }, v))
                .collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            out.push(b'{');
            for (i, (k, v)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                out.extend_from_slice(&json::encode_string(k, true));
                out.push(b':');
                encode(v, out)?;
            }
            out.push(b'}');
        }
        Value::Time(t) => {
            let bytes =
                t.marshal_json().map_err(|e| format!("json: error calling MarshalJSON for type time.Time: {e}"))?;
            out.extend_from_slice(&bytes);
        }
        Value::Location(_) | Value::LocationValue(_) | Value::EmptyStruct => out.extend_from_slice(b"{}"),
        Value::Func(f) => return Err(unsupported(&f.ty)),
        other => out.extend_from_slice(other.as_i64().unwrap_or_default().to_string().as_bytes()),
    }
    Ok(())
}

/// `json.Unmarshal(data, &v)` with `v any`: numbers become `float64`, objects `map[string]interface {}`.
pub fn unmarshal(data: &[u8]) -> Result<Value, String> {
    Ok(from_json(json::unmarshal(data)?, false))
}

/// Converts decoded JSON to a value. With `typed_numbers`, numbers written without a fraction or an exponent
/// that fit in an int become `int` (the environment model); otherwise every number is a `float64`.
pub fn from_json(j: json::Json, typed_numbers: bool) -> Value {
    match j {
        json::Json::Null => Value::Nil,
        json::Json::Bool(b) => Value::Bool(b),
        json::Json::Number(text) => {
            if typed_numbers && !text.iter().any(|b| matches!(b, b'.' | b'e' | b'E')) {
                if let Ok(i) = strconv::parse_int(&text, 10, 64) {
                    return Value::Int(i);
                }
            }
            Value::Float64(strconv::parse_float(&text, 64).unwrap_or(f64::INFINITY))
        }
        json::Json::String(s) => Value::string(s),
        json::Json::Array(items) => Value::any_slice(items.into_iter().map(|j| from_json(j, typed_numbers)).collect()),
        json::Json::Object(entries) => {
            let mut m = MapValue::new(Type::string_map());
            for (k, v) in entries {
                let _ = m.insert(Value::string(k), from_json(v, typed_numbers));
            }
            Value::Map(Rc::new(m))
        }
    }
}

/// Whether a JSON number of the environment does not fit a float64 (the original rejects the environment).
pub fn number_out_of_range(j: &json::Json) -> Option<Vec<u8>> {
    match j {
        json::Json::Number(text) => {
            let integer = !text.iter().any(|b| matches!(b, b'.' | b'e' | b'E'));
            if integer && strconv::parse_int(text, 10, 64).is_ok() {
                return None;
            }
            strconv::parse_float(text, 64).err().map(|_| text.clone())
        }
        json::Json::Array(items) => items.iter().find_map(number_out_of_range),
        json::Json::Object(entries) => entries.iter().find_map(|(_, v)| number_out_of_range(v)),
        _ => None,
    }
}
