// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// The app is a pure single-page app: it runs inside the Tauri webview and is
// served as static files by sy-server. There is no Node server, so no SSR.
export const ssr = false;
export const prerender = false;
