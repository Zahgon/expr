//! The `expr` command: compiles and runs one expression against a JSON environment and prints the result, the
//! type of the checked expression, or the error.
//!
//! Flags follow Go's `flag` package conventions (`-name value`, `-name=value`, `--name`, flags end at the first
//! argument that is not a flag or at `--`).

use std::io::{Read, Write};
use std::process::ExitCode;

use expr::builtin::json;
use expr::gostd::{json as gojson, strconv};
use expr::types::Kind;
use expr::value::{sprint_v, Value};
use expr::{CompileError, Expect, Options};

const EXIT_COMPILE: u8 = 1;
const EXIT_USAGE: u8 = 2;
const EXIT_RUN: u8 = 3;
const EXIT_OUTPUT: u8 = 4;

/// The kind of value a flag takes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FlagKind {
    Bool,
    String,
    Uint,
    List,
}

struct FlagDef {
    name: &'static str,
    kind: FlagKind,
    usage: &'static str,
    default: &'static str,
}

/// The flags, sorted by name as the usage text lists them.
const FLAGS: &[FlagDef] = &[
    FlagDef {
        name: "allow-undefined",
        kind: FlagKind::Bool,
        usage: "allow undefined variables (they evaluate to nil)",
        default: "",
    },
    FlagDef {
        name: "as",
        kind: FlagKind::String,
        usage: "expected result `kind`: any, bool, int, int64, float64 or a reflect kind name",
        default: "",
    },
    FlagDef {
        name: "check",
        kind: FlagKind::Bool,
        usage: "only compile; print the type of the expression",
        default: "",
    },
    FlagDef {
        name: "disable",
        kind: FlagKind::List,
        usage: "disable builtin `names` (comma-separated, repeatable; all disables every builtin)",
        default: "",
    },
    FlagDef { name: "disable-if", kind: FlagKind::Bool, usage: "treat if and else as identifiers", default: "" },
    FlagDef {
        name: "enable",
        kind: FlagKind::List,
        usage: "re-enable builtin `names` after -disable (comma-separated, repeatable)",
        default: "",
    },
    FlagDef { name: "env", kind: FlagKind::String, usage: "environment as a JSON `object`", default: "" },
    FlagDef {
        name: "env-file",
        kind: FlagKind::String,
        usage: "read the environment from `file` (- for standard input)",
        default: "",
    },
    FlagDef {
        name: "file",
        kind: FlagKind::String,
        usage: "read the expression from `file` (- for standard input)",
        default: "",
    },
    FlagDef {
        name: "max-nodes",
        kind: FlagKind::Uint,
        usage: "maximum number of AST `nodes` (0 means no limit)",
        default: "",
    },
    FlagDef { name: "no-optimize", kind: FlagKind::Bool, usage: "disable the optimizer", default: "" },
    FlagDef { name: "no-short-circuit", kind: FlagKind::Bool, usage: "evaluate both operands of and/or", default: "" },
    FlagDef {
        name: "output",
        kind: FlagKind::String,
        usage: "result `format`: value, typed or json",
        default: "value",
    },
    FlagDef {
        name: "timezone",
        kind: FlagKind::String,
        usage: "time zone `name` used by date() and now()",
        default: "",
    },
    FlagDef {
        name: "warn-on-any",
        kind: FlagKind::Bool,
        usage: "reject results of unknown type (needs -as)",
        default: "",
    },
];

