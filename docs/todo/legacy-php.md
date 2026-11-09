# PHPantom — PHP 5.5 & 5.6

Work needed to make the language server reliable for projects targeting
PHP 5.5 and 5.6. The server is a native Rust executable; the target PHP
version describes the source it understands, not a runtime needed to
start the server. CodeIgniter integration is a separate feature set.

The baseline supports substantial older PHP syntax and some versioned
stubs, but setting `[php] version = "5.5"` or `"5.6"` does not currently
provide complete language compatibility. Parsing, runtime semantics,
built-in symbols, diagnostics, and generated edits need separate work.

## Verification baseline

Rechecked on 2026-11-09 against commit
`9d1b18330282ccae6940351d9388f988b176be6a`, PHPantom 0.11.1,
Mago 1.58.2, and phpstorm-stubs commit
`2049ca3cfa1feabf52db074108fe776a5713cf02`.
The [audit evidence](../compatibility/php55-php56-audit.json) contains
112 source samples with separate PHP 5.5.38 and 5.6.40 observations.
These are 224 sample/version checks, not a percentage of PHP conformance.

Both projects explicitly selected their PHP version in `.phpantom.toml`.
Reference lint and selected runtime checks used the actual Windows x64
PHP binaries, with `-n`, `short_open_tag=1`, and `asp_tags=1`; runtime
probes also selected UTC. The same sources were opened through JSON-RPC
in the built server. The baseline treated a nonzero diagnostic result ID
as ready. The fast phase can publish that ID before the slow phase
finishes, so its diagnostic counts are historical observations, not proof
of complete diagnostic coverage. The regression fixture runner awaits
both native phases before inspecting the pull report.
CLI `analyze` results were also collected for both projects. Hover,
completion, definition, signature help, formatting, and code actions
were exercised where relevant to the sample. Rename, references,
semantic tokens, range formatting, and configuration reload were not
exhaustively exercised; LP18 includes the remaining qualification.

| Observation | PHP 5.5.38 | PHP 5.6.40 |
| --- | --- | --- |
| Samples accepted by reference `php -l` | 73 / 112 | 87 / 112 |
| Valid samples with PHPantom `syntax_error` | 3 | 3 |
| Samples rejected by reference `php -l` | 39 | 25 |
| Rejected samples with no PHPantom diagnostic at all | 37 | 23 |
| Rejected samples with only unrelated diagnostics | 2 | 2 |
| Generated actions rejected by reference `php -l` | 6 / 8 | 6 / 8 |
| Confirmed formatting change to a deterministic runtime result | 1 | 1 |

The three false syntax errors are curly-brace offsets, a class named
`Match`, and an ordinary line comment starting with `#[`. ASP tags are
another gap: the reference lexer accepts them with `asp_tags=1`, while
PHPantom supplies no type and no formatting edit. A silent document is
not evidence that PHP code inside the tags was understood.
None of the 39 / 25 invalid-source snapshots contains a target-version diagnostic;
the two unrelated reports concern an unknown class and an uninitialized
property. Valid syntax alone also says nothing about runtime correctness.

Namespace aliases, trait alias navigation, array syntax, references,
PHPDoc-based magic members, `finally`, `foreach` with `list()`, `::class`,
and expression arguments to `empty()` have working samples. Annotated
generators resolve their yielded values. Legacy stubs retain useful
`mysql_*`, `each()`, `create_function()`, and cURL types; `mysql_*`
deprecation in 5.5 is legitimate. Optional database/cURL extensions were
not loaded for runtime testing, and no database or network operations ran.

Formatting comparisons exclude timestamp-bearing objects and salted
password hashes. Broad but valid types such as `string|false` for
`substr()` and `int|false` for `strpos()` are not failures. The Unicode
escape spelling probe retained its PHP 5 literal value. The one-parameter
`unserialize()` signature matched both reference runtimes. These controls
must remain alongside the failing cases.

## Language boundaries

