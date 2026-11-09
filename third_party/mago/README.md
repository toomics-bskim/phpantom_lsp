# Mago dependency patches

`syntax/` and `formatter/` contain the production sources of the
`mago-syntax` and `mago-formatter` 1.58.2 crates, respectively. The root
Cargo manifest patches both crates so transitive users share the same CST.
All other Mago crates still come from crates.io.

Upstream: [carthage-software/mago](https://github.com/carthage-software/mago),
commit [`59c3a1b56df9916cff773f1e49d89f929ceda790`](https://github.com/carthage-software/mago/tree/59c3a1b56df9916cff773f1e49d89f929ceda790).
The published crate archive SHA-256 hashes are:

| Crate | SHA-256 |
| --- | --- |
| mago-syntax 1.58.2 | `d36dc15d1cec650a36f6099ab4f7ec18fa8266f6f0677eed7831a55dfe95adc8` |
| mago-formatter 1.58.2 | `593a7043aed31970138ac36e6a65872192983189dced150959fb9f00f5ec1650` |

Only `src/` and the published manifests are copied. Upstream test/benchmark
targets and development dependencies are omitted from these manifests;
their fixtures are not vendored. The upstream MIT and Apache 2.0 licenses
are preserved in [LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE).

## Local changes

- `syntax/src/settings.rs` and `syntax/src/parser/mod.rs`: carry the source
  PHP version in the parser, defaulting to the upstream modern version.
- `syntax/src/token/mod.rs`: map concatenation precedence to addition and
  subtraction precedence before PHP 8.
- `syntax/src/parser/internal/expression.rs`: use that mapping when binding
  infix operators and parsing concatenation's right operand.
- `formatter/src/lib.rs`: use the formatter's target for parsing, including
  when callers supply custom parser settings. Range formatting shares it.
- `formatter/src/internal/parens.rs`: use the same precedence when deciding
  whether parentheses are needed.

The change follows the [PHP concatenation precedence RFC](https://wiki.php.net/rfc/concatenation_precedence).
It does not yet make other source parsing or type inference version-aware.
There is no extra parsing pass or post-format text replacement.

Regression coverage lives in `tests/legacy_compatibility.rs` and
`tests/compatibility/formatting.json`; see the
[runtime check instructions](../../tests/compatibility/README.md).
When upgrading Mago, compare these six files with the new upstream source,
retain the patch until upstream provides equivalent behavior, and run the
formatting and reference-runtime checks before removing the overrides.
