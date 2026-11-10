// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Data transfer types shared by both backends (Tauri desktop and rz-server).
// They mirror the Rust types in `rz-model` (crates/rz-model/src/api.rs).
// Keep both sides in sync until we generate these from Rust.

export type BackendKind = 'desktop' | 'server';

export type GameId = 'iracing' | 'f1_25' | 'lmu';

export interface AppInfo {
	name: string;
	version: string;
	backend: BackendKind;
}

export interface SessionSummary {
	id: string;
	game: GameId;
	trackKey: string;
	carKey: string;
	sessionType: string;
	/** RFC 3339 timestamp */
	startedAt: string;
	lapCount: number;
	bestLapMs: number | null;
}