| Area | PHP 5.5 target | PHP 5.6 target | Current evidence |
| --- | --- | --- | --- |
| Earlier language foundations | Namespaces, closures, traits, short arrays | Same | Representative cases work; returned closures lose their result type |
| PHP 5.5 additions | Generators, `finally`, `foreach` destructuring, `::class`, expanded `empty()` | Same | Annotated generators work; unannotated yield inference has a gap |
| PHP 5.6 additions | Reject variadics, argument unpacking, exponentiation, constant expressions/arrays, function/constant imports | Accept these forms | No 5.5 rejection; 5.6 value probes work except untyped variadic/unpacked calls |
| Later syntax | Reject syntax introduced after 5.5 | Reject syntax introduced after 5.6 | Tested return declarations through property hooks lack version diagnostics |
| Native type names | Resolve names according to PHP 5 rules | Same | `int` is treated as a scalar where the runtime requires a class instance |
| Older runtime semantics | Preserve PHP 5 binding, coercion, and evaluation order | Same | Constructor, comparison, destructuring, and formatting gaps reproduced |
| Built-in availability | Include only symbols available by 5.5 | Include 5.6 additions too | Future functions, classes, constants, and methods leak into both targets |

The boundaries come from the official [PHP 5.3 release](https://www.php.net/releases/5_3_0.php),
[PHP 5.4 release](https://www.php.net/releases/5_4_0.php),
[PHP 5.5 release](https://www.php.net/releases/5_5_0.php), and
[PHP 5.6 new features](https://www.php.net/manual/en/migration56.new-features.php).
Semantic differences are described in the
[PHP 7.0 incompatibilities](https://www.php.net/manual/en/migration70.incompatible.php),
[PHP 7.0 deprecations](https://www.php.net/manual/en/migration70.deprecated.php),
[PHP 7.4 deprecations](https://www.php.net/manual/en/migration74.deprecated.php), and
[PHP 8.0 incompatibilities](https://www.php.net/manual/en/migration80.incompatible.php).
Modern documentation must not be applied to PHP 5 without checking the
version boundary. For example, `iterable $x` parses in PHP 5 as a class
hint; it is not evidence of support for the later primitive type.

## Delivery order

Each numbered item is one implementation task and one PR. LP10–LP17
address the remaining semantic and inference gaps. LP20 covers constructor
access and explicit calls.
LP19 covers the remaining edit producers; LP18 qualifies the final support
claim. Dependencies below are explicit;
items can move into scheduled sprints without changing their scope.

Reproduced defects should receive GitHub issues with the audit sample
and target version attached. This
document records the work; it does not imply issues have been opened.

## LP10. Loose comparisons narrow by the target semantics

**Impact: High · Complexity: High**

**Evidence.** With a `0|'foo'` parameter, the `== 'foo'` branch narrows to
`'foo'`, although both runtimes also enter it for `0`.
`php_type/literal.rs::loosely_equals` explicitly applies PHP 8 rules.

**Plan.** Make literal comparison and every narrowing consumer share the
target's coercion rules, including switch cases, negated conditions,
numeric strings, booleans, and null. Keep strict comparison unchanged.

**Done when.** Runtime-reachable values are never excluded by narrowing,
and positive/negative tests cover PHP 5 and PHP 8 differences without
weakening unrelated diagnostics.

## LP11. Resolve legacy dynamic variable binding

**Impact: Medium-High · Complexity: High**

**Evidence.** `indirect_variable` evaluates `$$names['key']` to an
`AuditIndirect` object, but hover is `mixed` and member completion is
empty. The explicitly braced control also loses the type, so changing
operator binding alone will not resolve the entire failure.

**Plan.** Resolve statically known variable names through the existing
forward walker, then apply the target's indirect-variable binding.
Keep genuinely unknown dynamic names conservative and bounded.

**Done when.** Implicit and explicit binding cases agree with the target
runtime, including property/static-member variants and PHP 7 controls.
Completion and diagnostics consume the same resolved value.

## LP12. Destructuring updates append targets in runtime order

**Impact: Medium-High · Complexity: High**

**Evidence.** `list_append_order` assigns `list($values[], $values[]) =
array(1, 2)`. Both runtimes produce `2` at index zero; PHPantom reports
`null`. This establishes a target-write inference error as well as an
evaluation-order requirement; it does not isolate the cause to order alone.

**Plan.** Update the forward walker's array state for destructuring
targets, with the version's assignment order and nested/keyed targets.

**Done when.** Append and ordinary targets have sound value types in PHP
5 and PHP 7 controls. Empty `list()`, skipped slots, strings, and
`foreach` destructuring retain their target-specific behavior.

## LP13. Legacy constant lookup preserves name rules

**Impact: Medium · Complexity: Medium-High**

**Evidence.** A lowercase lookup of `define('AUDIT_CASE_CONST', 'value',
true)` returns `'value'` on both runtimes but has no inferred hover type.
An undefined bare constant falls back to its name at runtime and also
has no inferred type in the probe.

**Plan.** Preserve case-insensitivity metadata for legacy `define` calls
without making all constants case-insensitive. Model namespace fallback
and undefined bare-name behavior separately from class constants.

**Done when.** Completion, navigation, rename, and value inference agree
for both constant kinds. Diagnose questionable undefined names while
preserving their runtime type and version-appropriate severity; include
modern error behavior and collisions between differently cased names.
Include built-in names unavailable at the selected target, such as
`PHP_INT_MIN` before PHP 7, in the undefined-name diagnostic checks.

## LP14. Built-in argument and return contracts match PHP 5

**Impact: High · Complexity: Medium-High**

**Evidence.** `count_scalar` evaluates `count(42)` to `1` in both
runtimes but receives `type_mismatch_argument`. Retaining an old function
name is insufficient when its signature or behavior has changed.

**Plan.** Correct versioned stub contracts and shared built-in inference,
starting with `count`. Audit resource/object transitions and changed
argument coercion or return values using runtime/reflection controls.
Keep broad valid results such as the tested `substr`/`strpos` unions.

**Done when.** Accepted legacy calls have no false type errors, invalid
calls still diagnose, and PHP 7/8 contracts remain intact. Test extension
APIs through declared stubs without requiring database/network access.

## LP15. Returned closures retain their bound receiver type

**Impact: Medium · Complexity: High**

**Evidence.** `closure_this` returns a closure reading `$this->name`;
the actual call returns `'bound'`, while hover is `mixed`. This is an
inference gap affecting the legacy baseline, not a demonstrated PHP
5-only regression.

**Plan.** Trace the returned callable's signature, captured receiver, and
result through the shared closure/call engine. Preserve declaration
scope rather than borrowing the caller's namespace or receiver.

**Done when.** Returned closures infer their result across call sites,
including static closures and rebinding controls. Coordinate with the
existing closure-scope work in the diagnostics backlog.

## LP16. Unannotated generators infer yielded values

**Impact: Medium · Complexity: High**

**Evidence.** `generator_inferred` yields a known string but its foreach
value is `mixed`. The annotated generator control resolves correctly.
This is not yet established as a legacy-specific defect.

**Plan.** Feed body-derived yield key/value types through the existing
generator and iterable inference paths, retaining explicit PHPDoc
precedence and lazy evaluation.

**Done when.** Unannotated local and cross-file generators infer usable
foreach values with several yield sites. Generator returns and `yield
from` obey the selected version's boundary; body inference does not scan
unrelated functions on every request.

## LP17. Variadic and unpacked calls carry argument types

**Impact: Medium-High · Complexity: High**

**Evidence.** On PHP 5.6, `variadic_value` returns `'first'` and
`unpack_value` returns `'b'`, but neither probe has a hover type.
Both files are correctly rejected by the PHP 5.5 runtime. These samples
combine untyped parameters and return inference: ordinary-call and modern
controls are needed before assigning the root cause to unpacking.

**Plan.** Isolate the missing propagation in the existing call engine,
then map known unpacked shapes and variadic argument packs to parameter
and return types. Include PHPDoc/generic signatures and unknown-length
packs instead of hardcoding the sample's string values.

**Done when.** PHP 5.6 calls produce sound result types and argument
diagnostics; PHP 5.5 still reports the unavailable syntax.
Ordinary calls and modern unpacking keep their behavior.

## LP19. Remaining generated edits respect the target language

**Impact: High · Complexity: Medium-High**

**Plan.** Audit edit producers outside constructor/accessor/property-hook
generation: constructor-parameter promotion, type insertion, method
implementation, extraction, import rewrites, and completion snippets.
Share the language capability checks rather than duplicating version
comparisons. Preserve PHPDoc when a type cannot be written natively.
Check constructor PHPDoc pseudo-types such as `resource`: treating every
simple documented name as a native hint can turn a resource contract into
a class constraint. Add a runtime reproducer before choosing the fix,
and distinguish documented pseudo-types from native class hints.
Define behavior for source that already uses syntax newer than its target;
these edits are not a whole-file transpiler.

**Done when.** Apply each offered edit to version-valid fixtures and lint
the result under both PHP 5 runtimes, with modern boundary controls.
Runtime checks cover edits that can change evaluation order or coercion.
Unavailable transformations are not offered, and supported edits preserve
their meaning. Record a reproducer for every additional failure found.

## LP20. Constructor access and explicit calls follow legacy rules

**Impact: High · Complexity: Medium-High**

**Evidence.** PHP 5.5.38 and 5.6.40 accept a child constructor calling
`parent::__construct(new DateTime())` when the global parent only declares
`function ParentName(DateTime $value) {}`. PHPantom reports
`ParentName::__construct() not found`. An explicit arrow call to
`$object->__construct()` on that same parent really is undefined, so adding
an ordinary method alias would be incorrect. Separately, constructing a child
of a class with a private class-name constructor produces a runtime fatal
error, but no access diagnostic after its arguments are satisfied.

**Plan.** Use the selected constructor and its declaring owner for explicit
scope calls and constructor visibility checks. Preserve ordinary method
lookup for arrow calls, and share the result with signatures, navigation,
and diagnostics. This is deferred from the constructor selection work;
selection and argument inference alone do not establish that a call is legal.

**Done when.** Valid parent/self constructor calls resolve without a false
unknown-member diagnostic. Inaccessible construction is diagnosed with own,
inherited, and trait constructors, with modern `__construct` and PHP 8 controls.
Runtime fixtures distinguish explicit scope calls from ordinary arrow calls.

## LP18. Qualify and document the support contract

**Impact: High · Complexity: Medium · Depends on: LP10–LP17, LP19, LP20**

**Plan.** Expand the regression matrix to remaining language and LSP
surfaces before claiming full support: include/require and manual
autoloading, procedural projects, references/rename, semantic tokens,
error recovery, optional tag modes, version reload, and multi-root
isolation. Check the [PHP 5.6 migration incompatibilities](https://www.php.net/manual/en/migration56.incompatible.php)
as well as new syntax. Record unsupported cases as numbered backlog items rather than
silently dropping them. Reuse modern regression tests and latency/memory
benchmarks to guard the shared engine.

**Done when.** A pinned 5.5/5.6 CI matrix covers source validity, type
claims, diagnostics, and generated/formatted code with modern controls.
Publish configuration precedence, tested patch versions, extension/stub
assumptions, and the remaining limits. Put feature demos in the existing
matching `examples/php/` files with runtime assertions; keep legacy-only
syntax in versioned test fixtures so modern playground lint still works.
Framework-specific CodeIgniter behavior remains a separately scoped task.

## Reproducing the baseline

Materialize each audit case's `source` as `<id>.php` in two temporary
projects, with `[php] version = "5.5"` and `"5.6"` respectively. The
evidence records the official runtime download URLs and archive hashes.
Build the recorded revision with `cargo build --locked --bin phpantom_lsp`.
For each version run:

```text
php -n -d short_open_tag=1 -d asp_tags=1 -l <id>.php
phpantom_lsp analyze --project-root <project> --format json --no-colour
```

Send LSP `initialize`, `initialized`, and `textDocument/didOpen` with the
project URI and each original source. Request diagnostics until the
computed snapshot is ready. The hover position is the final `$probe`
marked `audit-hover`; member completion replaces that expression with
`$probe->`. Request signature help inside the constructor call, definition
on the trait alias call, and code actions on `$name` in `setter`. Apply
each edit to the original source separately, then lint with the matching
runtime. For selected safe runtime probes, the evidence records the
expression wrapper and output. Do not compare nondeterministic values.
See the [compatibility fixture guide](../../tests/compatibility/README.md)
for automated regression checks and the optional reference-runtime job.
