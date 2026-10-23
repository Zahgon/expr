//! Calls of function values: methods of the `time` types bound to their receiver.
//!
//! A call through a signature of the typed-call table type-asserts each argument; any other call goes through
//! reflection, which turns nil arguments into zero values and rejects arguments of the wrong type.

use crate::gostd::time::{self, Time};
use crate::types::Type;
use crate::value::{FuncValue, Value};
use crate::vm::func_types::FUNC_TYPES;
use crate::vm::runtime::{assertion, zero_value, Panic, RtResult};

/// The method value's input types.
fn inputs(f: &FuncValue) -> Vec<Type> {
    f.ty.as_func().map(|func| func.ins.clone()).unwrap_or_default()
}

/// `OpCall`: a call through reflection.
pub fn call_reflect(f: &Value, args: Vec<Value>) -> RtResult<Value> {
    let func = match f {
        Value::Nil => return Err("invalid operation: cannot call nil".into()),
        Value::Func(func) => func,
        other => {
            return Err(format!("invalid operation: cannot call non-function of type {}", other.type_name()).into())
        }
    };
    let ins = inputs(func);
    if args.len() != ins.len() {
        return Err(format!("invalid number of arguments: expected {}, got {}", ins.len(), args.len()).into());
    }
    let mut prepared = Vec::with_capacity(args.len());
    for (arg, input) in args.into_iter().zip(&ins) {
        prepared.push(match arg {
            Value::Nil => zero_value(input),
            other => {
                let t = other.type_of().unwrap_or(Type::Any);
                if !t.assignable_to(input) {
                    return Err(format!("reflect: Call using {t} as type {input}").into());
                }
                other
            }
        });
    }
    invoke(func, &prepared)
}

/// `OpCallTyped`: a call through the typed-call table; arguments are popped (and asserted) last first.
pub fn call_typed(f: &Value, kind: usize, stack: &mut Vec<Value>) -> RtResult<Value> {
    let signature = FUNC_TYPES.get(kind.wrapping_sub(1)).copied().unwrap_or("func()");
    let params: Vec<&str> = signature
        .strip_prefix("func(")
        .and_then(|s| s.split_once(')'))
        .map(|(ins, _)| if ins.is_empty() { Vec::new() } else { ins.split(", ").collect() })
        .unwrap_or_default();
    let mut args = vec![Value::Nil; params.len()];
    for (i, param) in params.iter().enumerate().rev() {
        let v = stack.pop().ok_or_else(|| Panic::from("stack underflow"))?;
        if v.type_name() != *param {
            return Err(assertion(&v, param));
        }
        args[i] = v;
    }
    let Value::Func(func) = f else {
        return Err(assertion(f, signature));
    };
    invoke(func, &args)
}

fn time_arg(args: &[Value], i: usize) -> Time {
    match args.get(i) {
        Some(Value::Time(t)) => t.clone(),
        _ => Time::default(),
    }
}

fn int_arg(args: &[Value], i: usize) -> i64 {
    args.get(i).and_then(Value::as_i64).unwrap_or_default()
}

fn bytes_value(bytes: Vec<u8>) -> Value {
    Value::slice(Type::Uint8, bytes.into_iter().map(Value::Uint8).collect())
}

fn bytes_arg(args: &[Value], i: usize) -> Vec<u8> {
    match args.get(i) {
        Some(Value::Slice(seq)) => seq.items.iter().filter_map(|v| v.as_i64().map(|b| b as u8)).collect(),
        _ => Vec::new(),
    }
}

/// Go's `Time.MarshalBinary` encoding appended to `b`.
fn append_binary(t: &Time, mut b: Vec<u8>) -> Result<Vec<u8>, String> {
    let (sec, nsec, loc) = t.identity();
    let mut version = 1u8;
    let mut offset_sec: i8 = 0;
    let offset_min: i16 = if loc == 0 {
        -1
    } else {
        let (_, offset) = t.zone();
        if offset % 60 != 0 {
            version = 2;
            offset_sec = (offset % 60) as i8;
        }
        let minutes = offset / 60;
        if !(-32768..=32767).contains(&minutes) || minutes == -1 {
            return Err("Time.MarshalBinary: unexpected zone offset".to_string());
        }
        minutes as i16
    };
    b.push(version);
    b.extend_from_slice(&sec.to_be_bytes());
    b.extend_from_slice(&nsec.to_be_bytes());
    b.extend_from_slice(&offset_min.to_be_bytes());
    if version == 2 {
        b.push(offset_sec as u8);
    }
    Ok(b)
}

fn result_or_error(r: Result<Vec<u8>, String>) -> RtResult<Value> {
    r.map(bytes_value).map_err(Panic::from)
}