/// The usage text, as Go's `flag.PrintDefaults` lays it out.
fn usage() -> String {
    let mut out = String::from("usage: expr [flags] EXPRESSION\n");
    for f in FLAGS {
        let mut line = format!("  -{}", f.name);
        let (name, text) =
            match f.usage.split_once('`').and_then(|(a, rest)| rest.split_once('`').map(|(n, b)| (a, n, b))) {
                Some((a, n, b)) => (n.to_string(), format!("{a}{n}{b}")),
                None => (
                    match f.kind {
                        FlagKind::Bool => String::new(),
                        FlagKind::String => "string".to_string(),
                        FlagKind::Uint => "uint".to_string(),
                        FlagKind::List => "value".to_string(),
                    },
                    f.usage.to_string(),
                ),
            };
        if !name.is_empty() {
            line.push(' ');
            line.push_str(&name);
        }
        line.push_str(if line.len() <= 4 { "\t" } else { "\n    \t" });
        line.push_str(&text.replace('\n', "\n    \t"));
        if !f.default.is_empty() {
            line.push_str(&format!(" (default {})", strconv::quote(f.default.as_bytes())));
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// Parsed flags: the values of the flags that were set, in order.
#[derive(Default)]
struct Flags {
    set: Vec<(&'static str, Vec<u8>)>,
    args: Vec<Vec<u8>>,
}

impl Flags {
    fn get(&self, name: &str) -> Option<&[u8]> {
        self.set.iter().rev().find(|(n, _)| *n == name).map(|(_, v)| &v[..])
    }

    fn is_set(&self, name: &str) -> bool {
        self.set.iter().any(|(n, _)| *n == name)
    }

    fn bool(&self, name: &str) -> bool {
        self.get(name).is_some_and(|v| parse_bool(v) == Some(true))
    }

    fn list(&self, name: &str) -> Vec<String> {
        self.set
            .iter()
            .filter(|(n, _)| *n == name)
            .flat_map(|(_, v)| String::from_utf8_lossy(v).split(',').map(str::to_string).collect::<Vec<_>>())
            .collect()
    }
}

fn parse_bool(v: &[u8]) -> Option<bool> {
    match v {
        b"1" | b"t" | b"T" | b"TRUE" | b"true" | b"True" => Some(true),
        b"0" | b"f" | b"F" | b"FALSE" | b"false" | b"False" => Some(false),
        _ => None,
    }
}

/// Why flag parsing stopped.
enum FlagStop {
    Help,
    Error(String),
}

fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// Parses the arguments like Go's `FlagSet.Parse`.
fn parse_flags(args: &[Vec<u8>]) -> Result<Flags, FlagStop> {
    let mut flags = Flags::default();
    let mut i = 0;
    while i < args.len() {
        let s = &args[i];
        if s.len() < 2 || s[0] != b'-' {
            break;
        }
        let mut minuses = 1;
        if s[1] == b'-' {
            minuses = 2;
            if s.len() == 2 {
                i += 1;
                break;
            }
        }
        let name = &s[minuses..];
        if name.is_empty() || name[0] == b'-' || name[0] == b'=' {
            return Err(FlagStop::Error(format!("bad flag syntax: {}", lossy(s))));
        }
        i += 1;
        let (name, value) = match name.iter().position(|&b| b == b'=') {
            Some(p) => (&name[..p], Some(name[p + 1..].to_vec())),
            None => (name, None),
        };
        let Some(def) = FLAGS.iter().find(|f| f.name.as_bytes() == name) else {
            if name == b"help" || name == b"h" {
                return Err(FlagStop::Help);
            }
            return Err(FlagStop::Error(format!("flag provided but not defined: -{}", lossy(name))));
        };
        let value = if def.kind == FlagKind::Bool {
            let v = value.unwrap_or_else(|| b"true".to_vec());
            if parse_bool(&v).is_none() {
                return Err(FlagStop::Error(format!(
                    "invalid boolean value {} for -{}: parse error",
                    strconv::quote(&v),
                    def.name
                )));
            }
            v
        } else {
            let v = match value {
                Some(v) => v,
                None if i < args.len() => {
                    i += 1;
                    args[i - 1].clone()
                }
                None => return Err(FlagStop::Error(format!("flag needs an argument: -{}", def.name))),
            };
            let invalid = |why: &str| {
                Err(FlagStop::Error(format!("invalid value {} for flag -{}: {why}", strconv::quote(&v), def.name)))
            };
            match def.kind {
                FlagKind::Uint => {
                    if let Err(e) = strconv::parse_uint(&v, 0, 64) {
                        let why = if e.to_string().ends_with("value out of range") {
                            "value out of range"
                        } else {
                            "parse error"
                        };
                        return invalid(why);
                    }
                }
                FlagKind::List => {
                    if v.split(|&b| b == b',').any(<[u8]>::is_empty) {
                        return invalid("empty builtin name");
                    }
                }
                _ => {}
            }
            v
        };
        flags.set.push((def.name, value));
    }
    flags.args = args[i..].to_vec();
    Ok(flags)
}

/// The text Go's `os.ReadFile` reports for a failed read.
fn read_input(name: &[u8], stdin: &mut dyn Read) -> Result<Vec<u8>, String> {
    if name == b"-" {
        let mut data = Vec::new();
        stdin.read_to_end(&mut data).map_err(|e| format!("read /dev/stdin: {e}"))?;
        return Ok(data);
    }
    let path = lossy(name);
    match std::fs::read(&path) {
        Ok(data) => Ok(data),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(format!("open {path}: no such file or directory")),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Err(format!("open {path}: permission denied")),
        Err(e) if e.kind() == std::io::ErrorKind::IsADirectory => Err(format!("read {path}: is a directory")),
        Err(e) => Err(format!("open {path}: {e}")),
    }
}

/// Decodes the environment: one JSON object whose integers become `int` and other numbers `float64`.
fn decode_env(data: &[u8]) -> Result<Vec<(Vec<u8>, Value)>, String> {
    let (value, offset) = gojson::decode_use_number(data)?;
    if !gojson::only_whitespace_follows(data, offset) {
        return Err("unexpected data after the JSON value".to_string());
    }
    let kind = match &value {
        gojson::Json::Object(_) => None,
        gojson::Json::Null => Some("null"),
        gojson::Json::Bool(_) => Some("a boolean"),
        gojson::Json::Number(_) => Some("a number"),
        gojson::Json::String(_) => Some("a string"),
        gojson::Json::Array(_) => Some("an array"),
    };
    if let Some(kind) = kind {
        return Err(format!("the environment must be a JSON object, not {kind}"));
    }
    if let Some(text) = json::number_out_of_range(&value) {
        return Err(format!("number {} is out of range", lossy(&text)));
    }
    let gojson::Json::Object(entries) = value else { return Ok(Vec::new()) };
    Ok(entries.into_iter().map(|(k, v)| (k, json::from_json(v, true))).collect())
}

fn expect_of(name: &[u8]) -> Option<Expect> {
    Some(match name {
        b"any" => Expect::Any,
        b"bool" => Expect::Kind(Kind::Bool),
        b"int" => Expect::Kind(Kind::Int),
        b"int64" => Expect::Kind(Kind::Int64),
        b"float64" => Expect::Kind(Kind::Float64),
        other => Expect::Kind(Kind::by_name(std::str::from_utf8(other).ok()?)?),
    })
}

struct Io<'a> {
    stdin: &'a mut dyn Read,
    stdout: &'a mut dyn Write,
    stderr: &'a mut dyn Write,
}

impl Io<'_> {
    fn err_line(&mut self, text: &[u8]) {
        let _ = self.stderr.write_all(text);
        let _ = self.stderr.write_all(b"\n");
    }

    fn usage_error(&mut self, message: &str) -> u8 {
        self.err_line(format!("expr: {message}").as_bytes());
        let _ = self.stderr.write_all(usage().as_bytes());
        EXIT_USAGE
    }
}

fn run(args: &[Vec<u8>], io: &mut Io) -> u8 {
    let flags = match parse_flags(args) {
        Ok(f) => f,
        Err(FlagStop::Help) => {
            let _ = io.stderr.write_all(usage().as_bytes());
            return 0;
        }
        Err(FlagStop::Error(e)) => {
            io.err_line(e.as_bytes());
            let _ = io.stderr.write_all(usage().as_bytes());
            return EXIT_USAGE;
        }
    };

    let output = flags.get("output").unwrap_or(b"value").to_vec();
    if !matches!(&output[..], b"value" | b"typed" | b"json") {
        return io.usage_error(&format!("invalid output format {}", strconv::quote(&output)));
    }
    if flags.is_set("env") && flags.is_set("env-file") {
        return io.usage_error("-env and -env-file cannot be used together");
    }
    if flags.get("env-file") == Some(b"-") && flags.get("file") == Some(b"-") {
        return io.usage_error("standard input can be read only once");
    }

    let input = if flags.is_set("file") {
        if let Some(extra) = flags.args.first() {
            return io.usage_error(&format!("unexpected argument {}", strconv::quote(extra)));
        }
        match read_input(flags.get("file").unwrap_or_default(), io.stdin) {
            Ok(data) => data,
            Err(e) => {
                io.err_line(format!("expr: cannot read expression: {e}").as_bytes());
                return EXIT_USAGE;
            }
        }
    } else {
        match flags.args.len() {
            0 => return io.usage_error("missing expression"),
            1 => flags.args[0].clone(),
            _ => return io.usage_error("too many arguments"),
        }
    };

    let mut env = None;
    if flags.is_set("env") || flags.is_set("env-file") {
        let data = if flags.is_set("env-file") {
            match read_input(flags.get("env-file").unwrap_or_default(), io.stdin) {
                Ok(d) => d,
                Err(e) => {
                    io.err_line(format!("expr: invalid environment: {e}").as_bytes());
                    return EXIT_USAGE;
                }
            }
        } else {
            flags.get("env").unwrap_or_default().to_vec()
        };
        match decode_env(&data) {
            Ok(vars) => env = Some(vars),
            Err(e) => {
                io.err_line(format!("expr: invalid environment: {e}").as_bytes());
                return EXIT_USAGE;
            }
        }
    }

    let mut options = Options { env: env.clone(), ..Options::default() };
    options.allow_undefined_variables = flags.bool("allow-undefined");
    if let Some(name) = flags.get("as") {
        match expect_of(name) {
            Some(e) => options.expect = Some(e),
            None => {
                io.err_line(format!("expr: unknown kind {}", strconv::quote(name)).as_bytes());
                return EXIT_USAGE;
            }
        }
    }
    options.warn_on_any = flags.bool("warn-on-any");
    options.optimize = !flags.bool("no-optimize");
    options.short_circuit = !flags.bool("no-short-circuit");
    options.disable_if_operator = flags.bool("disable-if");
    options.disabled = flags.list("disable");
    options.enabled = flags.list("enable");
    if let Some(tz) = flags.get("timezone") {
        if let Err(e) = expr::gostd::time::load_location(tz) {
            io.err_line(format!("expr: {e}").as_bytes());
            return EXIT_USAGE;
        }
        options.timezone = Some(tz.to_vec());
    }
    if let Some(n) = flags.get("max-nodes") {
        options.max_nodes = strconv::parse_uint(n, 0, 64).ok();
    }

    let program = match expr::compile(&input, &options) {
        Ok(p) => p,
        Err(CompileError::Option(e)) => {
            io.err_line(e.as_bytes());
            return EXIT_COMPILE;
        }
        Err(CompileError::Expr(e)) => {
            io.err_line(&e.to_bytes());
            return EXIT_COMPILE;
        }
    };
    if flags.bool("check") {
        let _ = writeln!(io.stdout, "{}", program.node.ty());
        return 0;
    }

    let env_value = match env {
        Some(vars) => {
            let mut m = expr::value::MapValue::new(expr::types::Type::string_map());
            for (k, v) in vars {
                let _ = m.insert(Value::string(k), v);
            }
            Value::Map(std::rc::Rc::new(m))
        }
        None => Value::Nil,
    };
    let out = match expr::run(&program, &env_value) {
        Ok(v) => v,
        Err(e) => {
            io.err_line(&e.to_bytes());
            return EXIT_RUN;
        }
    };

    let mut line = Vec::new();
    match &output[..] {
        b"typed" => {
            line.extend_from_slice(out.type_name().as_bytes());
            line.push(b' ');
            line.extend_from_slice(&sprint_v(&out));
        }
        b"json" => match json::marshal(&out) {
            Ok(data) => line = data,
            Err(e) => {
                io.err_line(e.as_bytes());
                return EXIT_OUTPUT;
            }
        },
        _ => line = sprint_v(&out),
    }
    line.push(b'\n');
    let _ = io.stdout.write_all(&line);
    0
}

fn main() -> ExitCode {
    use std::os::unix::ffi::OsStringExt;
    let args: Vec<Vec<u8>> = std::env::args_os().skip(1).map(OsStringExt::into_vec).collect();
    // Deeply nested expressions recurse deeply; the original's goroutine stacks grow, so give ours room.
    let worker = std::thread::Builder::new().stack_size(1 << 30).spawn(move || {
        let (mut stdin, mut stdout, mut stderr) = (std::io::stdin(), std::io::stdout(), std::io::stderr());
        let mut io = Io { stdin: &mut stdin, stdout: &mut stdout, stderr: &mut stderr };
        let code = run(&args, &mut io);
        let _ = io.stdout.flush();
        code
    });
    match worker.map(|w| w.join()) {
        Ok(Ok(code)) => ExitCode::from(code),
        _ => ExitCode::from(EXIT_RUN),
    }
}
