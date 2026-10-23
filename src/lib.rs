//! An expression language and its evaluator: a Rust port of expr (github.com/expr-lang/expr, MIT licence).
//!
//! An expression is parsed, type-checked against the nature of its environment, optimized, compiled to
//! bytecode and run on a small virtual machine. Values keep their Go types, and every message, number and
//! date is printed the way the original prints it.
//!
//! ```
//! use expr::{compile, run, Options};
//! use expr::value::Value;
//!
//! let program = compile(b"1 + 2 * 3", &Options::default()).unwrap();
//! let out = run(&program, &Value::Nil).unwrap();
//! assert_eq!(expr::value::sprint_v(&out), b"7");
//! ```

pub mod ast;
pub mod builtin;
pub mod checker;
pub mod compiler;
pub mod conf;
pub mod file;
pub mod gostd;
pub mod optimizer;
pub mod parser;
pub mod types;
pub mod value;
pub mod vm;

use checker::nature::Nature;
use compiler::Program;
use conf::Config;
use file::Error;
use types::Kind;
use value::Value;

/// The expected result of an expression (`AsAny`, `AsBool`, `AsInt`, `AsInt64`, `AsFloat64`, `AsKind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expect {
    Any,
    Kind(Kind),
}

/// Compile options, applied in the order of the fields like the original's options.
#[derive(Clone, Debug)]
pub struct Options {
    /// The environment: variable names and values. `None` compiles without an environment.
    pub env: Option<Vec<(Vec<u8>, Value)>>,
    pub allow_undefined_variables: bool,
    pub expect: Option<Expect>,
    pub warn_on_any: bool,
    pub optimize: bool,
    pub short_circuit: bool,
    pub disable_if_operator: bool,
    /// Builtins to disable; `all` disables every builtin.
    pub disabled: Vec<String>,
    /// Builtins enabled again after the disables.
    pub enabled: Vec<String>,
    /// The time zone `date()` and `now()` use.
    pub timezone: Option<Vec<u8>>,
    pub max_nodes: Option<u64>,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            env: None,
            allow_undefined_variables: false,
            expect: None,
            warn_on_any: false,
            optimize: true,
            short_circuit: true,
            disable_if_operator: false,
            disabled: Vec::new(),
            enabled: Vec::new(),
            timezone: None,
            max_nodes: None,
        }
    }
}

/// Why an expression could not be compiled.
#[derive(Debug)]
pub enum CompileError {
    /// An option is invalid (an unknown time zone, `warn_on_any` without an expected kind).
    Option(String),
    /// A syntax or type error, or an error found while optimizing.
    Expr(Error),
}

/// Applies the options to a fresh configuration.
pub fn configure(options: &Options) -> Result<Config, CompileError> {
    let mut config = Config::default();
    if let Some(env) = &options.env {
        config.env = Nature::env(env);
        config.strict = true;
    }
    if options.allow_undefined_variables {
        config.strict = false;
    }
    match options.expect {
        Some(Expect::Any) => config.expect_any = true,
        Some(Expect::Kind(kind)) => {
            config.expect = kind;
            config.expect_any = true;
        }
        None => {}
    }
    if options.warn_on_any {
        if config.expect == Kind::Invalid {
            return Err(CompileError::Option(
                "WarnOnAny() works only with combination with AsInt(), AsBool(), etc. options".to_string(),
            ));
        }
        config.expect_any = false;
    }
    config.optimize = options.optimize;
    config.short_circuit = options.short_circuit;
    config.disable_if_operator = options.disable_if_operator;
    for name in &options.disabled {
        if name == "all" {
            config.disabled.extend(config.builtins.iter().map(|n| n.to_string()));
        } else {
            config.disabled.insert(name.clone());
        }
    }
    for name in &options.enabled {
        config.disabled.remove(name);
    }
    if let Some(name) = &options.timezone {
        config.timezone = Some(gostd::time::load_location(name).map_err(CompileError::Option)?);
    }
    if let Some(n) = options.max_nodes {
        config.max_nodes = n;
    }
    let disabled = config.disabled.clone();
    config.builtins.retain(|name| !disabled.contains(*name));
    Ok(config)
}

/// Compiles an expression.
pub fn compile(input: &[u8], options: &Options) -> Result<Program, CompileError> {
    let config = configure(options)?;
    compile_with(input, &config).map_err(CompileError::Expr)
}

/// Compiles an expression with a prepared configuration.
pub fn compile_with(input: &[u8], config: &Config) -> Result<Program, Error> {
    let mut tree = checker::parse_check(input, config)?;
    if config.optimize {
        optimizer::optimize(&mut tree.node).map_err(|e| e.bind(&tree.source))?;
    }
    compiler::compile(tree, config)
}

/// Runs a compiled program against an environment map (or nil).
pub fn run(program: &Program, env: &Value) -> Result<Value, Error> {
    vm::run(program, env)
}
