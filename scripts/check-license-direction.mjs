#!/usr/bin/env node
// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Enforces the license split from docs/PLAN.md §12.1:
// workspace crates under a permissive license must not depend (directly or
// transitively, via normal or build dependencies) on any GPL/AGPL crate.

import { execFileSync } from 'node:child_process';

const COPYLEFT = /\b(A?GPL|LGPL)-/;

/**
 * True if an SPDX expression only allows copyleft use, i.e. none of its
 * OR-alternatives is free of copyleft licenses. `MIT OR LGPL-2.1` is fine
 * (we pick MIT), `MIT AND GPL-3.0` is not.
 */
function onlyCopyleft(expression) {
	if (!expression) return false;
	const alternatives = expression.replace(/[()]/g, ' ').split(/\s+OR\s+|\//);
	return alternatives.every((alt) => COPYLEFT.test(alt));
}

const metadata = JSON.parse(
	execFileSync('cargo', ['metadata', '--format-version', '1', '--locked'], {
		encoding: 'utf8',
		maxBuffer: 256 * 1024 * 1024
	})
);

const packages = new Map(metadata.packages.map((p) => [p.id, p]));
const nodes = new Map(metadata.resolve.nodes.map((n) => [n.id, n]));

/** Ids of normal + build dependencies of a package (dev-deps don't ship). */
function shippedDeps(id) {
	const node = nodes.get(id);
	if (!node) return [];
	return node.deps
		.filter((d) => d.dep_kinds.some((k) => k.kind === null || k.kind === 'build'))
		.map((d) => d.pkg);
}

let violations = 0;
for (const memberId of metadata.workspace_members) {
	const member = packages.get(memberId);
	const license = member.license ?? '';
	if (!license) {
		console.error(`✗ ${member.name}: no "license" field in Cargo.toml`);
		violations++;
		continue;
	}
	if (onlyCopyleft(license)) continue; // copyleft crates may depend on anything compatible

	const seen = new Set();
	const stack = shippedDeps(memberId).map((id) => [id, [member.name]]);
	while (stack.length > 0) {
		const [id, path] = stack.pop();
		if (seen.has(id)) continue;
		seen.add(id);
		const pkg = packages.get(id);
		const depPath = [...path, pkg.name];
		if (onlyCopyleft(pkg.license)) {
			console.error(
				`✗ ${member.name} (${license}) depends on ${pkg.name} (${pkg.license}): ${depPath.join(' → ')}`
			);
			violations++;
			continue;
		}
		for (const dep of shippedDeps(id)) stack.push([dep, depPath]);
	}
}

if (violations > 0) {
	console.error(`\n${violations} license direction violation(s). See docs/PLAN.md §12.1.`);
	process.exit(1);
}
console.log('✓ License direction OK: permissive crates have no copyleft dependencies.');
