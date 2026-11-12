#!/usr/bin/env node
// Compare analyses of identical source trees, even in different checkout roots.
import { createReadStream } from 'node:fs';
import { createInterface } from 'node:readline';
import { fileURLToPath } from 'node:url';
import { resolve } from 'node:path';

const kinds = new Set(['Class', 'Function', 'Method', 'Property', 'Constant', 'Import', 'Type']);
const memberKinds = new Set(['Method', 'Property', 'Constant']);

export async function readDump(path) {
    const rows = new Map();
    let line = 0;
    const input = createReadStream(path, { encoding: 'utf8' });
    const lines = createInterface({ input, crlfDelay: Infinity });
    try {
        for await (const text of lines) {
            line += 1;
            const row = JSON.parse(line === 1 ? text.replace(/^\uFEFF/, '') : text);
            if (row.version !== 1 || typeof row.path !== 'string' || !row.path
                || typeof row.name !== 'string' || !kinds.has(row.kind)
                || !Number.isSafeInteger(row.start) || row.start < 0
                || !Number.isSafeInteger(row.end) || row.end < row.start
                || !Number.isSafeInteger(row.line) || row.line < 1
                || !Number.isSafeInteger(row.column) || row.column < 0
                || !Array.isArray(row.targets) || row.targets.some(target => typeof target !== 'string' || !target)) {
                throw new Error('invalid resolution record (expected schema version 1)');
            }
            const key = JSON.stringify([row.path, row.start, row.end, row.kind, row.name]);
            if (rows.has(key)) throw new Error('duplicate reference');
            row.targets = [...new Set(row.targets)].sort();
            rows.set(key, row);
        }
    } catch (error) {
        throw new Error(`${path}:${line || 1}: ${error.message}`, { cause: error });
    } finally {
        lines.close();
        input.destroy();
    }
    return rows;
}

export function coverage(rows) {
    const all = { resolved: 0, total: 0 };
    const members = { resolved: 0, total: 0 };
    for (const row of rows.values()) {
        all.total += 1;
        all.resolved += Number(row.targets.length > 0);
        if (memberKinds.has(row.kind)) {
            members.total += 1;
            members.resolved += Number(row.targets.length > 0);
        }
    }
    for (const counts of [all, members]) {
        counts.percent = counts.total ? 100 * counts.resolved / counts.total : null;
    }
    return { all, members };
}

export function compare(before, after) {
    const result = {
        coverage: { before: coverage(before), after: coverage(after) },
        lost: [], changed: [], gained: [], removed: [], added: [],
    };
    const change = (oldRow, newRow) => {
        const row = newRow ?? oldRow;
        return {
            path: row.path, start: row.start, end: row.end, line: row.line,
            column: row.column, kind: row.kind, name: row.name,
            before: oldRow?.targets ?? [], after: newRow?.targets ?? [],
        };
    };
    // Sorting keys makes the report independent of the dump's input order.
    for (const key of [...new Set([...before.keys(), ...after.keys()])].sort()) {
        const oldRow = before.get(key);
        const newRow = after.get(key);
        if (!oldRow) result.added.push(change(null, newRow));
        else if (!newRow) result.removed.push(change(oldRow, null));
        else if (JSON.stringify(oldRow.targets) !== JSON.stringify(newRow.targets)) {
            const bucket = !newRow.targets.length ? 'lost' : !oldRow.targets.length ? 'gained' : 'changed';
            result[bucket].push(change(oldRow, newRow));
        }
    }
    return result;
}

function formatCoverage(counts) {
    return `${counts.resolved}/${counts.total} (${counts.percent === null ? 'n/a' : counts.percent.toFixed(2) + '%'})`;
}

async function main(args) {
    const json = args.includes('--json');
    const paths = args.filter(arg => arg !== '--json');
    if (paths.length !== 2 || paths.some(arg => arg.startsWith('--'))) {
        console.error('Usage: node scripts/compare-resolutions.mjs BEFORE.jsonl AFTER.jsonl [--json]');
        return 2;
    }
    try {
        const result = compare(await readDump(paths[0]), await readDump(paths[1]));
        if (json) console.log(JSON.stringify(result, null, 2));
        else {
            console.log(`References: ${formatCoverage(result.coverage.before.all)} -> ${formatCoverage(result.coverage.after.all)}`);
            console.log(`Members:    ${formatCoverage(result.coverage.before.members)} -> ${formatCoverage(result.coverage.after.members)}`);
            console.log(`Targets: ${result.lost.length} lost, ${result.changed.length} changed, ${result.gained.length} gained`);
            console.log(`References: ${result.removed.length} removed, ${result.added.length} added`);
            for (const category of ['lost', 'changed', 'gained', 'removed', 'added']) {
                for (const row of result[category].slice(0, 20)) {
                    console.log(`${category}: ${row.path}:${row.line}:${row.column} ${row.name}: [${row.before.join(', ')}] -> [${row.after.join(', ')}]`);
                }
                if (result[category].length > 20) console.log(`... ${result[category].length - 20} more; use --json for all references`);
            }
        }
        return result.lost.length || result.changed.length || result.removed.length ? 1 : 0;
    } catch (error) {
        console.error(error.message);
        return 2;
    }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
    process.exitCode = await main(process.argv.slice(2));
}
