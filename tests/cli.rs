//! Runs the `expr` command over expressions taken from the upstream test suites (parser, checker, optimizer, VM,
//! builtins and the expression tables), adapted to a JSON environment, and compares standard output, standard
//! error and the exit status with the original command's.

use std::process::Command;

#[derive(Debug)]
struct Case {
    name: String,
    args: Vec<String>,
    status: i32,
    stdout: String,
    stderr: String,
}

/// Reads the fixture, a JSON array of cases; the parsing is minimal on purpose (strings and integers only).
fn load_cases() -> Vec<Case> {
    let text = include_str!("testdata/upstream_cases.json");
    let value = expr::builtin::json::unmarshal(text.as_bytes()).expect("fixture is valid JSON");
    let items = value.items().expect("fixture is an array").to_vec();
    items
        .iter()
        .map(|item| {
            let field = |name: &str| match &item {
                expr::value::Value::Map(m) => m.get(&expr::value::Value::string(name)).ok().flatten().cloned(),
                _ => None,
            };
            let text = |name: &str| match field(name) {
                Some(expr::value::Value::String(s)) => String::from_utf8_lossy(&s).into_owned(),
                _ => String::new(),
            };
            let args = field("args")
                .and_then(|a| a.items().map(<[expr::value::Value]>::to_vec))
                .unwrap_or_default()
                .iter()
                .map(|v| match v {
                    expr::value::Value::String(s) => String::from_utf8_lossy(s).into_owned(),
                    _ => String::new(),
                })
                .collect();
            let status = match field("status") {
                Some(expr::value::Value::Float64(f)) => f as i32,
                _ => -1,
            };
            Case { name: text("name"), args, status, stdout: text("stdout"), stderr: text("stderr") }
        })
        .collect()
}

#[test]
fn matches_the_original_command() {
    let cases = load_cases();
    assert!(cases.len() > 500, "fixture has {} cases", cases.len());
    let mut failures = Vec::new();
    for case in &cases {
        let out = Command::new(env!("CARGO_BIN_EXE_expr"))
            .args(&case.args)
            .env("TZ", "UTC")
            .output()
            .expect("the expr binary runs");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        let status = out.status.code().unwrap_or(-1);
        if status != case.status || stdout != case.stdout || stderr != case.stderr {
            failures.push(format!(
                "{} ({}):\n  want status {} stdout {:?} stderr {:?}\n  got  status {} stdout {:?} stderr {:?}",
                case.name,
                case.args.last().map_or("", String::as_str),
                case.status,
                case.stdout,
                case.stderr,
                status,
                stdout,
                stderr
            ));
        }
    }
    assert!(failures.is_empty(), "{} of {} cases differ:\n{}", failures.len(), cases.len(), failures.join("\n"));
}
