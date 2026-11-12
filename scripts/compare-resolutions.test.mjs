import assert from 'node:assert/strict';
import { mkdtemp, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { readDump, compare, coverage } from './compare-resolutions.mjs';

const record = (start, targets, kind = 'Method') => ({
    version: 1, path: 'src/사용.php', start, end: start + 4, line: 3, column: start,
    kind, name: 'save', targets,
});
const write = (path, rows) => writeFile(path, rows.map(row => JSON.stringify(row)).join('\n') + (rows.length ? '\n' : ''));

test('compares target sets and keeps missing references visible in coverage', async () => {
    const dir = await mkdtemp(join(tmpdir(), 'phpantom-resolution-'));
    try {
        const before = join(dir, 'before.jsonl');
        const after = join(dir, 'after.jsonl');
        await write(before, [record(1, ['A::save']), record(2, ['A::save', 'B::save']), record(3, []), record(4, []), record(5, ['Thing'], 'Class')]);
        await write(after, [record(2, ['A::save']), record(1, []), record(3, ['B::save']), record(5, ['Thing'], 'Class'), record(6, [])]);
        const result = compare(await readDump(before), await readDump(after));
        for (const key of ['lost', 'changed', 'gained', 'removed', 'added']) assert.equal(result[key].length, 1, key);
        assert.deepEqual(result.changed[0].before, ['A::save', 'B::save']);
        assert.deepEqual(result.coverage.before.members, { resolved: 2, total: 4, percent: 50 });
        const cli = spawnSync(process.execPath, [fileURLToPath(new URL('./compare-resolutions.mjs', import.meta.url)), before, after, '--json'], { encoding: 'utf8' });
        assert.equal(cli.status, 1, cli.stderr);
        assert.deepEqual(JSON.parse(cli.stdout), result);
        await write(after, [record(5, ['Thing'], 'Class'), record(4, []), record(3, []), record(2, ['B::save', 'A::save', 'A::save']), record(1, ['A::save'])]);
        const unchanged = compare(await readDump(before), await readDump(after));
        for (const key of ['lost', 'changed', 'gained', 'removed', 'added']) assert.equal(unchanged[key].length, 0);
    } finally {
        await rm(dir, { recursive: true, force: true });
    }
});

test('rejects truncated, duplicate, unknown-version and unreadable dumps', async () => {
    const dir = await mkdtemp(join(tmpdir(), 'phpantom-resolution-'));
    try {
        const path = join(dir, 'dump.jsonl');
        for (const rows of [[record(1, []), record(1, [])], [{ ...record(1, []), version: 2 }]]) {
            await write(path, rows);
            await assert.rejects(readDump(path));
        }
        await writeFile(path, '{');
        await assert.rejects(readDump(path), /dump.jsonl:1/);
        await assert.rejects(readDump(join(dir, 'missing')));
        await writeFile(path, '');
        assert.deepEqual(coverage(await readDump(path)).members, { resolved: 0, total: 0, percent: null });
    } finally {
        await rm(dir, { recursive: true, force: true });
    }
});
