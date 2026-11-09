# PHP target compatibility fixtures

Run the PHP-independent checks with:

```sh
cargo test --locked --test legacy_compatibility
```

The suite creates isolated projects with explicit legacy and modern PHP
targets. It uses the existing integration helpers and the real LSP
handlers, awaits both native diagnostic phases before reading the pull
report, and checks both LF and CRLF source. A nonzero result ID alone is
insufficient because the fast phase also publishes one. Trait alias
navigation checks the declaration location, not merely the absence of
diagnostics. This does not depend on the proposed project-wide semantic
export conformance mode.

The immutable [audit](../../docs/compatibility/php55-php56-audit.json)
supplies the 112 source samples and reference PHP 5.5/5.6 lint expectations.
Its historical PHPantom observations are **not** golden test results.
[`expectations.json`](expectations.json) independently declares the expected
hover types and deterministic runtime values for the active controls.
The runner also applies generated constructor/accessor edits and both
document and range formatting edits before exporting them for execution.
Formatting controls must remain idempotent.

[`formatting.json`](formatting.json) declares independent runtime expectations
for version-sensitive expressions, explicit parentheses, and lexical controls.
Every applicable case runs with LF and CRLF, before formatting and after both
document and selection formatting. Selection checks also preserve surrounding
lines. Parser checks cover the PHP 7.4/8.0 boundary, the modern default, and
byte spans after non-ASCII text; custom formatter parser settings must retain
the selected target.

For a manual check, set `[php] version = "5.5"` in `.phpantom.toml` and format
`<?php $probe = '10' . 1 + 2;`. On PHP 5 its result remains integer `103`.
With an `8.5` target the same expression returns string `"103"`, which formatting
must also preserve. The patched dependency sources and their provenance are
documented in [`third_party/mago`](../../third_party/mago/README.md).

## Indirect names

[`indirection.json`](indirection.json) uses the declarations in
[`indirection.php`](indirection.php) to distinguish PHP 5 binding from PHP 7/8.
The suite checks inferred values, direct receiver completion and diagnostics,
and formatting idempotence. The `indirection-5.5.json` and `indirection-5.6.json`
exports execute both the original and formatted sources with LF and CRLF.
Unknown names remain conservative; reads consider at most 16 distinct literal
names and 32 levels of indirection. Writes through dynamic names, side effects
inside selectors and indirect `global` imports remain tracked in the backlog.

## Loose comparisons

[`comparisons.json`](comparisons.json) records equality outcomes for PHP 5,
7 and 8. The shared comparison engine and reference runtime exports use
these expectations; LSP tests independently check branch types, strict
controls, `in_array()` and `switch`. Exports named `comparisons-5.5.json`
and `comparisons-5.6.json` run in the pinned runtime CI jobs.

