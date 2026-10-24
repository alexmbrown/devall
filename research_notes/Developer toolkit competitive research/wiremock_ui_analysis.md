# wiremock-ui (https://github.com/alexmbrown/wiremock-ui): style, features and reusable code for /dev/all

Scope: local files only (node_modules, dist, .git internals and the bundled JRE in .tools/ skipped). All source links point into `https://github.com/alexmbrown/wiremock-ui`. Line numbers were checked against the files as of the last commit `63e0ad1` (2026-10-20). The repo has 4 commits, all made on 2026-10-20 by Alex Brown (`alexmbrown`), each co-authored with Claude. Remote: `https://github.com/alexmbrown/wiremock-ui.git` (from `git remote -v`). About 15k lines of TS/TSX/CSS: 18,777 lines including the design kit and scripts.

---

## Q1. Tech stack: framework, language, build, styling, state, routing, editor, icons, fonts, tests, wrapper, backend

### Takeaway
It is a static, browser-only single-page app: **Vite 8 + React 19 + TypeScript 6**. Server state uses **TanStack Query 5** and client state uses **Zustand 5**. The editor is **Monaco** (via `@monaco-editor/react`, bundled locally with no CDN), icons are **Lucide**, and styling is **plain CSS driven by CSS custom-property tokens**. There is no Tailwind, no component library, no router library, no chart library, no backend and no desktop wrapper. It talks to WireMock's `/__admin` REST API directly with `fetch`.