/// Calls a bound method with prepared arguments and returns its first result.
fn invoke(f: &FuncValue, args: &[Value]) -> RtResult<Value> {
    match &f.receiver {
        Value::Time(t) => time_method(t, f.method, args),
        Value::Duration(d) => {
            let d = *d;
            Ok(match f.method {
                "Abs" => Value::Duration(time::duration_abs(d)),
                "Hours" => Value::Float64(time::duration_hours(d)),
                "Microseconds" => Value::Int64(time::duration_microseconds(d)),
                "Milliseconds" => Value::Int64(time::duration_milliseconds(d)),
                "Minutes" => Value::Float64(time::duration_minutes(d)),
                "Nanoseconds" => Value::Int64(d),
                "Round" => Value::Duration(time::duration_round(d, int_arg(args, 0))),
                "Seconds" => Value::Float64(time::duration_seconds(d)),
                "Truncate" => Value::Duration(time::duration_truncate(d, int_arg(args, 0))),
                _ => Value::string(time::duration_string(d)),
            })
        }
        Value::Month(m) => Ok(Value::string(time::month_string(*m))),
        Value::Weekday(w) => Ok(Value::string(time::weekday_string(*w))),
        Value::Location(l) => Ok(Value::string(l.name())),
        other => Err(format!("invalid operation: cannot call non-function of type {}", other.type_name()).into()),
    }
}

fn time_method(t: &Time, method: &str, args: &[Value]) -> RtResult<Value> {
    Ok(match method {
        "Add" => Value::Time(t.add(int_arg(args, 0))),
        "AddDate" => Value::Time(t.add_date(int_arg(args, 0), int_arg(args, 1), int_arg(args, 2))),
        "After" => Value::Bool(t.after(&time_arg(args, 0))),
        "AppendBinary" => return result_or_error(append_binary(t, bytes_arg(args, 0))),
        "AppendFormat" => {
            let layout = match args.get(1) {
                Some(Value::String(s)) => s.to_vec(),
                _ => Vec::new(),
            };
            bytes_value(t.append_format(bytes_arg(args, 0), &layout))
        }
        "AppendText" => {
            let mut b = bytes_arg(args, 0);
            return match t.marshal_text() {
                Ok(text) => {
                    b.extend_from_slice(&text);
                    Ok(bytes_value(b))
                }
                Err(e) => Err(e.replace("MarshalText", "AppendText").into()),
            };
        }
        "Before" => Value::Bool(t.before(&time_arg(args, 0))),
        "Compare" => Value::Int(t.compare(&time_arg(args, 0))),
        "Day" => Value::Int(t.day()),
        "Equal" => Value::Bool(t.equal(&time_arg(args, 0))),
        "Format" => {
            let layout = match args.first() {
                Some(Value::String(s)) => s.to_vec(),
                _ => Vec::new(),
            };
            Value::string(t.format(&layout))
        }
        "GoString" => Value::string(t.go_string()),
        "GobEncode" | "MarshalBinary" => {
            return result_or_error(append_binary(t, Vec::new()).map_err(|e| e.replace("MarshalBinary", method)))
        }
        "Hour" => Value::Int(t.hour()),
        "ISOWeek" => Value::Int(t.iso_week().0),
        "In" => match args.first() {
            Some(Value::Location(l)) => Value::Time(t.in_location(l)),
            _ => return Err("time: missing Location in call to Time.In".into()),
        },
        "IsDST" => Value::Bool(t.is_dst()),
        "IsZero" => Value::Bool(t.is_zero()),
        "Local" => Value::Time(t.local()),
        "Location" => Value::Location(t.location()),
        "MarshalJSON" => return result_or_error(t.marshal_json()),
        "MarshalText" => return result_or_error(t.marshal_text()),
        "Minute" => Value::Int(t.minute()),
        "Month" => Value::Month(t.month()),
        "Nanosecond" => Value::Int(t.nanosecond()),
        "Round" => Value::Time(t.round(int_arg(args, 0))),
        "Second" => Value::Int(t.second()),
        "Sub" => Value::Duration(t.sub(&time_arg(args, 0))),
        "Truncate" => Value::Time(t.truncate(int_arg(args, 0))),
        "UTC" => Value::Time(t.utc()),
        "Unix" => Value::Int64(t.unix()),
        "UnixMicro" => Value::Int64(t.unix_micro()),
        "UnixMilli" => Value::Int64(t.unix_milli()),
        "UnixNano" => Value::Int64(t.unix_nano()),
        "Weekday" => Value::Weekday(t.weekday()),
        "Year" => Value::Int(t.year()),
        "YearDay" => Value::Int(t.year_day()),
        "Zone" => Value::string(t.zone().0),
        "ZoneBounds" => Value::Time(t.zone_bounds().0),
        "Clock" => Value::Int(t.clock().0),
        "Date" => Value::Int(t.date_parts().0),
        _ => Value::string(t.string()),
    })
}
