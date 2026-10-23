<h1><a href="https://expr-lang.org"><img src="https://expr-lang.org/img/logo.png" alt="Zx logo" height="48"align="right"></a> Expr</h1>

[![test](https://github.com/expr-lang/expr/actions/workflows/test.yml/badge.svg)](https://github.com/expr-lang/expr/actions/workflows/test.yml)

**Expr** is an expression language, implemented here in Rust, designed to deliver dynamic configurations with unparalleled accuracy, safety, and speed. 
**Expr** combines simple [syntax](https://expr-lang.org/docs/language-definition) with powerful features for ease of use:

```js
// Allow only admins and moderators to moderate comments.
user.Group in ["admin", "moderator"] || user.Id == comment.UserId
```

```js
// Determine whether the request is in the permitted time window.
request.Time - resource.Age < duration("24h")
```

```js
// Ensure all tweets are less than 240 characters.
all(tweets, len(.Content) <= 240)
```

## Features

**Expr** is a safe, fast, and intuitive expression evaluator. 
Here are its standout features:

### Safety and Isolation
* **Memory-Safe**: Expr is designed with a focus on safety, ensuring that programs do not access unrelated memory or introduce memory vulnerabilities.
* **Side-Effect-Free**: Expressions evaluated in Expr only compute outputs from their inputs, ensuring no side-effects that can change state or produce unintended results.
* **Always Terminating**: Expr is designed to prevent infinite loops, ensuring that every program will conclude in a reasonable amount of time.

### Command Line
* **One binary**: `expr` compiles and runs an expression against a JSON environment and prints the result, its type or the error.

### Static Typing
* Ensures type correctness and prevents runtime type errors.
  ```
  $ expr -env '{"name": "Ann", "age": 31}' 'name + age'
  invalid operation: + (mismatched types string and int) (1:6)
   | name + age
   | .....^
  ```

### User-Friendly
* Provides user-friendly error messages to assist with debugging and development.

### Flexibility and Utility
* **Rich Operators**: Offers a reasonable set of basic operators for a variety of applications.
* **Built-in Functions**: Functions like `all`, `none`, `any`, `one`, `filter`, and `map` are provided out-of-the-box.

### Performance
* **Optimized for Speed**: Expr stands out in its performance, utilizing an optimizing compiler and a bytecode virtual machine.

## Install

```
cargo build --release
```

The binary is `target/release/expr`; the library crate is `expr`.

## Documentation

* See the [Language Definition](docs/language-definition.md) to learn the syntax.
* `expr -h` lists the command's flags: the environment (`-env`, `-env-file`), the expression file (`-file`), the
  compile options (`-allow-undefined`, `-as`, `-warn-on-any`, `-no-optimize`, `-no-short-circuit`, `-disable-if`,
  `-disable`, `-enable`, `-timezone`, `-max-nodes`), `-check` and the output format (`value`, `typed`, `json`).

## Examples

```
$ expr -env '{"names": ["world", "you"]}' '"Hello, " + names[0] + "!"'
Hello, world!

$ expr -env '{"tweets": [{"len": 42}, {"len": 98}, {"len": 69}]}' 'all(tweets, .len <= 240)'
true

$ expr -output typed 'map(1..3, # * 1.5)'
[]interface {} [1.5 3 4.5]
```

From Rust:

```rust
use expr::{compile, run, Options};
use expr::value::{sprint_v, Value};

let program = compile(b"sum(1..10)", &Options::default()).unwrap();
let out = run(&program, &Value::Nil).unwrap();
assert_eq!(sprint_v(&out), b"55");
```

## Testing

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## Who uses Expr?

* [Google](https://google.com) uses Expr as one of its expression languages on the [Google Cloud Platform](https://cloud.google.com).
* [Uber](https://uber.com) uses Expr to allow customization of its Uber Eats marketplace.
* [GoDaddy](https://godaddy.com) employs Expr for the customization of its GoDaddy Pro product.
* [ByteDance](https://bytedance.com) incorporates Expr into its internal business rule engine.
* [Aviasales](https://aviasales.ru) utilizes Expr as a business rule engine for its flight search engine.
* [Alibaba](https://alibaba.com) uses Expr in a web framework for building recommendation services.
* [Argo](https://argoproj.github.io) integrates Expr into Argo Rollouts and Argo Workflows for Kubernetes.
* [Wish.com](https://www.wish.com) employs Expr in its decision-making rule engine for the Wish Assistant.
* [OpenTelemetry](https://opentelemetry.io) integrates Expr into the OpenTelemetry Collector.
* [Philips Labs](https://github.com/philips-labs/tabia) employs Expr in Tabia, a tool designed to collect insights on their code bases.
* [CrowdSec](https://crowdsec.net) incorporates Expr into its security automation tool.
* [CoreDNS](https://coredns.io) uses Expr in CoreDNS, which is a DNS server.
* [qiniu](https://www.qiniu.com) implements Expr in its trade systems.
* [Junglee Games](https://www.jungleegames.com/) uses Expr for its in-house marketing retention tool, Project Audience.
* [Faceit](https://www.faceit.com) uses Expr to enhance customization of its eSports matchmaking algorithm.
* [Chaos Mesh](https://chaos-mesh.org) incorporates Expr into Chaos Mesh, a cloud-native Chaos Engineering platform.
* [Visually.io](https://visually.io) employs Expr as a business rule engine for its personalization targeting algorithm.
* [Akvorado](https://github.com/akvorado/akvorado) utilizes Expr to classify exporters and interfaces in network flows.
* [keda.sh](https://keda.sh) uses Expr to allow customization of its Kubernetes-based event-driven autoscaling.
* [Span Digital](https://spandigital.com/) uses Expr in its Knowledge Management products.
* [Xiaohongshu](https://www.xiaohongshu.com/) combining yaml with Expr for dynamically policies delivery.
* [Melrōse](https://melrōse.org) uses Expr to implement its music programming language.
* [Tork](https://www.tork.run/) integrates Expr into its workflow execution.
* [Critical Moments](https://criticalmoments.io) uses Expr for its mobile realtime conditional targeting system.
* [WoodpeckerCI](https://woodpecker-ci.org) uses Expr for [filtering workflows/steps](https://woodpecker-ci.org/docs/usage/workflow-syntax#evaluate).
* [FastSchema](https://github.com/fastschema/fastschema) - A BaaS leveraging Expr for its customizable and dynamic Access Control system.
* [WunderGraph Cosmo](https://github.com/wundergraph/cosmo) - GraphQL Federeration Router uses Expr to customize Middleware behaviour
* [SOLO](https://solo.one) uses Expr interally to allow dynamic code execution with custom defined functions.
* [Naoma.AI](https://www.naoma.ai) uses Expr as a part of its call scoring engine.
* [GlassFlow.dev](https://github.com/glassflow/clickhouse-etl) uses Expr to do realtime data transformation in ETL pipelines

[Add your company too](https://github.com/expr-lang/expr/edit/master/README.md)

## License

[MIT](https://github.com/expr-lang/expr/blob/master/LICENSE)