### Cited Findings
- package.json describes it as "Browser-only admin UI for WireMock. Hosted on GitHub Pages, talks directly to your WireMock admin API." (name `wiremock-ui`, version `0.1.0`, `"type": "module"`). See [package.json](https://github.com/alexmbrown/wiremock-ui/blob/main/package.json).
- **Runtime dependencies (exact pins):**
  - `@monaco-editor/react` 4.7.0
  - `@tanstack/react-query` 5.104.5
  - `@tanstack/react-virtual` 3.14.21
  - `clsx` 2.1.1
  - `fflate` 0.8.3
  - `lucide-react` 1.60.0
  - `monaco-editor` 0.57.0
  - `react` / `react-dom` 19.3.1
  - `yaml` ^2.9.1 (lockfile resolves 2.9.1)
  - `zustand` 5.0.15
  - Sources: [package.json](https://github.com/alexmbrown/wiremock-ui/blob/main/package.json), [package-lock.json](https://github.com/alexmbrown/wiremock-ui/blob/main/package-lock.json)
- **Dev dependencies:**
  - `@types/node` ^22.20.5
  - `@types/react` / `@types/react-dom` 19.3.0
  - `@vitejs/plugin-react` 6.1.2
  - `puppeteer-core` ^25.15.0 (screenshots only)
  - `typescript` 6.0.3
  - `vite` 8.3.4
  - `vitest` 5.0.5
  - `wiremock` 3.13.2: an npm package that bundles the standalone jar for local development.
  - Source: [package.json](https://github.com/alexmbrown/wiremock-ui/blob/main/package.json)
- **npm scripts:** `dev` (vite), `build` (`tsc -b && vite build`), `preview`, `typecheck`, `test` (`vitest run`), `wiremock`, `screenshots`, `gen:schema`, `design:kit`, `demo:seed`, `demo:traffic`. See [package.json](https://github.com/alexmbrown/wiremock-ui/blob/main/package.json).
- **TypeScript config:** target ES2022, `strict`, `noUnusedLocals` / `noUnusedParameters`, `moduleResolution: bundler`, alias `@/* → ./src/*`. See [tsconfig.json](https://github.com/alexmbrown/wiremock-ui/blob/main/tsconfig.json).
- **Vite config:**
  - `base: './'` keeps every asset URL relative, so the same build runs on GitHub Pages, any static host, or "dropped into a WireMock server's __files folder and served same-origin".
  - Build target `es2022`, `chunkSizeWarningLimit: 4000`, worker format `es`.
  - Vitest runs in `environment: 'node'` and includes `src/**/*.test.ts`.
  - Source: [vite.config.ts](https://github.com/alexmbrown/wiremock-ui/blob/main/vite.config.ts)
- README's own stack summary: "Vite, React 19, TypeScript, TanStack Query, Zustand, Monaco (bundled locally, no CDN), Lucide icons, fflate, yaml. Styling is plain CSS driven entirely by tokens in `src/styles/tokens.css`." See [README.md:75](https://github.com/alexmbrown/wiremock-ui/blob/main/README.md).
- **Routing:** a hand-written hash router stored in a Zustand store.
  - URL shape `#/<section>/<param>?<query>`; `?server=` is a deep link that selects or adds a server.
  - `history.replaceState` plus a `hashchange` listener.
  - Sources: [src/store/nav.ts:45-77](https://github.com/alexmbrown/wiremock-ui/blob/main/src/store/nav.ts), [src/App.tsx:47-66](https://github.com/alexmbrown/wiremock-ui/blob/main/src/App.tsx)
  - Pages are code-split with `React.lazy` + `Suspense`, and every page is wrapped in an `ErrorBoundary` keyed by section. See [src/App.tsx:17-29, 133-154](https://github.com/alexmbrown/wiremock-ui/blob/main/src/App.tsx).
- **Server state:** QueryClient defaults are `retry: 1`, `refetchOnWindowFocus: true`, `staleTime: 2000`. See [src/main.tsx:8-16](https://github.com/alexmbrown/wiremock-ui/blob/main/src/main.tsx).
- **Client state** lives in Zustand stores:
  - `useProjects` (persisted to localStorage)
  - `usePrefs` (localStorage)
  - `useWorkspace` (open editor tabs, sessionStorage)
  - `useNav`
  - plus small stores for the palette, dialogs and toasts.
  - Sources: [src/store/](https://github.com/alexmbrown/wiremock-ui/blob/main/src/store/projects.ts), [src/components/ui/toast.tsx:20-30](https://github.com/alexmbrown/wiremock-ui/blob/main/src/components/ui/toast.tsx), [src/components/ui/dialogs.tsx:26-37](https://github.com/alexmbrown/wiremock-ui/blob/main/src/components/ui/dialogs.tsx)
- **Monaco setup:**
  - Imported from the npm package with Vite `?worker` imports (editor worker + JSON worker), so it runs offline.
  - Both WireMock JSON schemas (3.x and 4.x) are bundled; the model path prefix (`inmemory://wmui/stub/v3/…`, `…/v4/…`, `inmemory://wmui/message-stub/…`) selects the schema.
  - Themes `wmui-light` / `wmui-dark` take their background from the `--bg-code` CSS variable and are re-applied by a `MutationObserver` on `data-theme`.
  - Source: [src/components/editor/monaco.ts:1-96](https://github.com/alexmbrown/wiremock-ui/blob/main/src/components/editor/monaco.ts)
- **Editor defaults:** fontSize 12, lineHeight 18, minimap off, `stickyScroll` off, padding 6, Ctrl/Cmd+S bound to save, optional `autoHeight`. `CodeDiff` uses side-by-side diff with `hideUnchangedRegions`. `languageFor()` picks a language from the content type or filename. See [src/components/editor/CodeEditor.tsx:24-145](https://github.com/alexmbrown/wiremock-ui/blob/main/src/components/editor/CodeEditor.tsx).
- **Charts** are hand-built SVG: stacked columns, gridlines, ticks, hover band, drag-to-select and keyboard navigation. No chart library. See [src/features/dashboard/TrafficPanel.tsx:125-280](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/dashboard/TrafficPanel.tsx).
- **Virtualised lists** use `useVirtualizer` from `@tanstack/react-virtual`:
  - Stub list rows are estimated at 44 px and folder headers at 28 px. See [src/features/stubs/StubList.tsx:106](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/stubs/StubList.tsx).
  - Request journal rows are estimated at 30 px. See [src/features/requests/RequestsPage.tsx:76](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/requests/RequestsPage.tsx).
- **ZIP and YAML:** `fflate` (`unzipSync`/`zipSync`) and `yaml` (`parse`) handle import/export. See [src/lib/importers.ts:1-2](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/importers.ts).
- **Fonts:** system stacks only, no webfonts or `@font-face`.
  - `--font-sans: ui-sans-serif, system-ui, -apple-system, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif`
  - `--font-mono: ui-monospace, 'JetBrains Mono', 'Cascadia Code', 'SF Mono', Menlo, Consolas, monospace`
  - Source: [src/styles/tokens.css:7-8](https://github.com/alexmbrown/wiremock-ui/blob/main/src/styles/tokens.css)
  - The brief allows self-hosted Inter + JetBrains Mono "if justified. No Google Fonts CDN at runtime." See [design/DESIGN_BRIEF.md:29](https://github.com/alexmbrown/wiremock-ui/blob/main/design/DESIGN_BRIEF.md).
- **Icons:** "Lucide at 13–16 px with a 2 px stroke. Keep Lucide unless there's a strong reason." See [design/DESIGN_BRIEF.md:30](https://github.com/alexmbrown/wiremock-ui/blob/main/design/DESIGN_BRIEF.md).
- **Testing:**
  - Vitest unit tests cover the pure lib modules: [src/lib/lib.test.ts](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/lib.test.ts), [match.test.ts](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/match.test.ts), [wiremock4.test.ts](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/wiremock4.test.ts) and [src/features/dashboard/activity.test.ts](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/dashboard/activity.test.ts).
  - Integration tests ([server.integration.test.ts](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/server.integration.test.ts), [templates.integration.test.ts](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/templates.integration.test.ts)) run only when `WIREMOCK_URL` is set.
  - A grep count of `it(`/`test(` gives roughly 70 cases (14 + 19 + 24 + 5 unit; 7 + 1 integration).
  - There are no React component or end-to-end UI tests.
- **CI/CD:** GitHub Actions on push to `main`.
  - Node 22 and Temurin Java 21.
  - Starts WireMock 3.13 (jar from the npm package) and 4.0.0-beta.40, and runs `npm test` against each.
  - Builds, zips `dist` as `wiremock-ui.zip` for self-hosting, then deploys to GitHub Pages.
  - Source: [.github/workflows/deploy.yml](https://github.com/alexmbrown/wiremock-ui/blob/main/.github/workflows/deploy.yml)
- **No desktop wrapper or backend:** there is no Tauri, Electron or Cargo.toml in the repo. README: "It can't start WireMock, change CLI flags or install extensions. It can't read the server's `mappings/` folder (only `__files`) and gets no push updates (it polls)." See [README.md:90-92](https://github.com/alexmbrown/wiremock-ui/blob/main/README.md).
- **Dev tooling scripts:**
  - [scripts/wiremock.mjs](https://github.com/alexmbrown/wiremock-ui/blob/main/scripts/wiremock.mjs) runs the bundled 3.x jar, or downloads a 4.x jar into `.tools/` from Maven Central with a SHA-1 check.
  - [scripts/demo-seed.mjs](https://github.com/alexmbrown/wiremock-ui/blob/main/scripts/demo-seed.mjs) seeds realistic stubs with deterministic md5-derived ids.
  - [scripts/demo-traffic.mjs](https://github.com/alexmbrown/wiremock-ui/blob/main/scripts/demo-traffic.mjs) generates demo traffic.
  - [scripts/screenshots.mjs](https://github.com/alexmbrown/wiremock-ui/blob/main/scripts/screenshots.mjs) captures every screen in dark and light via puppeteer-core and an installed Chrome/Edge.
  - [scripts/gen-schema.mjs](https://github.com/alexmbrown/wiremock-ui/blob/main/scripts/gen-schema.mjs) regenerates the JSON schemas from a server's `/__admin/docs/swagger` and patches gaps in the 4.x spec.
  - [scripts/build-design-kit.mjs](https://github.com/alexmbrown/wiremock-ui/blob/main/scripts/build-design-kit.mjs) builds the component catalog.
- `.claude/launch.json` defines a dev-server preview (`npm run dev -- --port 5173 --strictPort`). See [.claude/launch.json](https://github.com/alexmbrown/wiremock-ui/blob/main/.claude/launch.json).
- **Bundle size:** the built `dist/` is 15 MB. The Monaco chunk `CodeEditor-*.js` is 1.37 MB, plus dozens of per-language chunks. Feature pages are small, typically 3–35 KB each; `ImportPage` is 130 KB. Observed via `ls dist/assets` and `du -sh dist`.
- **No linter installed:** there is no ESLint or Prettier dependency or config, yet the code contains `eslint-disable-next-line react-hooks/exhaustive-deps` comments. See [src/lib/queries.tsx:17](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/queries.tsx) and [TrafficPanel.tsx:155](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/dashboard/TrafficPanel.tsx).

### Inferences
- React + Vite + TS is also the default Tauri v2 frontend template, so wiremock-ui's frontend code, CSS and components can move into a Tauri webview with little change. To share code verbatim, /dev/all should pin the same majors: React 19, Vite 8, TS 6, TanStack Query 5, Zustand 5, Monaco 0.57.
- The custom hash router and the lack of ESLint are deliberate minimalism. A multi-tool app like /dev/all will probably outgrow the single `Section` enum in `nav.ts`, and a nested router (for example TanStack Router) would be worth considering. That is a design choice, not a finding.
- Monaco is the dominant bundle cost. In a desktop app download size matters less, and Monaco is already bundled offline.

### Gaps
- No Rust or Tauri code exists in this repo, so there is no precedent for how the author structures Rust commands.
- There is no lint or format configuration, so the author's formatting preferences cannot be confirmed. The code looks Prettier-formatted (single quotes, trailing commas, long print width of about 180 columns), but no `.prettierrc` exists.

---

## Q2. Design system: palette, themes, typography, spacing, radius, shadows, density, layout patterns, components, motion

### Takeaway
The look is a calm, dense developer-tool aesthetic in the vein of Linear, Vercel, Raycast, VS Code and Chrome DevTools:
- **Colour:** neutral grey surfaces with a single **indigo accent** (`#4f46e5` light / `#6d68f5` dark), plus semantic colour families for success, warning, danger and info, HTTP methods, status classes, environments and charts.
- **Type and size:** **13 px** base text, **28 px** controls and rows, radii of **4/6/10 px**, and **120 ms** transitions.
- **Implementation:** every value is a CSS custom property in one `tokens.css`, switched by `[data-theme]` and `[data-density]` attributes on `<html>`, and consumed by about 720 lines of class-based component CSS (`app.css`).

### Cited Findings
- **Intended feel:** "a precise developer tool, in the family of Linear, Vercel dashboard, Raycast, VS Code, Insomnia/Bruno and Chrome DevTools. It should be calm and compact, with information density first. Avoid marketing gloss." See [design/DESIGN_BRIEF.md:20](https://github.com/alexmbrown/wiremock-ui/blob/main/design/DESIGN_BRIEF.md).
- **Hard constraints from the brief:**
  - Light and dark are both first-class; the default follows the OS.
  - **Compact is the default** (13 px base, 28 px controls and rows). Comfortable mode is 14 px with 32–34 px controls.
  - "All visual values come from CSS custom properties in `tokens.css`."
  - Desktop-first at 1280–1920 px; 1024 px must work; below 900 px the side nav collapses to icons; phone is out of scope.
  - WCAG AA, visible focus rings, and "status never conveyed by colour alone".
  - Source: [design/DESIGN_BRIEF.md:22-33](https://github.com/alexmbrown/wiremock-ui/blob/main/design/DESIGN_BRIEF.md)
- **Tokens, condensed from [src/styles/tokens.css](https://github.com/alexmbrown/wiremock-ui/blob/main/src/styles/tokens.css)** (`design/tokens.css` is an identical copy, verified with `diff`):
  ```css
  :root{ /* scale */
    --fs-2xs:10px; --fs-xs:11px; --fs-sm:12px; --fs-md:13px; --fs-lg:15px; --fs-xl:18px; --fs-2xl:22px; --lh:1.45;
    --row-h:28px; --control-h:28px; --control-h-sm:24px; --topbar-h:44px; --sidenav-w:188px; --sidenav-w-collapsed:48px; --tabbar-h:34px;
    --space-1:2px; --space-2:4px; --space-3:6px; --space-4:8px; --space-5:12px; --space-6:16px; --space-7:24px; --space-8:32px;
    --radius-sm:4px; --radius:6px; --radius-lg:10px;
    --shadow-sm:0 1px 2px rgb(0 0 0/.06); --shadow:0 4px 16px rgb(0 0 0/.08),0 1px 3px rgb(0 0 0/.06); --shadow-lg:0 16px 48px rgb(0 0 0/.18),0 2px 8px rgb(0 0 0/.08);
    --dur:120ms; }
  /* LIGHT (:root,[data-theme=light]) */
  --bg:#ffffff --bg-subtle:#f8f8fa --bg-muted:#f1f1f4 --bg-hover:#ececf0 --bg-active:#e4e4ea --bg-elevated:#ffffff --bg-overlay:rgb(15 15 20/.42) --bg-code:#fafafb
  --border:#e5e5ea --border-strong:#d4d4dc --focus-ring:#6366f1
  --text:#17171c --text-muted:#4f4f5a --text-subtle:#7a7a86 --text-disabled:#a6a6b0
  --accent:#4f46e5 --accent-hover:#4338ca --accent-subtle:#eef0ff --accent-border:#c7cbff --accent-text:#fff --accent-fg:#4338ca
  --success:#15803d/-subtle #ecfdf3/-border #b7ebc9  --warning:#b45309/#fff8eb/#fcd9a1  --danger:#dc2626 (hover #b91c1c)/#fef2f2/#fbc7c7  --info:#0369a1/#eff8ff/#b9e0fb
  --m-get:#15803d --m-post:#1d4ed8 --m-put:#b45309 --m-patch:#7e22ce --m-delete:#dc2626 --m-other:#52525b (+ -bg tints #e9f8ef #eaf0ff #fff4e5 #f5ecff #feeeee #f1f1f4)
  --diff-add:#e6ffec --diff-del:#ffebe9
  --chart-matched:#4f46e5 --chart-unmatched:#b45309 --chart-2xx:#15803d --chart-3xx:#2563eb --chart-4xx:#d97706 --chart-5xx:#be123c --chart-other:#8b8b96 --chart-grid:#ececf0
  /* DARK ([data-theme=dark]) */
  --bg:#0e0e11 --bg-subtle:#141418 --bg-muted:#1b1b20 --bg-hover:#222228 --bg-active:#2a2a31 --bg-elevated:#18181d --bg-overlay:rgb(0 0 0/.6) --bg-code:#121216
  --border:#26262d --border-strong:#34343c --focus-ring:#818cf8
  --text:#ececf1 --text-muted:#b0b0bb --text-subtle:#80808c --text-disabled:#55555e
  --accent:#6d68f5 --accent-hover:#7f7bff --accent-subtle:#1f1e3d --accent-border:#3b3874 --accent-fg:#a5a3ff
  --success:#4ade80/#0f2318/#1c4a2f --warning:#fbbf24/#2a1f0a/#5a4012 --danger:#f87171 (hover #fca5a5)/#2a1214/#5c2125 --info:#38bdf8/#0c2130/#154361
  --m-get:#4ade80 --m-post:#7aa2ff --m-put:#fbbf24 --m-patch:#c084fc --m-delete:#f87171 --m-other:#a1a1aa (+ -bg #10261a #141e3a #2b200c #24163a #2c1315 #1f1f25)
  --diff-add:#12261a --diff-del:#2d1517
  --chart-matched:#6d68f5 --chart-unmatched:#c27c0e --chart-2xx:#1f9d55 --chart-3xx:#3b7ddd --chart-4xx:#c27c0e --chart-5xx:#d9415e --chart-other:#6b6b76 --chart-grid:#24242b
  shadows darker: rgb(0 0 0/.4–.6)
  /* [data-density=comfortable] */ --row-h:34px --control-h:32px --control-h-sm:28px --fs-md:14px --fs-sm:13px --topbar-h:50px --tabbar-h:38px
  ```
- **Chart colours:** "validated (lightness band, CVD ΔE, contrast) against --bg-elevated". See [src/styles/tokens.css:105](https://github.com/alexmbrown/wiremock-ui/blob/main/src/styles/tokens.css) and [DESIGN_BRIEF.md:167](https://github.com/alexmbrown/wiremock-ui/blob/main/design/DESIGN_BRIEF.md).
- **Semantic colour meanings to keep:**
  - Methods: GET green, POST blue, PUT amber, PATCH purple, DELETE red, other grey.
  - Status classes: 2xx success, 3xx info, 4xx warning, 5xx danger.
  - Environments: local grey, dev blue, test purple, staging amber, **prod red**.
  - Match verdicts: match success, no-match danger, unknown subtle.
  - Recording/live: a pulsing danger dot.
  - Source: [design/DESIGN_BRIEF.md:303-313](https://github.com/alexmbrown/wiremock-ui/blob/main/design/DESIGN_BRIEF.md)
- **Theme switching:**
  - An inline script in `index.html` reads `localStorage['wmui.prefs']` and sets `data-theme` before first paint ("avoid a flash"). See [index.html:9-17](https://github.com/alexmbrown/wiremock-ui/blob/main/index.html).
  - `useThemeSync` sets `data-theme` and `data-density` and listens to `prefers-color-scheme` when the theme is `system`. See [src/App.tsx:31-45](https://github.com/alexmbrown/wiremock-ui/blob/main/src/App.tsx).
  - The top-bar button cycles system → light → dark. See [src/components/layout/TopBar.tsx:54-55,112-114](https://github.com/alexmbrown/wiremock-ui/blob/main/src/components/layout/TopBar.tsx).
- **App shell:**
  - A CSS grid `.app` with rows `var(--topbar-h) 1fr` and columns `var(--sidenav-w) 1fr` (48 px when collapsed); the top bar spans both columns.
  - Top bar and side nav use `--bg-subtle` with 1 px borders; main content uses `--bg`.
  - Below 900 px the nav collapses to icons only.
  - Source: [src/styles/app.css:415-458, 674-679](https://github.com/alexmbrown/wiremock-ui/blob/main/src/styles/app.css)
- **Top bar contents, left to right:**
  - sidebar toggle;
  - brand (20 px logo + "WireMock UI" in weight 650);
  - **project switcher** (colour swatch + name);
  - **server switcher**: health dot, environment chip, name and muted URL;
  - `v3.13.2 · 15 ms` (version and admin latency);
  - red pulsing **REC** badge while recording;
  - READ-ONLY / PROTECTED badge;
  - spacer;
  - "Search & actions Ctrl+K" subtle button;
  - theme icon button;
  - help icon.
  - Source: [src/components/layout/TopBar.tsx:57-118](https://github.com/alexmbrown/wiremock-ui/blob/main/src/components/layout/TopBar.tsx)
- **Side nav:**
  - Three groups: main (Overview, Stubs, Requests, Near misses, Messages*), **Tools** (Record, Scenarios, Files, Verify, Compare servers, Import / export) and **Configure** (Server settings, Projects & servers, Guides).
  - Each item is 30 px tall with a 16 px Lucide icon and a right-aligned tabular-number count; counts use the warning colour for alerts.
  - Group labels are 10 px uppercase with 0.06em letter-spacing.
  - Sources: [src/components/layout/SideNav.tsx:41-71](https://github.com/alexmbrown/wiremock-ui/blob/main/src/components/layout/SideNav.tsx), [app.css:431-456](https://github.com/alexmbrown/wiremock-ui/blob/main/src/styles/app.css)
- **Page anatomy:**
  - `.page-header`: min-height 44 px, padding 8/12, h1 at 15 px, 16 px subtle icon, muted description and right-aligned actions.
  - `.toolbar`: min-height 38 px, gap 6; holds search, filters and segmented controls.
  - `.page-body`: padding 16 px.
  - Source: [app.css:459-464](https://github.com/alexmbrown/wiremock-ui/blob/main/src/styles/app.css)
- **Split panes:**
  - `.split > .pane.side` (bg-subtle with a right border) and `.pane.fill`.
  - A 5 px `.resizer` turns 40 % accent on hover or drag. The `Resizer` component uses pointer-drag with min/max limits; double-click resets the width to 340.
  - Sources: [app.css:466-471](https://github.com/alexmbrown/wiremock-ui/blob/main/src/styles/app.css), [src/components/ui/misc.tsx:147-181](https://github.com/alexmbrown/wiremock-ui/blob/main/src/components/ui/misc.tsx)
- **IDE-style editor tabs (`.etab`):**
  - The active tab gets a 2 px accent top bar and blends into the content background.
  - Preview tabs are *italic*; a dirty tab shows a dot that turns into × on hover.
  - Middle-click closes; tabs drag to reorder; the context menu offers close, close others, close all saved and pin.
  - Max width 240 px, min width 100 px; the tab bar is 34 px.
  - Sources: [app.css:510-538](https://github.com/alexmbrown/wiremock-ui/blob/main/src/styles/app.css), [src/features/stubs/StubsPage.tsx:41-84](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/stubs/StubsPage.tsx)
- **Component CSS ([app.css](https://github.com/alexmbrown/wiremock-ui/blob/main/src/styles/app.css)):**
  - **Buttons** (`.btn`, :153-187): height `--control-h`, padding 0 12 px, radius 6, 12 px font weight 500, `--border-strong` border on `--bg-elevated`. Variants: primary (accent), danger, ghost, subtle, danger-ghost, `.active` (accent-subtle). `btn-sm` is 24 px at 11 px; `.icon-btn` is a transparent square.
  - **Segmented control** (:211-235): `--bg-muted` track with 2 px padding; the active segment is `--bg-elevated` with `--shadow-sm`; 11 px labels.
  - **Inputs** (:238-266): `--bg` background, `--border-strong`; focus shows a `--focus-ring` border plus a 3 px `color-mix(... 22%)` halo; `.mono` variant; custom CSS-gradient select chevron.
  - **Switch** (:280-286): 28×16 track that turns accent when on.
  - **Badges** (:289-320): 18 px high, radius 4, 10 px bold, letter-spacing .03em. **Method badges** are monospace, fixed 48–52 px wide, with `-bg` tints. Status codes are monospace and coloured by class (:322-326).
  - **Tags** (:328-342): 20 px pills.
  - **Health dot** (:344-349): 8 px; `ok` adds a glow ring; pulses on a 1.4 s loop.
  - **Kbd** (:351-363): 2 px bottom border.
  - **Panels and cards** (:380-389): radius 10 px, 1 px border, panel header min-height 40 px.
  - **Callouts** (:391-408): info, warning, danger and success, each with `-subtle` background and `-border`.
  - **Tables** (:497-502): sticky header (11 px, subtle, weight 600), hover and selected rows.
  - **Lists** (:475-495): a 2 px transparent left border that becomes accent on selection, over an accent-subtle background; disabled stubs dim to 0.55 opacity with the URL struck through; folder headers are sticky.
  - **`.kv` key/value grid** (:504-507).
  - **Sub-tabs** (:540-559): 2 px accent underline.
  - **Modals** (:562-571): overlay top-aligned at 8vh; sizes md 560 / lg 820 / xl 1120; `pop` animation 160 ms `cubic-bezier(0.2,0.9,0.3,1.2)` from `translateY(6px) scale(.985)`.
  - **Menus** (:573-587): fade in 80 ms; 180–320 px wide; 28 px items with icon, hint and optional description line.
  - **Toasts** (:589-595): bottom-right.
  - **Command palette** (:597-603): 620 px wide with a 48 px, 15 px-text input.
  - **Empty states** (:606-609): centred, with a 32 px disabled-colour icon.
  - **Spinner** (:611-612): 14 px ring.
  - **Collapsible `.fieldset`** (:620-626): `<details>` with a rotating chevron.
  - **`.matcher-row` grid** (:628-629): key | 150 px operator | value | actions.
  - **Near-miss `.nm-row`** (:637-639): 22 px / 120 px / 1fr / 1fr grid; failing rows tinted danger-subtle.
  - **Drop zone** (:644-645): 1.5 px dashed border that turns accent on drag-over.
  - **Env chips** (:647-653).
  - **`.switcher`** (:655-658): 30 px.
  - **Status bar** (:671-672).
  - **Charts, KPIs and mix bar** (:681-722).
- **Utility classes** form a small Tailwind-like layer over the tokens: `.row`, `.row-sm`, `.col`, `.col-lg`, `.grow`, `.spacer`, `.grid-2/3/4`, `.p-4`, `.mt-4`, `.muted`, `.subtle`, `.small`, `.xs`, `.strong`, `.truncate`, `.mono`, `.label` (11 px uppercase). See [app.css:96-150](https://github.com/alexmbrown/wiremock-ui/blob/main/src/styles/app.css).
- **Base styles:**
  - `body { overflow:hidden; font-size:13px; line-height:1.45; -webkit-font-smoothing:antialiased }`.
  - `:focus-visible` uses a 2 px `--focus-ring` outline.
  - Custom 10 px rounded scrollbars.
  - `::selection` uses accent-subtle.
  - Source: [app.css:13-94](https://github.com/alexmbrown/wiremock-ui/blob/main/src/styles/app.css)
- **Motion:** "Motion is subtle: 80–160 ms fades and pops for menus and modals, and 120 ms colour transitions. No bouncy page transitions." Live data must not cause layout shift or flicker. See [DESIGN_BRIEF.md:327-337](https://github.com/alexmbrown/wiremock-ui/blob/main/design/DESIGN_BRIEF.md).
- **Traffic chart spec:** "Thin columns (≤ 24 px) with a 2 px surface gap between segments, a 4 px rounded top and a square baseline. Hairline gridlines at clean ticks". The tooltip leads with values; the chart is focusable and arrow keys step through intervals. A table toggle and a 100 % stacked "Responses by status" bar sit alongside. See [DESIGN_BRIEF.md:153-167](https://github.com/alexmbrown/wiremock-ui/blob/main/design/DESIGN_BRIEF.md) and [TrafficPanel.tsx:125-280](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/dashboard/TrafficPanel.tsx).
- **Component inventory:** the brief lists every component with its variants and states (button, icon button, segmented, inputs, checkbox/radio/switch, badge, method badge, status code, env chip, tag, health dot, kbd, callout, panel/card/stat card, list item, table, `.kv`, editor tabs, sub-tabs, fieldset, matcher row, match report, modal, menu, toast, command palette, empty state, drop zone, code shell, scenario graph, charts, spinner/progress/resizer). See [DESIGN_BRIEF.md:264-301](https://github.com/alexmbrown/wiremock-ui/blob/main/design/DESIGN_BRIEF.md).
- **React UI primitives:**
  - `MethodBadge`, `StatusCode`, `Spinner`, `Empty`, `Callout`, `Switch`, `Checkbox` (with indeterminate state), generic `Segmented<T>`, `Field`, `SearchInput` (Esc clears), `CopyButton`, `Resizer`, `RelTime`, `Kbd`, `MOD` (⌘/Ctrl). See [src/components/ui/misc.tsx](https://github.com/alexmbrown/wiremock-ui/blob/main/src/components/ui/misc.tsx).
  - Portal-based `Menu` / `MenuPopup` / `useContextMenu`. See [Menu.tsx](https://github.com/alexmbrown/wiremock-ui/blob/main/src/components/ui/Menu.tsx).
  - `Modal` (Esc closes, autofocus, restores focus). See [Modal.tsx](https://github.com/alexmbrown/wiremock-ui/blob/main/src/components/ui/Modal.tsx).
  - Promise-based `confirmDialog` / `promptDialog`, including a "type the server name to confirm" mode. See [dialogs.tsx:31-93](https://github.com/alexmbrown/wiremock-ui/blob/main/src/components/ui/dialogs.tsx).
  - `toast.success/error/info` with an optional action such as **Undo**; at most 5 toasts at once; 3.2 s default, 7 s with an action or on error. See [toast.tsx:20-41](https://github.com/alexmbrown/wiremock-ui/blob/main/src/components/ui/toast.tsx).
- **Brand mark:** an indigo rounded square (`#4f46e5`, rx 7 on 32) with a white "W" wave stroke. See [public/favicon.svg](https://github.com/alexmbrown/wiremock-ui/blob/main/public/favicon.svg). The brief calls it a placeholder. See [DESIGN_BRIEF.md:350](https://github.com/alexmbrown/wiremock-ui/blob/main/design/DESIGN_BRIEF.md).
- **Hard-coded colours outside the tokens:**
  - Project swatches: `PROJECT_COLORS = ['#6366f1','#0ea5e9','#10b981','#f59e0b','#ef4444','#a855f7','#ec4899','#64748b']`. See [src/store/projects.ts:49](https://github.com/alexmbrown/wiremock-ui/blob/main/src/store/projects.ts).
  - Monaco line highlight `#00000008` (light) / `#ffffff08` (dark). See [monaco.ts:73,79](https://github.com/alexmbrown/wiremock-ui/blob/main/src/components/editor/monaco.ts).
  - `.btn-danger` text is `#fff`. See [app.css:179](https://github.com/alexmbrown/wiremock-ui/blob/main/src/styles/app.css).
- **Design hand-off kit in `design/`:**
  - `DESIGN_BRIEF.md` (370 lines, including a ready-to-paste Claude Design prompt).
  - `component-catalog.html` (about 1,170 lines, self-contained, with the real CSS inlined).
  - `tokens.css`.
  - **48 screenshots** (24 screens × dark/light) at 1440×900.
  - Sources: [README.md:86-88](https://github.com/alexmbrown/wiremock-ui/blob/main/README.md), [design/](https://github.com/alexmbrown/wiremock-ui/blob/main/design/DESIGN_BRIEF.md)
- **What the screenshots show:**
  - Dense three-zone IDE layout: stub list with folder tree | VS Code tabs | form editor ([03-stub-editor-form-dark.png](https://github.com/alexmbrown/wiremock-ui/blob/main/design/screenshots/03-stub-editor-form-dark.png)).
  - Overview with 4 stat cards, an Activity range segmented control (5 min, 15 min, 1 hour, 6 hours, All loaded), a Traffic panel with KPIs and stacked columns, Top endpoints and Recent unmatched ([01-overview-light.png](https://github.com/alexmbrown/wiremock-ui/blob/main/design/screenshots/01-overview-light.png)).
  - DevTools-Network-style journal with a monospace millisecond timestamp column and an UNMATCHED warning badge ([10-request-detail-dark.png](https://github.com/alexmbrown/wiremock-ui/blob/main/design/screenshots/10-request-detail-dark.png)).
  - Near-miss match tables with red-tinted failing rows and "#1 / #2 / #3" candidates ([11-near-misses-light.png](https://github.com/alexmbrown/wiremock-ui/blob/main/design/screenshots/11-near-misses-light.png)).
  - Grouped templates menu ([08-new-stub-templates-menu-light.png](https://github.com/alexmbrown/wiremock-ui/blob/main/design/screenshots/08-new-stub-templates-menu-light.png)).
  - Raycast-like command palette ([22-command-palette-dark.png](https://github.com/alexmbrown/wiremock-ui/blob/main/design/screenshots/22-command-palette-dark.png)).

### Inferences
- "Style like wiremock-ui" is concrete and portable:
  - Copy `tokens.css` and `app.css` (about 920 lines) and the primitives in `src/components/ui/*` plus `CodeEditor`/`monaco.ts`.
  - Keep the shell: a 44 px top bar with project/environment switchers, a 188 px grouped left nav with counts, a page header and toolbar, split panes with VS Code tabs, and a Ctrl+K palette.
  - The nav's "main / Tools / Configure" grouping maps naturally onto /dev/all's tool categories (HTTP, Mock, Webhooks, Tunnel, Load, Utilities, DB, K8s/Docker, AI).
- The top-bar **project switcher → server (env) switcher** pattern matches /dev/all's project-centric model directly. Env chips (local, dev, test, staging, prod) with "protected / read-only" safety modes are reusable across HTTP-client environments, DB connections and K8s contexts.
- Several components are generic and would work for /dev/all's HTTP client, load tester and webhook tester without redesign: the method badge, status-code colouring, the `.nm-row` match table, the traffic chart (KPIs + stacked columns + status mix bar) and the `.kv` header grid.
- To give K8s/Docker/DB/AI tools their own identity colours, new token families would need adding. The current token set has no generic categorical palette beyond the method tints and `--chart-*`.

### Gaps
- The brief asks Claude Design for a redesign: final token values, component specs, P0 screens and a new brand mark ([DESIGN_BRIEF.md:360-370](https://github.com/alexmbrown/wiremock-ui/blob/main/design/DESIGN_BRIEF.md)). Nothing in the repo shows that redesign was done, so the current tokens are the "before" state.
- Known visual issues the author wants fixed ([DESIGN_BRIEF.md:339-350](https://github.com/alexmbrown/wiremock-ui/blob/main/design/DESIGN_BRIEF.md)):
  - the top bar crowds with long server names (proposed merged breadcrumb `Checkout / ● Local ▾`);
  - the stub form is long and single-column;
  - spacing rhythm is inconsistent;
  - stat cards are generic;
  - scenario graph labels collide;
  - "the light theme feels flat… In the dark theme, borders are slightly too faint";
  - badge sizes vary between 10 and 11 px;
  - empty states are text-only.

---

## Q3. Features: what the app does with WireMock (screens and user flows)

### Takeaway
It is a full-coverage WireMock 3.x/4.x admin console with 14 sections:
- **Main:** Overview, Stubs, Requests, Near misses, and Messages (4.x only).
- **Tools:** Record, Scenarios, Files, Verify, Compare servers, Import/export.
- **Configure:** Server settings, Projects & servers, Guides.

Signature features:
- an IDE-style stub workspace (form + schema-validated JSON + activity + code + diff);
- a live request journal with "create stub from request";
- client-side near-miss explanation with one-click "fix stub to match";
- 20 stub templates;
- import from HAR, OpenAPI, Postman, cURL and ZIP;
- multi-project/multi-server environments with server-to-server compare and copy;
- a connection doctor.

There are **no AI features**. The only data generation is OpenAPI schema → sample bodies and a picker for Handlebars template helpers.

### Cited Findings
- **Sections** (`Section` union): dashboard, stubs, requests, near-misses, messages, recording, scenarios, files, verify, compare, import, settings, servers, help. See [src/store/nav.ts:3-34](https://github.com/alexmbrown/wiremock-ui/blob/main/src/store/nav.ts). Every page except servers, help and compare is wrapped in `ConnectionGate`, which shows the Connection doctor when the server is unreachable. See [src/App.tsx:131-146](https://github.com/alexmbrown/wiremock-ui/blob/main/src/App.tsx).
- **README feature table** (projects & servers, stubs, stub editor, 20 templates, requests, messages, near misses, record & snapshot, scenarios, files, verify, compare, import/export, server settings, extras). See [README.md:9-27](https://github.com/alexmbrown/wiremock-ui/blob/main/README.md).
- **Stubs workspace, P0 ([DESIGN_BRIEF.md:80-126](https://github.com/alexmbrown/wiremock-ui/blob/main/design/DESIGN_BRIEF.md)):**
  - **List:**
    - Virtualised; resizable from 240 to 720 px (default 340).
    - Folder-tree / flat toggle; sorts: Match order (priority asc, then newest), URL, Name, Method. See [StubList.tsx:63-68](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/stubs/StubList.tsx) and [stub-utils.ts:334-349](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/stub-utils.ts).
    - Field search `method:post status:4 tag:x folder:y scenario:x id:abc is:proxy|fault|delay|templated|webhook|scenario|persistent|disabled|file|channel|websocket|sse`, plus free text over name, URL, id, folder, description, tags, proxy URL and body file. See [stub-utils.ts:351-412](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/stub-utils.ts).
    - Row layout: method badge · name (or monospace URL) on line 1. Line 2 holds the status code or PROXY/FAULT badge, the URL, trait icons (Timer = delay, Zap = templated, Webhook, GitBranch = scenario, FileText = body file, HardDrive = persistent) and up to 2 tags. See [StubList.tsx:332-371](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/stubs/StubList.tsx).
    - Single click opens a preview tab; double-click pins the tab; rows are draggable as JSON.
    - **Opt-in selection mode** (Select button, or Ctrl/Shift-click) turns the toolbar into a bulk bar: enable, disable, move to folder, tag, persistent, export, toggle proxy, copy to another server, delete with **Undo**. See [StubList.tsx:78-140](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/stubs/StubList.tsx) and [actions.tsx:60-130](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/stubs/actions.tsx).
  - **Tabs:**
    - One set per server, kept in sessionStorage.
    - A preview tab is replaced on the next single click.
    - Capped at 30 tabs, keeping dirty ones.
    - Source: [src/store/workspace.ts:49-72](https://github.com/alexmbrown/wiremock-ui/blob/main/src/store/workspace.ts)
  - **Editor views:** Form · JSON · Activity · Code · Diff. Plus Save/Create (Ctrl+S), Revert, short id with copy, DISABLED and PERSISTENT badges, and a ⋯ menu:
    - duplicate, or duplicate into a new tab;
    - enable/disable;
    - proxy ↔ mock toggle;
    - copy JSON, call-cURL, create-cURL or Java DSL;
    - download JSON;
    - copy to another server;
    - delete.
    - Source: [src/features/stubs/StubEditor.tsx:19, 130-198](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/stubs/StubEditor.tsx)
  - **Conflict detection:**
    - Each tab stores a server-JSON baseline.
    - If the server copy changes while the user has a draft, a warning shows with View diff, Discard mine & reload, and Keep mine.
    - If the stub disappeared, the editor offers "Restore as new stub".
    - Source: [StubEditor.tsx:36-77, 200-219](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/stubs/StubEditor.tsx)
  - **Form view (collapsible sections):**
    - **General:** name, folder (autocomplete), description, tags, priority, persistent.
    - **Request:**
      - method, including 4.x `oneOf`/`noneOf` sets and QUERY;
      - URL match type (equals, path, regex, path regex, path template) with path-param rows;
      - "Match on:" chips for Query, Headers, Cookies, Body, Form, Multipart, Basic auth, Host, Port, Scheme, Client IP and Custom matcher.
      - Matcher rows offer about 20 operators, including date and 4.x number operators, plus and/or/not and hasExactly/includes.
      - Body pattern cards: equalToJson (with options and placeholders), JSONPath with a sub-matcher, JSON Schema, XML, XPath with namespaces, binary.
    - **Response:**
      - Modes: Respond / Proxy to upstream / Simulate fault / Open channel (4.x).
      - Body types: None / JSON / Text·template / File / Binary, with a Monaco editor.
      - "Insert template helper" menu.
      - Delays: None / Fixed / Uniform / Log-normal / Chunked dribble.
      - Faults: connection reset, empty response, malformed chunk, random data then close.
    - **Scenario**, **Webhooks & serve event listeners**, **Other metadata**.
    - Sources: [src/features/stubs/StubForm.tsx:80-130](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/stubs/StubForm.tsx), [form/ResponseSection.tsx:14-17, 76-79, 188-192, 402-406](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/stubs/form/ResponseSection.tsx), [form/PatternEditors.tsx](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/stubs/form/PatternEditors.tsx)
  - **JSON view:** Monaco validated against WireMock's schema, with completions on Ctrl+Space. See [StubEditor.tsx:222-233](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/stubs/StubEditor.tsx).
  - **Activity view:**
    - Requests this stub served, via server-side `?matchingStub=`.
    - Near misses for this stub's pattern, via `POST /near-misses/request-pattern`, with a field-by-field report and "Adjust stub to accept this request".
    - Source: [StubActivity.tsx:13-37](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/stubs/StubActivity.tsx)
  - **Code view:** Java DSL, cURL to call the stub, cURL to create it (admin API), and the JSON file. See [StubEditor.tsx:252-276](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/stubs/StubEditor.tsx).
- **20 templates** in 6 groups (Basics, Matching, Dynamic, Resilience, Proxy, Integration). See [src/lib/templates.ts:13-310](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/templates.ts).
  - Basics: JSON response, REST CRUD (5 stubs in a folder), error 404/500, body from file, CORS preflight.
  - Matching: match JSON body, match by JSONPath, GraphQL operation, SOAP/XPath, query + header + auth.
  - Dynamic: templated response, stateful scenario.
  - Resilience: slow (log-normal), network fault, fail-then-succeed scenario, chunked dribble.
  - Proxy: proxy everything else, proxy one path.
  - Integration: webhook callback, binary body.
- **About 36 Handlebars helper snippets** in groups Request, Body, Random, Date, Logic, Strings and Misc, including JWT and Faker extension snippets, feed the "insert template helper" menu. See [templates.ts:313-349](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/templates.ts).
- **Requests journal, P0:**
  - Live polling toggle with an interval of 1/2/3/5/10 s and a "last N" limit of 50–5000.
  - Filter syntax `method:` `status:` `stub:` plus free text; segmented All / Matched / Unmatched.
  - Export as HAR or JSON; Clear.
  - Optional time window `?from=&to=`, set by clicking or dragging on the overview chart.
  - Sources: [src/features/requests/RequestsPage.tsx:18-110](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/requests/RequestsPage.tsx), [TrafficPanel.tsx:141-145](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/dashboard/TrafficPanel.tsx)
  - The detail pane has sub-tabs Request / Response / Matched stub-or-"Why no match?" / Raw. It opens on "match" when the request was unmatched. See [RequestDetail.tsx:19-22,59-145](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/requests/RequestDetail.tsx).
  - **Create stub from request** dialog:
    - URL mode: exact path / path + query / "Generalise ids", which turns numeric segments into `/[0-9]+` and UUIDs into a regex.
    - Match query, choose headers, choose body matcher, use the actual response.
    - Live JSON preview, then "Open in editor".
    - Sources: [CreateStubDialog.tsx:11-85](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/requests/CreateStubDialog.tsx), [src/lib/request-to-stub.ts:26-83](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/request-to-stub.ts)
- **Near misses, P0:**
  - Unmatched requests are grouped by request, newest first, with a "differs: …" summary.
  - Each expands to the top 3 candidate stubs with their distance, Open and **Fix stub to match**.
  - Fix opens the stub with the minimal edit pre-applied as a draft for review. See [NearMissesPage.tsx:21-56](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/requests/NearMissesPage.tsx) and [src/lib/fix.ts:5-110](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/fix.ts).
  - WireMock's API reports only a *distance*. The per-field report (✓ / ✗ / ? "evaluated on server") comes from a client-side re-implementation of matching. See [src/lib/match.ts:1-6](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/match.ts).
  - The scenario state is added as its own row, because the near-miss API omits it. See [match.ts:393-410](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/match.ts).
- **Overview:**
  - 4 stat cards: Stubs, Requests in journal, Unmatched, Recording.
  - Activity range 5 min / 15 min / 1 h / 6 h / all loaded.
  - Traffic panel: KPIs (requests, rate per minute, unmatched %, latency p50/p95), stacked columns on "nice" intervals, a Chart/Table toggle and a "Responses by status" 100 % bar.
  - Also: Top endpoints, Recent unmatched, a Server panel, **Unused stubs** (via `/mappings/unmatched`) with "Remove all".
  - A security callout appears when a non-local server has no auth.
  - Header actions: Persist all, Clear journal, Reset, New stub.
  - Sources: [DESIGN_BRIEF.md:153-169](https://github.com/alexmbrown/wiremock-ui/blob/main/design/DESIGN_BRIEF.md), [src/features/dashboard/activity.ts](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/dashboard/activity.ts)
- **Record & snapshot:**
  - Wizard fields: target URL, path regex, method, captured headers, body matcher (auto, equalToJson, equalTo, equalToXml) with ignoreArrayOrder/ignoreExtraElements, text/binary extract thresholds, persist, repeatsAsScenarios, transformers.
  - Form state is kept in sessionStorage.
  - Start; Stop & review lists captured stubs with checkboxes, "Discard unchecked", "Move to folder" and "Persist kept".
  - Snapshot from the journal can include `allowNonProxied`.
  - Sources: [src/features/recording/RecordingPage.tsx:12-120](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/recording/RecordingPage.tsx), [DESIGN_BRIEF.md:171-178](https://github.com/alexmbrown/wiremock-ui/blob/main/design/DESIGN_BRIEF.md)
- **Scenarios:**
  - One panel per scenario: current-state badge, a state select with **Set state**, and Reset.
  - An SVG state-machine diagram; clicking a state jumps to it.
  - A table of "When state is → Request → Moves to"; rows open the stub.
  - Also "New scenario stub" and "Reset all".
  - Source: [src/features/scenarios/ScenariosPage.tsx:9-110](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/scenarios/ScenariosPage.tsx)
- **Files (`__files`):**
  - Filterable list; new file; binary-safe upload (drag anywhere); replace; download.
  - Image preview; Monaco editor for text extensions.
  - "Used by" stub chips; "Stub that serves it".
  - Source: [src/features/files/FilesPage.tsx:14-60](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/files/FilesPage.tsx)
- **Verify:**
  - Request-pattern builder with an "exactly / at least / at most / none N" assertion, using `/requests/count` + `/requests/find`.
  - Copy as Java `verify(...)`; Remove matching requests.
  - "Find stubs by metadata" (JSONPath) with delete matching.
  - Source: [src/features/verify/VerifyPage.tsx:12-60](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/verify/VerifyPage.tsx)
- **Compare servers:**
  - Pick any two servers across all projects.
  - Stubs pair by id, falling back to method + URL + name + scenario.
  - Statuses: changed, only A, only B, same; Differences/All filter.
  - Side-by-side Monaco diff; copy selected stubs A→B or B←A.
  - Source: [src/features/compare/ComparePage.tsx:14-70](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/compare/ComparePage.tsx)
- **Import / export:**
  - Import by drop zone or paste. Formats:
    - WireMock stub JSON (single, array or `{mappings}`);
    - ZIP root-dir (`mappings/` + `__files/`);
    - **HAR**, skipping static assets and deduplicating;
    - **OpenAPI 3 / Swagger 2** in JSON or YAML, using examples or schema-generated bodies, with folders from title/tag;
    - **Postman** collections, with folder hierarchy and saved examples;
    - **cURL** (one or many commands, a shell-quote-aware parser);
    - WireMock UI snapshots and project exports.
  - Preview list with checkboxes and an "exists" badge.
  - Options: duplicate policy OVERWRITE/IGNORE, target folder, "delete stubs not in import".
  - Export: stubs JSON, a ZIP root-dir usable with `--root-dir`, and a **full snapshot**: JSON of stubs + base64 files + settings + scenario states + 4.x message stubs.
  - Sources: [src/lib/importers.ts:9-404](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/importers.ts), [src/features/import/ImportPage.tsx:54-130](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/import/ImportPage.tsx)
- **Server settings:**
  - Global latency presets: No delay, Fast LAN 5–20 ms uniform, Typical API ~120 ms log-normal, Slow 3G ~800 ms, Timeout test 30 s. A custom distribution is also available.
  - Proxy pass-through switch; extended settings JSON.
  - Danger zone: persist all, clear journal, reset scenarios, reset server, delete all stubs, shutdown.
  - **Startup command builder** with Docker, Java, npm and docker-compose snippets.
  - "This browser" prefs: theme, density, default editor, word wrap.
  - Sources: [src/features/settings/SettingsPage.tsx:13-311](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/settings/SettingsPage.tsx), [src/features/help/startup.ts:16-49](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/help/startup.ts)
- **Projects & servers:**
  - Projects have colour swatches. Servers have env chips (local, dev, test, staging, prod, other), admin path, auth (none, basic, bearer, custom header) with session-only secrets by default, extra headers, notes, and **protected** (confirm every change, type-the-name for destructive actions) or **read-only** modes.
  - Test connection; reorder, duplicate; export projects with or without credentials; import.
  - Sources: [src/features/servers/ServersPage.tsx:14-60](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/servers/ServersPage.tsx), [src/store/projects.ts:6-47](https://github.com/alexmbrown/wiremock-ui/blob/main/src/store/projects.ts)
- **Connection doctor:**
  - Explains in browser terms why a call failed: mixed content; the CORS preflight rejecting `Authorization` on 4.x (with an nginx snippet); 401/403; a 404 admin path; timeout; a generic checklist that names Chrome's "Apps on device" / "Local network" permissions.
  - Copyable startup commands.
  - Source: [src/features/servers/ConnectionDoctor.tsx:5-158](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/servers/ConnectionDoctor.tsx)
- **Messages (WireMock 4 only):**
  - Message-stub list and editor with Form + JSON validated against the 4.x schema.
  - 4 templates: WebSocket echo, WebSocket request/reply, WebSocket broadcast, SSE push on HTTP call. Templates also create the channel-opening HTTP stubs.
  - Channels view: endpoints table + open channels with an ad-hoc send box.
  - Message journal.
  - Sources: [src/features/messages/MessagesPage.tsx:20-60](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/messages/MessagesPage.tsx), [src/lib/messages.ts:107-180](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/messages.ts)
- **Guides:** an in-app docs page with a left TOC: getting started, connecting from a browser, proxying, recording, faults & delays, templating cheat sheet, WebSocket/SSE, self-hosting, where data lives, shortcuts, limitations. See [src/features/help/HelpPage.tsx:9-21](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/help/HelpPage.tsx).
- **Keyboard:**
  - Ctrl/⌘ K opens the command palette, with groups Actions, Go to, Stubs and Switch server, and actions such as new stub, persist all, clear journal, reset scenarios/mappings/all.
  - `/` focuses the page search (`[data-page-search]`).
  - `g` + o/s/r/n/m/c/e/f/v/i/h jumps to a section within 1.2 s.
  - Ctrl/⌘ S saves; Esc closes overlays and leaves selection mode.
  - Sources: [src/lib/shortcuts.ts:11-64](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/shortcuts.ts), [src/features/palette/CommandPalette.tsx:29-110](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/palette/CommandPalette.tsx)
- **Self-hosting and deep links:**
  - When served from WireMock's own `__files`, the app auto-connects to its origin and hides its own asset requests from the journal. See [App.tsx:68-88](https://github.com/alexmbrown/wiremock-ui/blob/main/src/App.tsx) and [queries.tsx:92-100](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/queries.tsx).
  - `#/stubs?server=http://host:8080` selects or adds a server. See [App.tsx:47-66](https://github.com/alexmbrown/wiremock-ui/blob/main/src/App.tsx).
- **Removed feature:** commit `82e9265` removed the **Test request** page (send requests to stubs, "Edit & send", "Replay") and the browser-to-stub `send()` client: "The UI no longer calls stubs." README now says "It doesn't send traffic to your stubs (use your app, tests, cURL or an HTTP client)." Sources: `git log`, [README.md:92](https://github.com/alexmbrown/wiremock-ui/blob/main/README.md).
- **No AI features.** The repo has no LLM or AI code. "Generation" is limited to `sampleFromSchema` (OpenAPI schema → example) in [importers.ts:250-285](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/importers.ts), example requests derived from a pattern via `exampleRequest`/`deRegex` in [codegen.ts:7-96](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/codegen.ts), and Handlebars random/Faker snippets.

### Inferences
- The removed "Test request" page is exactly the gap /dev/all's HTTP client fills. A combined app can restore "send to stub", "replay from journal" and "edit & send" by linking the mock module to the HTTP-client module.
- Features that transfer well beyond WireMock:
  - **Requests journal + create-stub-from-request** maps onto the webhook tester (capture, inspect, then turn into a mock).
  - **Import (HAR / OpenAPI / Postman / cURL)** also suits the HTTP client's collection import.
  - **Overview traffic panel** maps onto load-tester results.
- AI hooks /dev/all could add using the existing pure functions (none exist today):
  - generate stubs from natural language or an OpenAPI spec;
  - explain near misses;
  - generate realistic response bodies (local LLM).

### Gaps
- No screenshot exists for the Messages section ([DESIGN_BRIEF.md:256](https://github.com/alexmbrown/wiremock-ui/blob/main/design/DESIGN_BRIEF.md)).
- I did not read every line of StubForm/PatternEditors/ResponseSection (about 1,600 lines). The descriptions of form behaviour come from the brief, README and section headers, not from line-by-line review.

---

## Q4. Data model and API integration: admin endpoints, types, settings/config storage, persistence

### Takeaway
- **API client:** one typed `WireMockClient` class (a `fetch` wrapper) covers the whole `/__admin` API, with normalised `ApiError` kinds.
- **Types:** a hand-written `types.ts` mirrors the admin JSON and uses index signatures so unknown fields survive round-trips.
- **Writes:** every write goes through `useWrite()`, which enforces read-only/protected servers, confirmations, toasts and query invalidation.
- **Data placement:** WireMock is the source of truth. Folders, tags, descriptions and disabled/proxy state are stored **inside each stub's `metadata`** on the server. The browser keeps only projects/servers and prefs (localStorage) and open tabs, drafts and secrets (sessionStorage).

### Cited Findings
- **Connection config:** `{ url, adminPath='/__admin', auth: {type:'none'|'basic'|'bearer'|'header', username, password, token, headerName, headerValue, remember}, headers[], timeoutMs=15000 }`. See [src/lib/api.ts:27-46, 176-183](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/api.ts).
- **`ApiError` kinds:** `'network' | 'http' | 'timeout' | 'mixed-content' | 'parse' | 'auth-preflight'`, plus status and body. HTTP errors parse WireMock's `{errors:[{title,detail}]}`. See [api.ts:48-63, 227-238](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/api.ts).
- **Browser-only plumbing in `api.ts`:**
  - mixed-content detection (`isMixedContent`, Safari special case);
  - Chrome Local Network Access hint (`targetAddressSpace`);
  - private-IP and internal-hostname detection;
  - `diagnoseAuthPreflight` re-probes `/health` without credentials.
  - Source: [api.ts:65-168](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/api.ts)
- **Endpoints used** (all relative to `adminBase`), from [api.ts:263-516](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/api.ts):
  - **System:**
    - `GET /health`; `GET /version` (404 means 2.x); `ping()` falls back to `GET /mappings?limit=1`.
    - `POST /reset`, `POST /shutdown`.
    - `GET /settings` and `POST /settings` (always sends the merged object because WireMock replaces the whole object).
    - `GET /docs/swagger`.
  - **Mappings:**
    - `GET /mappings?limit&offset`, `GET|PUT|DELETE /mappings/{id}`, `POST /mappings`, `DELETE /mappings`.
    - `POST /mappings/reset`, `POST /mappings/save`.
    - `POST /mappings/import` (`{mappings, importOptions:{duplicatePolicy, deleteAllNotInImport}}`).
    - `POST /mappings/find-by-metadata`, `POST /mappings/remove-by-metadata`.
    - `GET|DELETE /mappings/unmatched`.
  - **Journal:**
    - `GET /requests?limit&since&unmatched&matchingStub`, `GET|DELETE /requests/{id}`, `DELETE /requests`.
    - `POST /requests/count|find|remove`, `GET /requests/unmatched`.
    - `GET /requests/unmatched/near-misses`, `POST /near-misses/request`, `POST /near-misses/request-pattern`.
  - **Recording:** `POST /recordings/start|stop|snapshot`, `GET /recordings/status`.
  - **Scenarios:** `GET /scenarios`, `POST /scenarios/reset`, `PUT /scenarios/{name}/state`.
  - **WireMock 4 messaging:**
    - `GET|POST|DELETE /message-mappings`, `GET|PUT|DELETE /message-mappings/{id}`, `POST /message-mappings/find-by-metadata|remove-by-metadata`.
    - `GET /channels`, `GET /channel-providers`, `POST /channels/send`.
    - `GET|DELETE /messages`, `GET|DELETE /messages/{id}`, `POST /messages/count|find|remove`.
  - **Files:** `GET /files` (Windows backslashes normalised), `GET|PUT|DELETE /files/{path}` (text or binary).
- **Core types** ([src/lib/types.ts:1-407](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/types.ts)):
  - `StubMapping {id, uuid?, name, request: RequestPattern, response: ResponseDefinition, persistent, priority, scenarioName, requiredScenarioState, newScenarioState, postServeActions, serveEventListeners, metadata, insertionIndex}`.
  - `RequestPattern` covers method (or 4.x `MethodSet`), the 5 URL keys, scheme/host/port/clientIp, path/query/form/header/cookie maps of `ContentPattern`, basic auth, body and multipart patterns, and customMatcher.
  - `ContentPattern` covers all operators, including and/or/not, hasExactly/includes and number matchers.
  - `ResponseDefinition` covers status, headers, body/jsonBody/base64Body/bodyFileName, fault, delays, proxy fields, transformers, and `openWebsocketChannel`/`openSseChannel`.
  - Also `ServeEvent` (with `timing` and `subEvents`), `LoggedRequest`, `LoggedResponse`, `NearMiss` (`matchResult.distance`), `Scenario`, `GlobalSettings`, `RecordSpec`, `RecordingStatus`.
  - 4.x types: `MessageStubMapping`, `MessageTrigger` (`message` | `http-stub` | `http-request`), `MessageAction`, `ChannelTarget` (`originating` | `request-initiated`), `MessageChannel`, `MessageServeEvent`.
  - The header comment states the round-trip policy: "unknown fields are preserved via index signatures so round-tripping a stub never drops data the UI does not know about." See [types.ts:1-3](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/types.ts).
- **Capability detection:** `capabilitiesFor(version, specPaths)` returns `{v4, messages, numberMatchers, methodSets, channelStubs (4.0.0-beta.39+), statusMessage, uuidAlias, schema: 3|4}`. It comes from `/version`, with OpenAPI paths as a fallback, and is cached for 5 minutes. See [src/lib/capabilities.ts:6-66](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/capabilities.ts) and [queries.tsx:52-68](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/queries.tsx).
- **Query keys and polling:**
  - Keys are `['wm', serverKey, …]`, where `serverKey = id@normalizedUrl+adminPath`, so stale data never shows after a URL change. See [queries.tsx:28-45](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/queries.tsx) and [projects.ts:243-247](https://github.com/alexmbrown/wiremock-ui/blob/main/src/store/projects.ts).
  - Polling intervals:
    - health: 10 s
    - mappings: 15 s
    - journal: `pollMs` (default 3000) when live
    - near misses: ≥ 3 s
    - unused stubs: 15 s
    - scenarios: 10 s
    - recording status: 5 s
    - channels: ≥ 2 s live, otherwise 15 s
    - activity window: ≥ 10 s
  - Source: [queries.tsx:70-238](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/queries.tsx)
- **`useWrite()` wrapper:**
  - Refuses writes on read-only servers.
  - Confirms on protected servers, requiring the server name to be typed for danger actions.
  - Shows success/error toasts and invalidates queries.
  - Source: [queries.tsx:240-290](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/queries.tsx)
  - `useStubCache` optimistically upserts and removes stubs in the mappings cache. See [queries.tsx:298-323](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/queries.tsx).
- **Metadata conventions on the server:**
  - `metadata["wiremock-gui"].folder`, compatible with holomekc/wiremock-gui; `metadata.section` (testsmith) is also read.
  - `metadata.wmui = { description, tags, disabled, proxyOff, updatedAt, createdAt }`.
  - Disabling a stub adds a request header matcher `X-WireMock-UI-Disabled: equalTo "true"` that no client sends.
  - The proxy↔mock toggle parks proxy config in `wmui.proxyOff`.
  - Sources: [src/lib/stub-utils.ts:3-22, 145-186](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/stub-utils.ts), [README.md:77-84](https://github.com/alexmbrown/wiremock-ui/blob/main/README.md)
  - Example demo mapping: `"metadata": {"wiremock-gui": {"folder": "/platform"}, "wmui": {"tags": ["smoke"]}}`. See [.wiremock-demo/mappings/health-139f97a9-….json](https://github.com/alexmbrown/wiremock-ui/blob/main/.wiremock-demo/mappings/health-139f97a9-bf27-4c51-a7ce-d69dfc466821.json).
- **Identity handling:** `normalizeIdentity` always sends `id`, never only `uuid`, because 4.x ignores `uuid`. `cleanForSave` strips `insertionIndex` and `response.fromConfiguredStub`. See [stub-utils.ts:44-56, 231-246](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/stub-utils.ts).
- **Browser storage:**
  - localStorage `wmui.projects`: projects and servers, with secrets stripped unless `auth.remember` is set (Zustand persist v1).
  - sessionStorage `wmui.session-secrets`: password, token and headerValue per server id.
  - localStorage `wmui.prefs`: theme, density, editorMode, pollMs = 3000, journalLimit = 200, wordWrap, sidebarCollapsed, stubListWidth = 340, showFolders.
  - sessionStorage `wmui.workspace`: tabs and drafts per server.
  - sessionStorage `wmui.collapsed.<sid>` and `wmui.record`.
  - Sources: [projects.ts:186-241](https://github.com/alexmbrown/wiremock-ui/blob/main/src/store/projects.ts), [prefs.ts:22-38](https://github.com/alexmbrown/wiremock-ui/blob/main/src/store/prefs.ts), [workspace.ts:115](https://github.com/alexmbrown/wiremock-ui/blob/main/src/store/workspace.ts), [StubList.tsx:77,113](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/stubs/StubList.tsx), [RecordingPage.tsx:44-50](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/recording/RecordingPage.tsx)
- **Data shapes:**
  - `Project {id, name, description, color, servers[], activeServerId, createdAt, updatedAt}`.
  - `ServerConfig {id, name, url, adminPath, auth, headers, env, protected, readOnly, notes}`.
  - The seed is project "Default" with server "Local" at `http://localhost:8080`.
  - `redactServer` strips credentials and any header whose name matches `/auth|token|key|secret/i` for exports.
  - Source: [projects.ts:8-65, 263-271](https://github.com/alexmbrown/wiremock-ui/blob/main/src/store/projects.ts)
- **File formats:**
  - Projects export: `{kind:'wiremock-ui/projects', version:1, exportedAt, projects}`. See [ServersPage.tsx:14,28-36](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/servers/ServersPage.tsx).
  - Server snapshot: `{kind:'wiremock-ui/server-snapshot', version:1, exportedAt, server{name,url,wiremockVersion}, settings, scenarios[{name,state}], mappings, messageMappings?, files{path:base64}}`. See [ImportPage.tsx:103-130](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/import/ImportPage.tsx) and [importers.ts:22,66-70](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/importers.ts).
  - ZIP export writes `mappings/<folder>/<safe-name>-<id8>.json` + `__files/…`, usable with `--root-dir`. See [importers.ts:113-126](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/importers.ts).
- **No local file persistence:** apart from user-initiated downloads (`<a download>` + Blob) and the file picker (`<input type=file>`), nothing is written to disk. See [src/lib/download.ts:1-29](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/download.ts).
- **WireMock 4 beta workarounds:**
  - `PUT /message-mappings/{id}` can silently drop the mapping, so the client verifies it survived and re-creates it if not.
  - `POST` with an existing id duplicates, so imports update instead.
  - SSE channels are reported as `"websocket"`; they are told apart by the `Upgrade` header.
  - `/channels/send` ignores headers.
  - Sources: [README.md:94-108](https://github.com/alexmbrown/wiremock-ui/blob/main/README.md), [api.ts:437-453](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/api.ts), [messages.ts:10-21](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/messages.ts)

### Inferences
- In a Tauri app most of the browser-specific connection logic disappears if admin calls go through Rust (reqwest) or the Tauri HTTP plugin: mixed content, Local Network Access, the CORS preflight with `Authorization`, and parts of the Connection doctor. The rest of the client can keep the same method surface, either as a TS class calling `invoke()` or as a Rust `WireMockClient`.
- The "metadata on the server" convention keeps wiremock-ui, holomekc/wiremock-gui and /dev/all interoperable on the same WireMock server. /dev/all should keep `metadata["wiremock-gui"].folder` and `metadata.wmui` so both apps can manage the same stubs.
- /dev/all is "local-first, project-centric", so the localStorage `Project`/`ServerConfig` model would naturally move into per-project files: a project folder holding mock server connections, plus ZIP root-dir / snapshot exports that already exist as formats. Secrets would go to the OS keychain rather than sessionStorage. This is a suggestion, not something found in the repo.
- If /dev/all runs its own embedded mock server instead of external WireMock, `types.ts` plus the WireMock JSON stub format could still be the stub schema, keeping import/export compatible with WireMock root dirs. Matching would then have to be implemented in Rust; `match.ts` is a readable spec of the semantics to port, including Java regex quirks and multi-value semantics.

### Gaps
- The repo has no Rust or Tauri code, so there is no direct evidence of how the author would run WireMock from a desktop app.
- `.tools/` does contain a JRE 21 (`jdk-21.0.12.1+1-jre`) and a `wiremock-4.0.0-beta.40` jar used by `scripts/wiremock.mjs` for local development. A Java sidecar is possible, but it is not mentioned in the docs.

---

## Q5. Code quality and reusability: what ports cleanly into a Tauri app, plus technical debt

### Takeaway
- **Layering is clean:** about 3,000 lines of **framework-free, unit-tested pure TypeScript in `src/lib/`** (matching, fix suggestion, JSONPath, codegen, importers, HAR, request→stub, templates, messages, capabilities, types). Thin React feature pages sit on top, along with a small, token-driven component kit.
- **Portability:** the lib layer and UI kit can move into a Tauri React frontend nearly verbatim.
- **Needs replacing:** browser-only glue (CORS/mixed-content diagnostics, `<a download>`, `<input type=file>`, localStorage/sessionStorage, polling) should become Tauri equivalents.
- **Debt:** small. There are no TODO/FIXME markers. The main items are no linter, no UI tests, approximate client-side matching, a heavy Monaco bundle and a single global section router.

### Cited Findings
- **Reusable pure modules** (no React imports except where noted). File paths are under [src/lib/](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/):

| Module | Lines | What it does | Portability |
|---|---|---|---|
| `types.ts` | 407 | WireMock 3/4 admin JSON types | verbatim |
| `api.ts` | 517 | `WireMockClient`, all admin endpoints, `ApiError`, browser network diagnostics | keep method surface; swap `fetch` for Tauri HTTP or Rust; drop mixed-content/LNA code |
| `capabilities.ts` | 68 | version → feature flags (3.x vs 4.x) | verbatim |
| `stub-utils.ts` | 465 | metadata conventions, disable/proxy toggles, traits, field search, sort, folder tree, matcher op labels | verbatim |
| `match.ts` | 526 | client-side WireMock matcher producing per-field verdicts (regex Java→JS, json-unit placeholders, XML normalise, XPath via DOMParser, JSONPath, dates, number matchers, multi-value semantics, scenario row) | verbatim in webview (needs DOM `DOMParser`/`XPathResult` for XML); also a spec for a Rust port |
| `jsonpath.ts` | 231 | small JSONPath evaluator (filters, slices, recursive descent, `.length()`) | verbatim |
| `fix.ts` | 110 | minimal edit so a stub matches a request | verbatim |
| `request-to-stub.ts` | 83 | journal entry → stub (URL modes, id generalisation, header/body options) | verbatim; reusable for webhook tester / HTTP history |
| `har.ts` | 88 | journal → HAR, HAR → stubs | verbatim; reusable for HTTP client |
| `importers.ts` | 404 | file/text/ZIP sniffing; cURL parser; OpenAPI 3/Swagger 2 → stubs with `sampleFromSchema`; Postman → stubs; ZIP root-dir builder | verbatim; cURL/OpenAPI/Postman parsers reusable by HTTP client |
| `codegen.ts` | 302 | example request from pattern, cURL builders, WireMock Java DSL generator | verbatim |
| `templates.ts` | 349 | 20 stub templates + ~36 Handlebars snippets | verbatim (data) |
| `messages.ts` | 186 | WireMock 4 WebSocket/SSE helpers + 4 templates | verbatim |
| `download.ts` | 33 | Blob download, file picker | replace with Tauri dialog/fs |
| `queries.tsx` | 323 | TanStack Query hooks, `useWrite` guard, cache helpers | React; keep pattern, adapt to project model |
| `shortcuts.ts` | 65 | global keys | React; generalise |

  Sources: [src/lib/](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/api.ts), line counts from `wc -l`.
- **The UI kit is small and dependency-light:**
  - `components/ui/misc.tsx` (226 lines), `Menu.tsx` (153), `Modal.tsx` (58), `dialogs.tsx` (130), `toast.tsx` (77).
  - `components/editor/CodeEditor.tsx` (145) + `monaco.ts` (96).
  - `components/layout/TopBar.tsx` (120), `SideNav.tsx` (93), `ErrorBoundary.tsx` (24).
  - `MatchReportView.tsx` (35).
  - Source: [src/components/](https://github.com/alexmbrown/wiremock-ui/blob/main/src/components/ui/misc.tsx)
- **Feature pages are mostly 100–450 lines each.** The largest: `PatternEditors.tsx` 642, `StubForm.tsx` 513, `ResponseSection.tsx` 456, `StubList.tsx` 380, `TrafficPanel.tsx` 375, `ServersPage.tsx` 370, `MessagesPage.tsx` 370. Source: `wc -l`.
- **Testing focus:** tests target the logic most likely to be wrong — matching, number parsing, 4.x behaviour, activity bucketing. Integration tests check the client against real 3.x and 4.x servers in CI. Sources: [src/lib/match.test.ts](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/match.test.ts), [wiremock4.test.ts](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/wiremock4.test.ts), [.github/workflows/deploy.yml](https://github.com/alexmbrown/wiremock-ui/blob/main/.github/workflows/deploy.yml).
- **Good practices in the code:**
  - Typed pure functions with doc comments explaining WireMock quirks (e.g. [match.ts:69-86](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/match.ts) on `Double.parseDouble` semantics; [api.ts:437-453](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/api.ts)).
  - Optimistic cache updates.
  - Undo for deletes, by re-importing with the same ids. See [actions.tsx:93-103](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/stubs/actions.tsx).
  - Accessibility touches: `role`/`aria` on tabs, segmented controls and dialogs; a chart with `aria-label` and keyboard navigation. See [TrafficPanel.tsx:187-198](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/dashboard/TrafficPanel.tsx).
- **Technical debt and limitations:**
  - No TODO/FIXME/HACK markers anywhere outside node_modules (grep). The only "TODO" hits are the "todos" demo data.
  - No ESLint/Prettier installed, despite `eslint-disable` comments ([queries.tsx:17](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/queries.tsx)).
  - No component or E2E tests.
  - The near-miss explanation is a best-effort re-implementation:
    - JSON Schema, binary, multipart and custom/extension matchers show as "evaluated on server";
    - XML placeholders, and date format/truncation, are unknown;
    - Java regex is converted by heuristics.
    - Sources: [match.ts:1-6, 30-42, 291-294, 509-510](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/match.ts), [README.md:92](https://github.com/alexmbrown/wiremock-ui/blob/main/README.md)
  - Polling only, with no push updates. See [README.md:92](https://github.com/alexmbrown/wiremock-ui/blob/main/README.md).
  - The delete-undo trash is in memory only. See [actions.tsx:9-10](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/stubs/actions.tsx).
  - Bulk operations are sequential per-stub PUT/DELETE loops. See [actions.tsx:49-66](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/stubs/actions.tsx).
  - The router is a single flat `Section` enum plus a free-form `param` string ([nav.ts:3-43](https://github.com/alexmbrown/wiremock-ui/blob/main/src/store/nav.ts)), and `App.tsx` switches on it ([App.tsx:98-129](https://github.com/alexmbrown/wiremock-ui/blob/main/src/App.tsx)).
  - `useWm()` reads a single global active project/server ([queries.tsx:21-26](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/queries.tsx)), so pages cannot show two servers at once except Compare, which builds its own clients.
  - Large bundle: dist is 15 MB and the Monaco chunk is 1.37 MB; `chunkSizeWarningLimit` was raised to 4000 ([vite.config.ts](https://github.com/alexmbrown/wiremock-ui/blob/main/vite.config.ts)).
  - A few hard-coded colours outside the tokens (see Q2).
  - The form is long and single-column (author's own known issue, [DESIGN_BRIEF.md:342](https://github.com/alexmbrown/wiremock-ui/blob/main/design/DESIGN_BRIEF.md)).
  - Credentials sit in sessionStorage/localStorage in plain text when "remember" is ticked ([projects.ts:186-241](https://github.com/alexmbrown/wiremock-ui/blob/main/src/store/projects.ts)).

### Inferences
- **Lowest-effort port** is to embed wiremock-ui's `src/lib/*`, `src/components/*`, `src/styles/*` and the feature pages as a "Mock server (WireMock)" module inside /dev/all's React frontend. Three changes are needed:
  - (a) replace `useProjects`/localStorage with /dev/all's project store;
  - (b) route `WireMockClient.raw()` through Tauri (reqwest) to remove CORS and mixed-content concerns;
  - (c) replace `download.ts` with Tauri save/open dialogs.
- **Generalisation candidates for the whole toolkit:** `useWrite()` (read-only/protected guard + confirm + toast + invalidate), the protected/read-only server flags, `confirmDialog`'s type-to-confirm, the Connection doctor pattern, command palette + `g`-key navigation, VS Code-style tabs with preview/dirty/conflict detection (`workspace.ts`), and the virtualised list + detail split.
- **Lifting Tauri's limitations:** wiremock-ui cannot start WireMock, read `mappings/` or get push updates ([README.md:90-92](https://github.com/alexmbrown/wiremock-ui/blob/main/README.md)). Tauri can start or stop a WireMock sidecar (Java or Docker), read and write the root dir directly, and tail logs. Those are natural additions in /dev/all.

### Gaps
- PatternEditors and ResponseSection were not reviewed line by line for internal coupling. They import query hooks (`useCapabilities`, `useMappings`), so they are less standalone than `src/lib`.
- There is no measurement of runtime performance with large stub sets beyond the brief's claim that the list "must stay readable at 1,000+ stubs" ([DESIGN_BRIEF.md:91](https://github.com/alexmbrown/wiremock-ui/blob/main/design/DESIGN_BRIEF.md)).

---

## Q6. Author preferences, docs and roadmap signals in the repo

### Takeaway
The author (Alex Brown) builds AI-assisted, every commit co-authored with Claude, and documents heavily. Recurring preferences:
- zero-install, local/offline operation with no CDN;
- the server as the source of truth;
- dense, keyboard-first, IDE-like UX in **both themes**, with design tokens set up for AI-driven re-theming (a Claude Design brief);
- real-server integration tests in CI against multiple versions;
- explicit safety rails for shared or prod environments;
- candid documentation of limitations and upstream bugs.

There is no roadmap or TODO file. The only forward-looking items are the design brief's polish targets, known issues and requested deliverables.

### Cited Findings
- **Zero install and no CDN:** "Zero install… nothing to install and no backend"; "Monaco (bundled locally, no CDN)"; "Bundles Monaco locally (no CDN) so the app works offline and on locked-down networks." Sources: [README.md:3-5, 75](https://github.com/alexmbrown/wiremock-ui/blob/main/README.md), [monaco.ts:1](https://github.com/alexmbrown/wiremock-ui/blob/main/src/components/editor/monaco.ts).
- **Server as source of truth:** "Nothing to keep in sync… Stubs, folders, tags and notes live in each stub's `metadata` on the WireMock server, so every teammate and every UI sees the same thing." See [README.md:6](https://github.com/alexmbrown/wiremock-ui/blob/main/README.md).
- **Compatibility first:** works with stock WireMock 3.x and the 4.x betas, "No extensions required"; compatible with holomekc/wiremock-gui folders and testsmith `section`. Sources: [README.md:7](https://github.com/alexmbrown/wiremock-ui/blob/main/README.md), [stub-utils.ts:3-9](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/stub-utils.ts).
- **Target users and UX values:** developers, SDETs/QA and platform engineers who "keep the app open for hours… switch between local / QA / staging servers, and value density, speed and the keyboard." See [DESIGN_BRIEF.md:18](https://github.com/alexmbrown/wiremock-ui/blob/main/design/DESIGN_BRIEF.md).
- **What not to change:** the IA; the split-pane + tabs workflow; semantic colours; "Monospace for URLs, ids, header names/values and code"; "Information density — do not make rows or controls taller in compact mode." See [DESIGN_BRIEF.md:352-358](https://github.com/alexmbrown/wiremock-ui/blob/main/design/DESIGN_BRIEF.md).
- **Design workflow built for AI hand-off:**
  - `npm run screenshots` and `npm run design:kit` generate a brief package, catalog and 48 screenshots.
  - §12 holds a ready-to-paste prompt asking Claude Design to "Express colours, type, spacing, radii and shadows as values for the existing CSS custom-property names in tokens.css".
  - Sources: [README.md:69-70, 86-88](https://github.com/alexmbrown/wiremock-ui/blob/main/README.md), [DESIGN_BRIEF.md:368-370](https://github.com/alexmbrown/wiremock-ui/blob/main/design/DESIGN_BRIEF.md)
- **Pending design deliverables (de facto roadmap):**
  - a final light/dark design system;
  - component specs;
  - high-fidelity P0 screens (Stubs workspace, Requests, Near misses, Overview, Command palette, Connection doctor), then P1;
  - a brand mark;
  - IA notes.
  - Source: [DESIGN_BRIEF.md:360-366](https://github.com/alexmbrown/wiremock-ui/blob/main/design/DESIGN_BRIEF.md)
- **Polish targets:**
  - sticky section nav or a two-column stub form;
  - a DevTools-Network-like journal (sticky header, selected-row marker, pretty/raw toggle);
  - a "beautiful and instantly scannable" match report;
  - a better scenario graph layout;
  - friendlier first-run Connection doctor;
  - a visual language for WebSocket vs SSE and received vs sent.
  - Source: [DESIGN_BRIEF.md:123-262](https://github.com/alexmbrown/wiremock-ui/blob/main/design/DESIGN_BRIEF.md)
- **Known issues 1–10:** see [DESIGN_BRIEF.md:339-350](https://github.com/alexmbrown/wiremock-ui/blob/main/design/DESIGN_BRIEF.md).
- **Commit history:**
  - Baseline: a browser-only UI targeting 3.13.
  - Then WireMock 4.x support with detailed beta-bug workarounds.
  - Then opt-in selection, removing the Test request page, and the rebuilt overview traffic panel with validated chart tokens.
  - Then a CI bump.
  - All on 2026-10-20, with `Co-Authored-By: Claude Opus 5.5`. Source: `git log`.
- **Safety-rail preference:** protected servers (confirm every change, type the name for destructive ops), read-only servers, session-only credentials by default, a security callout when a non-local server has no auth, and redacted project exports. Sources: [README.md:13](https://github.com/alexmbrown/wiremock-ui/blob/main/README.md), [queries.tsx:254-290](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/queries.tsx), [projects.ts:263-271](https://github.com/alexmbrown/wiremock-ui/blob/main/src/store/projects.ts), [DESIGN_BRIEF.md:168](https://github.com/alexmbrown/wiremock-ui/blob/main/design/DESIGN_BRIEF.md).
- **Honest limitations section:** [README.md:90-92](https://github.com/alexmbrown/wiremock-ui/blob/main/README.md). Documented upstream WireMock 4 beta bugs: [README.md:94-108](https://github.com/alexmbrown/wiremock-ui/blob/main/README.md).
- **Uses Claude Code tooling:** `.claude/launch.json` for the preview server ([.claude/launch.json](https://github.com/alexmbrown/wiremock-ui/blob/main/.claude/launch.json)).

### Inferences
- /dev/all should carry these values forward:
  - offline and local-first, with all assets bundled;
  - external systems (WireMock, K8s, DBs) remain the source of truth wherever possible;
  - dense, keyboard-first UI in both themes driven by one token file;
  - the same "design brief + screenshots + catalog" hand-off loop;
  - CI integration tests against real services;
  - explicit prod-safety modes.
- The author removed request sending from wiremock-ui to keep its scope "admin only". That suggests the author prefers clear module boundaries, which in /dev/all means HTTP client ↔ mock server cross-links rather than duplicated features.

### Gaps
- No ROADMAP, CHANGELOG, issues export or TODO file exists, and the README does not mention Tauri or /dev/all. The intent to merge into /dev/all comes only from /dev/all's own README, which was not examined here.
- The brief does not show whether the Claude Design redesign was ever produced or applied.
