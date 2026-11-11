# PHPantom — Bug Fixes

Every bug below must be fixed at its root cause. "Detect the
symptom and suppress the diagnostic" is not an acceptable fix.
If the type resolution pipeline produces wrong data, fix the
pipeline so it produces correct data. Downstream consumers
(diagnostics, hover, completion, definition) should never need
to second-guess upstream output.

Each entry below carries an **Impact · Complexity** rating using the same
scale defined in [`docs/todo.md`](../todo.md), but a bug's row lives
**here only** — do not add or link a bug entry to `docs/todo.md`'s sprint
or backlog tables. This file is its own list, not a domain document
sprint items draw from: whenever it holds anything, that is actively
addressed, independently of sprint planning.

Bugs land here from wherever they surface: found while working on another
task, or sweeps of the sample projects under `projects/`. Entries are
grouped by the mechanism that has to change, not by the symptom that
surfaced: one entry is one root cause, however many shapes it shows up in.

## Crashes

No outstanding items.

## Type comparison

No outstanding items.

## Conditional return types

No outstanding items.

## Standard-library return types

No outstanding items.

## Reachability

No outstanding items.

## Narrowing

No outstanding items.

## Array types

### B1. Line comments in a multiline PHPDoc type swallow later fields

**Impact: Medium-High · Complexity: Medium**

`@psalm-type Config = array{` with `name: string, // description` on one
line and `enabled: bool` on the next parses correctly in the PHPDoc CST,
but the stored alias becomes a raw, unstructured type. `type_text` folds
the lines together before `PhpType::parse` reads the type again, so the
line comment consumes the remaining fields and closing brace. Generated
Symfony configuration aliases commonly contain these comments.

Preserve the comment boundaries or consume the parsed type without
reconstructing an invalid single-line string. Cover aliases, return and
parameter types, quoted `//` literals, CRLF, and source spans used by
navigation. Do not suppress downstream diagnostics or discard fields.

**Where to look:** `src/docblock/parser.rs` (`type_text`),
`src/docblock/templates.rs` (`extract_type_aliases_from_info`), and
`src/php_type/parse.rs`.

## Laravel

No outstanding items.

## Blade

No outstanding items.

## Templates

No outstanding items.

## Framework names in strings

No outstanding items.

## Name resolution

No outstanding items.

## Code actions

No outstanding items.

## Miscellaneous

No outstanding items.
