//! Compile options.

use std::collections::{BTreeSet, HashSet};

use crate::builtin;
use crate::checker::nature::Nature;
use crate::gostd::time::Loc;
use crate::types::Kind;

/// The default limit on the number of nodes of an expression.
pub const DEFAULT_MAX_NODES: u64 = 10_000;

/// The default memory budget of a run, counted in allocated elements.
pub const DEFAULT_MEMORY_BUDGET: u64 = 1_000_000;

/// The configuration a program is compiled with.
#[derive(Clone, Debug)]
pub struct Config {
    /// The nature of the environment; unknown when no environment was given.
    pub env: Nature,
    /// The kind the result must have (`Kind::Invalid`: any).
    pub expect: Kind,
    /// Whether a result of unknown type satisfies `expect`.
    pub expect_any: bool,
    pub optimize: bool,
    /// Whether unknown names are errors.
    pub strict: bool,
    pub short_circuit: bool,
    pub max_nodes: u64,
    /// The builtins that are still available.
    pub builtins: BTreeSet<&'static str>,
    pub disabled: HashSet<String>,
    pub disable_if_operator: bool,
    /// The location `date()` and `now()` use, set by the timezone option.
    pub timezone: Option<Loc>,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            env: Nature::default(),
            expect: Kind::Invalid,
            expect_any: false,
            optimize: true,
            strict: false,
            short_circuit: true,
            max_nodes: DEFAULT_MAX_NODES,
            builtins: builtin::NAMES.iter().copied().collect(),
            disabled: HashSet::new(),
            disable_if_operator: false,
            timezone: None,
        }
    }
}

impl Config {
    /// Whether an environment variable shadows a builtin of the same name.
    pub fn is_overridden(&self, name: &[u8]) -> bool {
        self.env.get(name).is_some()
    }

    pub fn is_disabled(&self, name: &str) -> bool {
        self.disabled.contains(name)
    }
}