PHP 5 accepts hexadecimal numeric strings; PHP 7 removes that behavior.
PHP 8 changes number/string comparison and accepts trailing whitespace
in numeric strings. See the official [PHP 7 migration guide](https://www.php.net/manual/en/migration70.incompatible.php)
and [PHP 8 comparison RFC](https://wiki.php.net/rfc/string_to_number_comparison).
Where 32-bit and 64-bit PHP 5 integers give different answers, analysis
keeps both possibilities because the project target does not specify an
architecture. Non-finite or unrepresentable values also remain uncertain.

## Legacy source forms

The source tests cover legacy identifiers, offsets and comments, mixed HTML,
short/ASP/script tags, incomplete edits, Unicode and CRLF positions, and
cross-file discovery with and without Composer. Modern syntax and stub
availability stay covered separately. Short tags remain enabled; PHP 5
analysis assumes ASP tags are enabled, without reading a host `php.ini`.

`PHPANTOM_COMPAT_OUTPUT` also exports `sources-5.5.json` and `sources-5.6.json`.
Pass each to `scripts/check-php-compatibility.mjs` with its matching reference
runtime to check the source and formatted result. These checks execute the
legacy forms that cannot run directly in the modern `examples/php` playground.

## Source syntax capabilities

[`syntax.json`](syntax.json) pairs source constructs with their introduction
versions. The suite checks their boundaries from PHP 5.5 through 8.5, the
original audit's lint outcomes, CLI/LSP agreement, editor navigation and
UTF-16 positions. Legacy class-like type names and PHPDoc remain separate
from syntax capability checks. A `#[` line on PHP 5 is a comment, not an
unsupported attribute.

Boundaries follow PHP's [5.6 migration guide](https://www.php.net/manual/en/migration56.new-features.php)
and the subsequent release migration guides through
[8.5](https://www.php.net/manual/en/migration85.new-features.php).
`PHPANTOM_COMPAT_OUTPUT` exports `syntax-5.5.json`, `syntax-5.6.json`, and
the later target matrices for the same reference-runtime runner. These are
lint expectations; language acceptance does not establish runtime type
inference or availability of an extension.

## Native type hints

Native hints are interpreted using the source file's target, including class
names that only became built-in types in later PHP releases. The tests keep
global and namespaced classes, imported aliases, closures, and cross-file
callers separate from PHPDoc primitives and modern stub declarations. Member
completion, hover, definition, and argument diagnostics use the same types.

The optional export adds `native-hints-5.5.json` and `native-hints-5.6.json`.
Run these with the reference-runtime checker to verify both accepted objects
and rejected scalar arguments. PHP 5's recoverable type errors are converted
to exceptions in the rejection probes so their outcome can be asserted.

## Legacy constructors

Constructor checks distinguish `new` from an ordinary method call: an override
of a parent's class-name method does not replace that parent's constructor.
The matrix covers inheritance, namespace boundaries, trait aliases, visibility,
and the PHP 8 removal boundary. Cross-file tests check property types, class
templates, and reference outputs through the shared type engine.
Version boundaries follow the [constructor removal RFC](https://wiki.php.net/rfc/remove_php4_constructors).

The reference runner accepts `constructors-5.5.json` and `constructors-5.6.json`
exported by `PHPANTOM_COMPAT_OUTPUT`. Reflection verifies the selected parameter
names against PHP 5.5.38 and 5.6.40. Explicit `parent::__construct()` dispatch to
a legacy constructor remains a separate backlog item.

## Built-in availability

`cargo test --test integration builtin_availability` checks introduction and
removal boundaries with actual embedded stubs and fresh sessions in both
version directions. Completion, hover and diagnostics agree; installed stub
navigation, project polyfills, custom stub precedence and CLI analysis have
separate controls. The metadata normalizer also tests scopes, aliases, case,
duplicate variants and CRLF.

The bundled stubs omit an introduction tag for `PHP_INT_MIN`. Its PHP 7.0
boundary is supplied by a provenance-scoped correction based on the official
[PHP 7.0 constants migration guide](https://www.php.net/manual/en/migration70.constants.php).
This does not establish whether an optional extension is installed, and
package `@since` tags do not select a PHP release.

## Reference runtime checks

Export fixtures while running the Rust checks, then execute them with an
explicitly selected PHP binary and exact patch version. For example, in
PowerShell:

```powershell
$env:PHPANTOM_COMPAT_OUTPUT = Join-Path $env:TEMP 'phpantom-compatibility-output'
cargo test --locked --test legacy_compatibility
node scripts/check-php-compatibility.mjs "$env:PHPANTOM_COMPAT_OUTPUT/5.5.json" C:/php55/php.exe 5.5.38
node scripts/check-php-compatibility.mjs "$env:PHPANTOM_COMPAT_OUTPUT/5.6.json" C:/php56/php.exe 5.6.40
```

On Unix, set `PHPANTOM_COMPAT_OUTPUT=/tmp/phpantom-compatibility-output`
before the cargo command and pass the appropriate PHP executable paths.
The same runner accepts the exported `7.4.json` and `8.5.json` controls,
with a matching exact runtime version. No PHP executable is needed by the
ordinary Rust suite, and exporting does not execute PHP.

The optional [Legacy PHP compatibility workflow](../../.github/workflows/legacy-php.yml)
runs on manual dispatch. It downloads the official PHP 5.5.38 and 5.6.40
Windows archives, verifies their SHA-256 hashes, installs the matching
[Microsoft VC11 dependency](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist/),
and checks the generated fixtures. Reference PHP runs with `-n`, UTC,
and short tags enabled; PHP 5 also enables ASP tags, matching the baseline. Each subprocess
has a timeout. Lint, runtime, and pending expectation counts are separate.

## Target selection and restart checks

The `php_target` unit tests and `php_version` integration tests cover
configuration precedence, Composer ranges, restoration of filtered stubs,
and a sequence of fresh 5.5, 8.5, 5.6, and 5.5 sessions. They compare CLI
and LSP formatting and check version-sensitive types and generated setters.
Config and root Composer file changes keep the active target unchanged;
restarting applies the requested target. See the
[configuration contract](../../docs/configuration.md#php).

For a repeatable local cost sample, run:

```sh
cargo test --test integration php_target_cost_sample -- --ignored --nocapture
```

This constructs a backend with the embedded stubs, initializes a temporary
project, opens a `DateTime` example, then measures 100 warm hover requests.
It reports startup time, median and 95th percentile hover time, and cache
entry counts. Startup excludes compilation and document opening. The test
is ignored in normal runs because timings are observations, not assertions.

One local run on 2026-11-09 used Rust 1.99.1, an unoptimized Linux Docker
build with debug info disabled, version-aware source parsing, and the
repository's embedded stubs:

| Composer requirement | Startup (ms) | Hover p50 / p95 (ms) | Indexed stub functions / constants |
| --- | ---: | ---: | ---: |
| `5.5` | 32.7 | 0.38 / 0.49 | 4,906 / 7,640 |
| `8.5` | 30.2 | 0.38 / 0.46 | 4,999 / 8,085 |
| `5.6` | 29.8 | 0.33 / 0.45 | 4,924 / 7,640 |
| `^8.0` or `^5.5` | 47.7 | 0.38 / 0.48 | 4,906 / 7,640 |

The legacy cases retained six class entries and the modern case retained
15, with zero function entries and two parsed files in each lazy cache.
These counts distinguish parsed caches from the
embedded symbol indexes; they are not heap-byte estimates. The snapshot
is a smoke measurement, not a release benchmark or a comparison with an
earlier build. Index counts alone do not establish complete PHP compatibility.

## Known gaps and extending coverage

`destructuring.json` checks the shared `list()`/`foreach` binder against
PHP 5.5/5.6 and modern targets, including array writes, nested patterns,
live source-variable reads, and legacy string unpacking. Exports retain
LF/CRLF and formatted variants; `destructuring-empty-*.json` also checks
version-specific syntax failures. The rules follow the
[PHP 7 migration guide](https://www.php.net/manual/en/migration70.incompatible.php)
and PHP 5.6's
[list compilation](https://github.com/php/php-src/blob/PHP-5.6/Zend/zend_compile.c),
with native runtime checks deciding the expected values. No external
implementation code is copied. Target expressions with scope effects and
aliasing remain in LP22 of the compatibility backlog.

The ignored Rust test asserts the desired unsupported-version diagnostics.
Run it explicitly with `cargo test --test legacy_compatibility -- --ignored`
when implementing them, then remove its ignore attribute. It is not passing
coverage. The unsupported-version check is provisional until the server
has a diagnostic code for it; replace its message assertion with that code
when implementing the diagnostic.

Formatting runtime expectations are required; none has a pending marker.
The runner can report a pending value mismatch separately when a future
fixture explicitly links to an outstanding task. A syntax failure, crash,
or unexpected pass still fails the run.

All 112 original sources receive reference lint checks on each legacy
runtime. Active LSP controls cover hover, diagnostics, completion,
constructor signature help, trait navigation, formatting, and
constructor/accessor generation; they do not claim full
feature coverage for every audit source. Expand the controls alongside the
remaining tasks in [the compatibility backlog](../../docs/todo/legacy-php.md).
Broader call-resolution assertions and the remaining LSP
surfaces belong to that qualification work. Keep nondeterministic values
(dates, random hashes) and extension-dependent side effects out of runtime
expectations. A fixture accepted by PHP lint need not execute successfully,
and neither fact proves the language server inferred its types correctly.
