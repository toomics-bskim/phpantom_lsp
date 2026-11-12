// Run the exported conformance fixtures against an explicitly selected PHP binary.
// Usage: node scripts/check-php-compatibility.mjs <fixtures.json> <php> <exact-version>
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, readdirSync, rmdirSync, unlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const [fixturePath, php, expectedVersion] = process.argv.slice(2);
assert(fixturePath && php && expectedVersion,
    'Usage: node scripts/check-php-compatibility.mjs <fixtures.json> <php> <exact-version>');
const fixtures = JSON.parse(readFileSync(fixturePath, 'utf8'));
assert.equal(expectedVersion.split('.').slice(0, 2).join('.'), fixtures.target,
    'The PHP runtime must match the fixture target');

function run(args) {
    const legacyOptions = fixtures.target.startsWith('5.') ? ['-d', 'asp_tags=1'] : [];
    const result = spawnSync(php, ['-n', '-d', 'date.timezone=UTC', '-d', 'short_open_tag=1',
        ...legacyOptions, ...args], { encoding: 'utf8', timeout: 10000, maxBuffer: 1024 * 1024 });
    if (result.error) throw result.error;
    assert.equal(result.signal, null, 'PHP was terminated by a signal');
    return result;
}
const version = run(['-r', 'echo PHP_VERSION;']);
assert.equal(version.status, 0, version.stdout + version.stderr);
assert.equal(version.stdout, expectedVersion, 'Use the pinned reference PHP patch version');
assert(fixtures.cases.length > 0, 'An empty fixture set is not a successful run');

const directory = mkdtempSync(join(tmpdir(), 'phpantom-compatibility-'));
const failures = [];
const pending = [];
let linted = 0;
let evaluated = 0;
try {
    for (const [index, fixture] of fixtures.cases.entries()) {
        const path = join(directory, `${index}.php`);
        writeFileSync(path, fixture.source);
        const lint = run(['-l', path]);
        // A malformed edit is always a failure, even for a pending semantic check.
        if ((lint.status === 0) !== fixture.lint) {
            failures.push(`${fixture.id}: expected lint=${fixture.lint}, exit=${lint.status}\n${lint.stdout}${lint.stderr}`);
            continue;
        }
        linted++;
        if (!fixture.runtime) continue;
        assert(fixture.lint, `${fixture.id}: cannot execute a syntax-negative fixture`);
        if (fixture.runtime.expectedExit !== undefined) {
            assert(Number.isInteger(fixture.runtime.expectedExit) && fixture.runtime.expectedExit !== 0);
            assert.equal(typeof fixture.runtime.message, 'string');
            assert(fixture.runtime.message.length > 0);
            const result = run([path]);
            if (result.status !== fixture.runtime.expectedExit
                || !(result.stdout + result.stderr).includes(fixture.runtime.message)) {
                failures.push(`${fixture.id}: expected exit=${fixture.runtime.expectedExit} containing ${fixture.runtime.message}, got exit=${result.status}\n${result.stdout}${result.stderr}`);
            } else {
                evaluated++;
            }
            continue;
        }
        writeFileSync(path, fixture.source + '\necho "\\nPHPANTOM_RESULT:", json_encode(array(gettype('
            + fixture.runtime.probe + '), ' + fixture.runtime.probe + '));\n');
        const result = run([path]);
        if (result.status !== 0) {
            failures.push(`${fixture.id}: runtime exit=${result.status}\n${result.stdout}${result.stderr}`);
            continue;
        }
        const marker = '\nPHPANTOM_RESULT:';
        const start = result.stdout.lastIndexOf(marker);
        assert(start >= 0, `${fixture.id}: missing runtime result`);
        const actual = JSON.parse(result.stdout.slice(start + marker.length));
        let matches = true;
        try { assert.deepEqual(actual, fixture.runtime.expected); } catch { matches = false; }
        if (fixture.pending) {
            if (matches) failures.push(`${fixture.id}: now passes; remove its pending marker and enable the regression (${fixture.pending})`);
            else pending.push(`${fixture.id}: ${JSON.stringify(actual)} != ${JSON.stringify(fixture.runtime.expected)} (${fixture.pending})`);
        } else if (!matches) {
            failures.push(`${fixture.id}: ${JSON.stringify(actual)} != ${JSON.stringify(fixture.runtime.expected)}`);
        } else {
            evaluated++;
        }
    }
} finally {
    for (const name of readdirSync(directory)) unlinkSync(join(directory, name));
    rmdirSync(directory);
}
console.log(`PHP ${expectedVersion}: ${linted} lint expectations, ${evaluated} runtime expectations passed; ${pending.length} pending, ${failures.length} failures.`);
for (const message of pending) console.log(`PENDING ${message}`);
for (const message of failures) console.error(`FAIL ${message}`);
if (failures.length) process.exitCode = 1;
