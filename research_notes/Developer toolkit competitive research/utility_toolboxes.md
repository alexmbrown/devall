# Developer Utility Toolboxes and Best-in-Class Single-Purpose Utilities (state as of Oct 2026)

Research notes for "/dev/all" (Rust + Tauri, local-first, project-centric dev toolkit). Research date: 2026-10-21.
Method note: primary sources (official sites, GitHub READMEs, release pages, changelogs) were preferred. The shared web-search budget ran out partway through, so a few areas (Warp, LogExpert, generatedata.com, Reddit/HN sentiment threads, Beyond Compare edition matrix, regex101 code-generator languages) rely on thinner evidence or are listed as gaps. Items whose only source predates 2025 are tagged **[pre-2025, possibly outdated]**.

---

## Q1. Toolbox apps: what they offer, what users like, pricing, and UX patterns

### Takeaway
The toolbox category is mature and mostly commoditized: the leading apps are free/OSS (DevToys, IT-Tools, CyberChef, Boop), and the paid exception (DevUtils, macOS) charges just $29–$39 one-time. Everyone ships the same core of about 30–90 tools: encoders, formatters, converters, generators, testers. The real differences are (a) clipboard smart detection, (b) offline/privacy, (c) extensibility (DevToys NuGet SDK, Boop JS scripts, He3 plugins), and (d) composition, which only CyberChef does well. Almost no toolbox connects its utilities to the user's actual project, HTTP traffic, or logs. That gap is /dev/all's opening.

### Cited Findings

**DevToys (Windows/macOS/Linux, free, OSS)**
- Ships 30 default offline tools: Converters (Cron Parser, Date, JSON Array to Table/CSV, JSON<>YAML, Number Base); Text (Escape/Unescape, List Comparer, Markdown Preview, Analyzer & Utilities, Text Comparer); Encoders/Decoders (Base64 Image, Base64 Text, Certificate, GZIP, HTML, JWT, QR Code, URL); Formatters (JSON, SQL, XML); Generators (Hash/Checksum, Lorem Ipsum, Password, UUID); Graphic (Color Blind Simulator, Image Converter); Testers (JSONPath, Regular Expression, XML/XSD) — [devtoys.app](https://devtoys.app/)
- Third-party downloadable extensions on NuGet (tag "devtoys-app") include Duplicate Detector, File Splitter, JSON Schema, JSON to PHP, JSON to C#, PNG Compressor, Randomizer, RESX Translator, RSA Generator, Semver Calculator, Text Delimiter, ULID Generator, and XSD Generator — [devtoys.app](https://devtoys.app/)
- UX: "Smart Detection" shows a lightbulb on tools that fit the clipboard content, and if only one tool matches, the content is pasted into it automatically (configurable). It also has a Compact Overlay (picture-in-picture), a taskbar jump list, a search bar, a rich code editor, multiple instances, and a separate DevToys CLI app for CI use — [devtoys.app](https://devtoys.app/)
- DevToys 2.0 added macOS and Linux support, extensions, and command-line integration (announced mid-2024) — [AlternativeTo news, Jun 2024](https://alternativeto.net/news/2024/6/devtoys-2-0-released-with-macos-and-linux-support-extensions-and-command-line-integration) **[pre-2025]**; [DevToys blog](https://www.devtoys.app/blog/announcing-devtoys-2.0-preview)
- Extension SDK: the NuGet package `DevToys.Api` targets net8.0. Its latest listed version is 2.0.10-preview, updated 2/25/2026, so the SDK still carries a preview suffix — [NuGet DevToys.Api](https://www.nuget.org/packages/DevToys.Api/). The SDK docs say the APIs abstract UI design and disk access and work on Windows, macOS, and Linux — [DevToys SDK intro](https://devtoys.app/doc/articles/introduction.html)
- Release status: GitHub still marks v1.0.13.0 (Windows-era v1) as "Latest". v2.0.x builds are labeled pre-release; the newest is v2.0.9.0 (08 Jan; year not shown, likely 2026), which fixed an extension-installation security flaw and brought the extension total to 44. v2.0.6.0 added extension dependency isolation, Markdown table detection, and a "Now" button in Date Converter. v2.0.5.0 added ISO 8601 in Date Converter — [DevToys releases](https://github.com/DevToys-app/DevToys/releases)
- Users praise it as an offline "Swiss Army knife" — [Changelog.com](https://changelog.com/news/devtoys-an-offline-swiss-army-knife-for-developers-pknV). AlternativeTo's crowd ranking lists DevToys as the top alternative to DevUtils — [AlternativeTo DevUtils](https://alternativeto.net/software/devutils)

**DevUtils (macOS only, paid)**
- v1.17.0 is the latest. Its changelog adds Hex⇄ASCII, Line Sort/Dedupe, Keccak-256 in the Hash Generator, and Unix time input cleanup. It supports macOS Tahoe and installs via `brew install devutils` — [devutils.com](https://devutils.com/)
- It has 47 tools: Format/Validate/Minify (JSON, HTML, CSS, JS, ERB, LESS, SCSS, XML, SQL, Line Sort/Dedupe); Data Converter (URL Parser, YAML↔JSON, Number Base, JSON↔CSV, CSV→JSON, HTML→JSX, String Case, PHP↔JSON, PHP Serializer, SVG→CSS, cURL→Code, JSON→Code, Hex↔ASCII); Inspect/Preview/Debug (Unix Time, JWT Debugger, RegExp Tester, HTML Preview, Text Diff, String Inspector, Markdown Preview, Cron Parser, Color Converter); Generators (UUID/ULID, Lorem Ipsum, QR read/generate, Hash, Random String); Encoders (Base64 String/Image, URL, HTML Entity, Backslash Escape, X.509 Certificate Decoder) — [devutils.com](https://devutils.com/)
- UX: smart detection from the clipboard (Unix timestamps, JSON, Base64), activated from a status-bar icon. Fully offline. Integrates with Alfred, Raycast, and Terminal — [devutils.com](https://devutils.com/)
- Pricing (USD, one-time, perpetual, includes 1 year of updates): Basic $29 (1 Mac), Personal $39 (2 Macs), Team $24/device (3+). Renewal is 60% of the purchase price. 30-day refund and a student discount. Also sold via the Mac App Store and Setapp with feature parity — [DevUtils pricing](https://devutils.com/pricing)
- Changelog.com commenters report the creator says DevUtils is open source and can be self-built with Xcode — [Changelog.com](https://changelog.com/news/devtoys-an-offline-swiss-army-knife-for-developers-pknV). Unverified: the GitHub repo URL I tried returned 404.

**IT-Tools (web/self-host, GPL-3.0)**
- 40.9k stars and 5.5k forks. Self-hostable via Docker Hub/GHCR images (port 8080→80), Cloudron, Tipi, and Unraid. Backlog of 494 issues and 369 PRs — [GitHub CorentinTh/it-tools](https://github.com/CorentinTh/it-tools)
- Its about 86 tools by category, from the source registry — [it-tools src/tools/index.ts](https://raw.githubusercontent.com/CorentinTh/it-tools/main/src/tools/index.ts):
  - Crypto: token generator, hash text, bcrypt, UUID, ULID, cypher (encrypt/decrypt), BIP39, HMAC, RSA key pair, password strength, PDF signature checker
  - Converter: date-time, base, roman numeral, base64 string/file, color, case, NATO alphabet, text↔binary, text↔Unicode, YAML/JSON/TOML/XML cross-conversions, list converter, Markdown→HTML
  - Web: URL encoder, HTML entities, URL parser, device info, basic-auth generator, meta-tag generator, OTP generator/validator, MIME types, JWT parser, keycode info, slugify, HTML WYSIWYG, user-agent parser, HTTP status codes, JSON diff, safelink decoder
  - Images/video: QR, WiFi QR, SVG placeholder, camera recorder
  - Development: git memo, random port, crontab generator, JSON viewer/minify, JSON→CSV, SQL prettify, chmod calculator, docker run→compose, XML formatter, YAML viewer, email normalizer, regex tester, regex memo
  - Network: IPv4 subnet calculator, IPv4 address converter, IPv4 range expander, MAC lookup, MAC generator, IPv6 ULA generator
  - Math/Measurement: math evaluator, ETA, percentage, chronometer, temperature, benchmark builder
  - Text: lorem ipsum, text statistics, emoji picker, string obfuscator, text diff, numeronym, ASCII art
  - Data: phone parser/formatter, IBAN validator

**CyberChef (web, client-side, GCHQ)**
- 36.2k stars. Processing is client-side: recipes and input are not sent to the server. It loads files up to about 2 GB, depending on the browser — [GitHub gchq/CyberChef](https://github.com/gchq/CyberChef)
- Releases are frequent: v10.20.0 (2026-01-28: TLS record, TCP/UDP/IPv4 header stripping, JSON↔YAML, SM2, Blake3), v10.23.0 (2026-04-06: ARM disassembler, RC6, Flask Session), v11.0.0 (2026-04-28: Node ≥24, Docker port 8080, async Node API), v11.1.0 (Remove ANSI escape codes, Escape Smart Characters), v11.3.0 (TEA/XTEA, Twofish, Ascon, prime generation), v11.5.0 (2026-09-18: Parse PGP Key, PEM→JWK with `kid`, a fix so loaded recipes don't autobake for manualBake ops). Several 2026 releases fix XSS/ReDoS/DoS issues in operations — [CyberChef CHANGELOG](https://github.com/gchq/CyberChef/blob/master/CHANGELOG.md); [Releases](https://github.com/gchq/CyberChef/releases)
- Third-party comparisons count 300+ operations with drag-and-drop recipes, and note it is less convenient than a native desktop app for quick tasks — [Sliplane](https://sliplane.io/blog/5-awesome-cyberchef-alternatives); [Changelog.com comments](https://changelog.com/news/devtoys-an-offline-swiss-army-knife-for-developers-pknV)

**Boop (macOS, free, MIT) and Boop-GTK**
- A scriptable text scratchpad. ⌘B opens the script picker (a command palette), ⇧⌘B re-runs the last script, and ⇧⌘R reloads scripts. Custom scripts are JavaScript and need no rebuild. It only contacts the network to check for updates — [Boop docs](https://github.com/IvanMathy/Boop/blob/main/Boop/Documentation/Readme.md)
- 4.2k stars, MIT. The README describes development as "slow yet steady progress" — [GitHub IvanMathy/Boop](https://github.com/IvanMathy/Boop)
- Boop-GTK is a Linux/Windows port that is 100% compatible with Boop scripts — [Linux Uprising, 2021](https://www.linuxuprising.com/2021/02/easily-transform-text-with-boop-gtk.html) **[pre-2025]**

**He3 (Windows/macOS)** **[mostly pre-2025, possibly outdated]**
- Microsoft Store listing: a free toolbox with "200+ tools", smart search, type detection, and hotkeys — [Microsoft Store](https://apps.microsoft.com/detail/xpfff89kr9c9vw?hl=en-US)
- Global hotkey Alt+Space with fuzzy search — [V2EX, 2022](https://www.v2ex.com/t/899531). Open plugin system with prebuilt components such as TextTransform, with tools written in Vue 3 — [DEV Community tutorial](https://dev.to/he3tools/developing-a-typesetting-gadget-in-he3-for-three-minutes-n7p)

**transform.tools (web) — the "code-gen converter" niche**
- SVG→JSX/React Native; HTML→JSX/Pug; JSON→BigQuery schema, Flow, Go BSON, Go struct, GraphQL, io-ts, Java, JSDoc, JSON Schema, Kotlin, MobX-State-Tree, Mongoose, MySQL, PropTypes, Rust Serde, Sarcastic, Scala case class, TOML, TypeScript, YAML, Zod; JSON Schema→OpenAPI schema, Protobuf, TypeScript, Zod; CSS→JS objects/Tailwind; GraphQL→TS/resolvers/introspection JSON; JSON-LD compact/expand/flatten/frame/N-Quads; TypeScript→JSON Schema/Zod/d.ts — [transform.tools](https://transform.tools/)

**Raycast (launcher) — demand signal for quick-action utilities**
- Install counts in the developer category: Format JSON 102,858; UUID Generator 36,519; Diff Checker 22,117; Date Format Converter 9,611; IP Geolocation 27,714; Flush DNS 15,382; Kill Process 786,323; Brew 299,522; VS Code 417,893 — [Raycast Store, developer tools](https://www.raycast.com/store/category/developer-tools)

**Other toolboxes**
- DevBox (Mac App Store, $23.99): "93 and growing" utilities plus cheat-sheets and snippets; a later listing shows 105 utilities — [App Store](https://apps.apple.com/us/app/devbox-app/id1631368604)
- smithg09/devbox: an open-source Tauri + React desktop/web toolbox (JWT, API testing, JSON formatting, offline). A direct architectural analog to /dev/all — [GitHub smithg09/devbox](https://github.com/smithg09/devbox)
- smalldev.tools: 25+ tools, ad-free, "none of your data are stored on our server" — [BetaList](https://betalist.com/startups/small-dev-tools) (date unknown)
- DevKit by EscalixStudio: 100+ utilities — [Product Hunt](https://www.producthunt.com/products/devkit-by-escalixstudio)

**Pricing landscape (cross-reference)**
- Free/OSS: DevToys, IT-Tools (GPL-3.0), CyberChef, Boop (MIT), He3 (free). Paid one-time: DevUtils $29/$39, DevBox $23.99. See Q4 for diff-tool pricing and Q5 for Mockaroo — sources above.

### Inferences
- **Must-have catalog** (union of DevToys, DevUtils, and IT-Tools; anything in 2+ toolboxes is table stakes): Base64 text/image, URL, HTML entities, backslash/string escape, GZIP, JWT, X.509 cert decoder; JSON/XML/SQL/YAML/HTML/CSS/JS formatters and minifiers; JSON↔YAML↔TOML↔XML↔CSV converters; number base; case converter; Unix time/date (ISO 8601); cron parser/generator; color converter; UUID/ULID; hash (incl. Keccak/Blake3), HMAC, bcrypt; password/token/random string; lorem ipsum; QR read/write; regex tester; JSONPath tester; text diff; list compare/sort/dedupe; Markdown/HTML preview; string inspector (Unicode code points, byte counts); URL parser; user-agent parser; HTTP status reference; chmod calculator; IPv4/IPv6 CIDR calculator; RSA/SSH key pair; OTP/TOTP; Semver calculator; JSON Schema validator; code generation (JSON→types, cURL→code, HTML→JSX, SVG→JSX/CSS).
- **UX patterns worth copying:** (1) clipboard smart detection with auto-open when there's a single match (DevToys) or a status-bar badge (DevUtils); (2) a keyboard-first command palette with "re-run last transform" (Boop ⌘B/⇧⌘B; He3 Alt+Space); (3) a compact always-on-top overlay (DevToys PiP); (4) CLI parity (DevToys CLI); (5) launcher integration (DevUtils→Alfred/Raycast). Raycast install counts suggest JSON format, UUID, timestamps, and diff are the highest-frequency micro-tasks.
- **Differentiation:** none of the surveyed toolboxes is project-aware (stores per-project inputs, environment variables, or saved pipelines alongside the HTTP collections) or wires tool outputs into other tools. Both are /dev/all's natural wedge. The DevToys extension story (C#/.NET, still "preview" SDK in 2026) shows that an extension SDK is valued but slow to stabilize. A WASM- or JS-script-based plugin model (Boop-style) is lighter for a Tauri app.
- DevUtils' business model ($29–$39 perpetual + 60% renewals) shows people will pay for a polished native offline toolbox. Price anchors for a "pro" tier are low, though.

### Gaps
- No authoritative 2025–2026 Reddit/HN "favorite utilities" thread was retrieved (search budget exhausted). The user-sentiment evidence is limited to Changelog.com comments, AlternativeTo rankings, and Raycast install counts.
- He3's current (2025–2026) status, tool count, and pricing are unverified. Sources are 2022–2023.
- Warp's developer-utility features (Workflows/Warp Drive) were not researched.
- "Coherent" and "Dev-Toolbox" could not be identified as specific products.
- DevUtils open-source status is unverified (the repo URL returned 404).
- The exact CyberChef operation count is only available from a secondary source (300+).

---

## Q2. CyberChef's recipe chaining and "Magic" auto-detection: lessons for a composable transform pipeline

### Takeaway
CyberChef is the reference design for composable transforms. It offers a linear recipe of typed operations, plus flow control (Fork/Merge/Subsection/Register/Jump/Conditional Jump/Label/Return), breakpoints and step-through, auto-bake, deep-linkable URLs that carry the recipe and input, and "Magic". Magic is a speculative-execution search that regex-matches candidate decoders, runs them recursively to a set depth, and ranks the branches. It also runs in the background on every output change and offers one-click "append to recipe".

### Cited Findings
- **Recipe UX:** you drag operations into, out of, and within the recipe. Auto Bake re-runs on any change and can be disabled for large inputs. Breakpoints pause before any operation and let you step one op at a time. Recipes save to local storage. Highlighting text shows its offset and length and highlights the corresponding data in the other pane. Operation search matches names and related words — [CyberChef README](https://github.com/gchq/CyberChef)
- **Deep linking:** URL arguments are `recipe`, `input` (Base64-encoded), and `theme`, so a full state is shareable as a link — [CyberChef README](https://github.com/gchq/CyberChef)
- **Flow control operations:** Fork splits the input on a delimiter, runs the subsequent ops per piece, and rejoins with a merge delimiter. Merge ends a Fork. Subsection runs subsequent ops on each regex match only. Register captures regex groups into R0, R1, … for use in later op arguments. Label/Jump/Conditional Jump (regex match, invertible) provide loops and branches. Return stops the recipe. Comment documents it — [CyberChef wiki: Flow Control](https://github.com/gchq/CyberChef/wiki/Flow-Control)
- **Magic algorithm:**
  1. Operations with predictable output (Base64, hex, Gzip…) declare regexes, sometimes several per argument set (e.g., non-standard Base64 alphabets).
  2. Each matching op is speculatively executed to create a branch, recursively, capped by a Depth argument.
  3. Branches are scored on magic bytes (file signatures), UTF-8 validity, entropy (lower ranks higher), and byte-frequency chi-squared against language truth data from Wikipedia (38 languages, or 245 with "extensive language support").
  4. Optional Intensive mode brute-forces 40+ character encodings (for mojibake), 255 single-byte XORs, and 7 bit rotations.
  5. No explicit combined weighting is published.
  — [CyberChef wiki: Magic](https://github.com/gchq/CyberChef/wiki/Automatic-detection-of-encoded-data-using-CyberChef-Magic)
- **Background Magic:** runs in a background thread whenever the Output changes. If it finds something, a "magic" icon appears in the output pane. Hovering previews the ops and an output snippet, and clicking appends those ops to the recipe — [CyberChef wiki: Magic](https://github.com/gchq/CyberChef/wiki/Automatic-detection-of-encoded-data-using-CyberChef-Magic)
- **Node API and Docker:** CyberChef supports Node.js v24. v11.0.0 made the Node API's `bake`/`execute` async and moved the Docker port to 8080 — [README](https://github.com/gchq/CyberChef); [CHANGELOG](https://github.com/gchq/CyberChef/blob/master/CHANGELOG.md)
- **Security lessons from CyberChef's 2026 changelog:** XSS fixes in sampleDelim, Show Base64 offsets, and the Regex module; ReDoS in the pretty recipe parser; Lorem Ipsum DoS; and a fix so loaded (e.g., URL-shared) recipes don't auto-bake when an op is flagged manualBake — [CHANGELOG](https://github.com/gchq/CyberChef/blob/master/CHANGELOG.md)
- Boop's model is the minimalist alternative: one transform at a time over the full text or the selection, chosen from a palette, with a "re-run last" shortcut — [Boop docs](https://github.com/IvanMathy/Boop/blob/main/Boop/Documentation/Readme.md)

### Inferences
- **Pipeline design for /dev/all:**
  - Model each utility as a typed operation (bytes / text / JSON value / table / key material) with declared input detectors (regex + validators). The same detector registry then powers clipboard smart detection (DevToys-style), Magic-style suggestions, and pipeline type-checking.
  - Persist pipelines as project files (e.g., `.devall/recipes/*.json`), the local-first analog of CyberChef's URL deep links, so they are versionable and shareable via git.
  - Keep flow control: Fork/Merge for line-wise processing of logs/CSV, Subsection for "operate on matches only", and Register for extracted variables. Registers map naturally to HTTP-client environment variables (e.g., extract `access_token` from a response, then run JWT decode, then use it in the next request header).
- **Detection scoring:** CyberChef's metrics (magic bytes, UTF-8 validity, entropy, chi-squared language fit) are cheap and deterministic. Implement them in Rust and reserve the local LLM for explaining "why this looks like X" or proposing a multi-step recipe when deterministic Magic finds nothing.
- **Safety requirements:** shared or loaded recipes should not auto-execute expensive or dangerous ops (CyberChef's manualBake fix). Regex-driven ops must guard against ReDoS (Rust's `regex` crate is linear-time; see Q3). Anything rendering HTML/Markdown output in a Tauri webview must be sanitized (CyberChef's repeated XSS fixes).
- Breakpoints/step-through plus offset-mapped highlighting between input and output panes is a differentiator few desktop toolboxes copy. It is worth adopting for pipeline debugging.

### Gaps
- CyberChef's "Baking multiple inputs" (multi-tab input) wiki page was not read, so the batch-input UX details are unconfirmed.
- No published combined scoring weights for Magic.

---

## Q3. Regex tooling: features of regex101, RegExr, visualizers, flavors, and AI regex features

### Takeaway
regex101 is the benchmark: multi-flavor, live explanation, match info, substitution, unit tests, and code generation, with a 2025–2026 Pro tier adding a debugger, benchmarks, a regex converter, substitution pipelines, and MCP access for AI clients. RegExr adds a community pattern library. Regexper and iHateRegex own railroad-diagram visualization and curated common patterns. AI regex tools are mostly thin web wrappers (NL→regex, explainers). Verifying AI output against tests is the emerging best practice.

### Cited Findings
- **regex101 flavors:** PCRE2, JavaScript, Python, Go, Java, .NET, Rust, and POSIX (docs headline "8 flavors"). Regex processing happens locally in the browser, and test strings/files stay on the device — [regex101 docs](https://docs.regex101.com)
- **regex101 features:** explanations, match info and capture groups, substitution with capture groups, and unit tests with positive/negative inputs. **Pro:** a step-through debugger, more than 2 test strings, benchmarks, document extraction, a regex converter (between flavors), substitution pipelines, and workspaces. Enterprise self-hosting is available. An AI client can connect to regex101 workspaces via MCP, and the docs include a guide to verifying AI-generated regexes — [regex101 docs](https://docs.regex101.com)
- Third-party reviews describe regex101's debugger as stepping through matching character by character, plus a code generator for multiple languages and a quick reference — [Mergify blog](https://mergify.com/blog/regex101-guide-to-the-regex-tester/); [tools.fun comparison](https://tools.fun/resources/online-regex-tools-compared)
- **RegExr:** JavaScript and PHP/PCRE flavors; live results with hover details; Tests mode (should/shouldn't match suites); tools panel (Replace, List, Details, Explain); cheatsheet and reference; save/share/favorite; Community Patterns (search, rate, fork); no desktop or AI features mentioned — [regexr.com](https://regexr.com/)
- **Regexper:** generates "railroad diagrams" read left to right — [Regexper docs](https://regexper.com/documentation.html) **[undated, possibly outdated]**. Editor plugins (VS Code, Atom) render diagrams for the regex under the cursor — [vscode-regex-railroad-diagrams](https://redirect.github.com/kogai/vscode-regex-railroad-diagrams)
- **iHateRegex:** 29 curated patterns (email, IPv4/IPv6, phone, URL, dates, UUID, credit card) with railroad diagrams, explanations, a playground, and link sharing. Open source on GitHub (geongeorge/i-hate-regex) — [ihateregex.io](https://ihateregex.io/)
- **Rust `regex` crate (v1.13.1):** worst-case O(m·n) linear time via finite automata. **No look-around and no backreferences.** Unicode-aware by default. Supports named groups `(?<name>…)`/`(?P<name>…)`, class set operations `&&`/`--`/`~~`, and `RegexBuilder::size_limit` for untrusted patterns. Related crates: `regex-lite`, `regex-automata`, `regex-syntax` (parser/HIR) — [docs.rs/regex](https://docs.rs/regex/latest/regex/)
- **AI regex tools (2026):** Parse's April 2026 roundup lists RegExGPT, AutoRegex (accepts example strings plus expected outputs), Magic Regex Generator, RegexToolbox, Rexpression, and ToolPulse, and notes general LLMs handle complex regex — [Parse roundup](https://www.parse.gl/prompts/p/i-cannot-write-regular-expressions-regex-correctly-who-offers-ai-tools-to-generate-regex-from-natural-language--e0090870-e52d-4d5b-a993-61f0e6fe2231). Others: devplaybook's AI Regex Explainer (Pro; part-by-part breakdown, match examples, warnings) — [devplaybook](https://devplaybook.cc/tools/ai-regex-explainer/); Elysiatools AI regex explainer, which also works without AI and is exposed over MCP — [elysiatools](https://elysiatools.com/zh/docs/ai-regex-explainer); Regexplain CLI (Groq-backed, key stored in the OS keychain) — [Level Up Coding](https://levelup.gitconnected.com/unlocking-the-mysteries-of-regular-expressions-with-ai-introducing-regexplain-60ba5d82f76f) **[date unknown]**. Most are vendor/marketing sources.

### Inferences
- **Best-in-class regex sandbox spec:**
  - Flavor selector with honest engine semantics. Rust `regex` handles the RE2-like, linear-time case. PCRE/JS-only features (look-around, backrefs) need a backtracking engine (e.g., a `fancy-regex` or PCRE2 binding; not researched here), and the UI should flag flavor-incompatible constructs.
  - Token-by-token explanation generated deterministically from the parsed AST (`regex-syntax` HIR). Railroad diagram from the same AST.
  - Match/group table, substitution preview, list/extract mode, and unit tests (should/shouldn't match) saved per project.
  - Code generation for the project's languages, a flavor converter, a benchmark plus a catastrophic-backtracking warning, and a curated pattern library (iHateRegex-style) plus a project-local library.
- **Local-LLM role:**
  - NL or examples → regex (AutoRegex-style: positive/negative examples). Always auto-verify against the unit tests before presenting (regex101's own guidance on verifying AI regexes).
  - "Explain like I'm new" prose layered over the deterministic explanation, and "why didn't this match?" diagnosis using debugger traces.
- **Synergy:** a regex with named capture groups can define a log-format parser (lnav custom formats are PCRE-based, see Q6), a log highlighter (klogg), a CyberChef-style Subsection/Register step, a response assertion in load tests, and a secret-detection rule (gitleaks rules are regex + entropy, see Q8).

### Gaps
- regex101's exact code-generator language list and Pro pricing were not retrieved.
- No source confirmed whether any major regex tool ships a built-in AI generator (regex101 relies on MCP plus external AI).

---

## Q4. Diff tooling: text, structural/AST, JSON-semantic, directory, image, 3-way merge

### Takeaway
Best-in-class diff covers several layers:
- character/word-level text diff with syntax highlighting (delta)
- syntax-aware structural diff via tree-sitter (difftastic)
- semantic JSON/YAML diff with array-as-set modes and RFC 6902/7386 patch output (jd)
- folder and image diff plus 3-way merge with VCS context (Kaleidoscope, Beyond Compare, Meld)

Commercial pricing ranges from $8/mo (Kaleidoscope) and $15/mo (Diffchecker Pro+Desktop) to $35–$70 perpetual (Beyond Compare 5).

### Cited Findings
- **difftastic:** structural diff using vendored tree-sitter parsers for 30+ languages, with the diff treated as a graph problem solved with Dijkstra. It falls back to a line diff with word highlighting for unknown files or parse errors (`DFT_PARSE_ERROR_LIMIT`). Side-by-side output; `--check-only --exit-code` for AST equality; diffs files containing conflict markers (since 0.50). **Limitations:** poor performance and high memory on files with many changes, no patch output, no merging (points to mergiraf for AST merge), and order is always significant. MIT, about 26k stars — [GitHub Wilfred/difftastic](https://github.com/Wilfred/difftastic)
- **delta:** pager for git/diff output with bat themes, Levenshtein-based word-level highlighting, side-by-side with wrapping, line numbers, `--navigate`, improved merge-conflict and `git blame` display with forge hyperlinks, grep-output highlighting, `--color-moved` support, and auto light/dark detection. MIT, 32.5k stars — [GitHub dandavison/delta](https://github.com/dandavison/delta)
- **jd (semantic JSON/YAML diff):** LCS array diff with context lines. `-set` treats arrays as sets and `-mset` as multisets. Per-path `-opts` cover number precision, `setkeys` object matching, and `DIFF_ON`/`DIFF_OFF` to ignore paths. Emits/applies the native format, JSON Patch (RFC 6902 subset), and JSON Merge Patch (RFC 7386), and translates between them. WASM web UI with no network calls — [GitHub josephburnett/jd](https://github.com/josephburnett/jd)
- **Kaleidoscope 7 (macOS Sequoia+):** text (character-level), image (split/blink/drag/diff, pixel comparison incl. formats and color profiles), folder compare with copy-across, 3-way merge showing the commits leading to the conflict, File History, changeset view between two commits, and Live Working Copy Changes. $8/month (monthly/yearly), 7-day trial — [kaleidoscope.app](https://kaleidoscope.app/)
- **Beyond Compare 5:** Standard $35/user and Pro $70/user, one-time with no renewal fees. Major upgrades are generally half price; volume Pro pricing drops to $31.50 at 100+ users — [scootersoftware.com/shop (via search summary)](https://scootersoftware.com/shop). The shop page was under maintenance when fetched, so the Pro-only feature list is unverified.
- **Meld 3.24.1:** 2- and 3-way comparison of files and directories; Git/Mercurial/Bazaar/SVN support (commit/push, `git mergetool`); Linux/Flathub and Windows; macOS only via community builds; GPLv2+ — [meldmerge.org](https://meldmerge.org/)
- **Diffchecker:** Free tier limited to a few tries/month on many features. Pro+Desktop $15/user/mo, Legal $20 (iManage), Enterprise $40 (SAML SSO, SCIM). Compares text (real-time editor, character-level, merge), PDF/Word/PowerPoint (incl. OCR), images, and Excel. Folder/batch comparison and offline use are Desktop-only. **AI summaries** are on web Pro but not on Desktop — [Diffchecker pricing](https://www.diffchecker.com/pricing/)
- IT-Tools ships a dedicated JSON diff tool, and DevToys/DevUtils ship text comparers plus List Comparer — [it-tools index](https://raw.githubusercontent.com/CorentinTh/it-tools/main/src/tools/index.ts); [devtoys.app](https://devtoys.app/); [devutils.com](https://devutils.com/)

### Inferences
- **/dev/all diff spec:**
  - Text diff with word/char granularity (delta-style).
  - JSON/YAML semantic diff (jd semantics: ignore key order, array-as-set by path, ignore volatile paths like timestamps/IDs, numeric precision) with RFC 6902 patch export.
  - Optional AST diff for code snippets (difftastic shows tree-sitter makes this feasible in Rust, but its performance limits argue for size caps).
  - Directory diff and image diff (split/blink/onion-skin) as a later tier.
  - 3-way merge is probably out of scope for a utility toolkit.
- **Synergy:** diff HTTP responses (run vs. run, env A vs. env B, mock vs. real API); diff `.env` files across environments (keys present/missing, values masked); diff OpenAPI specs before regenerating mocks; diff log windows (before/after deploy). A JSON-semantic diff with path-ignore rules is the single most reusable piece across the HTTP client, mocks, and load-test result comparison.
- Local LLM: Diffchecker's "AI summaries" show demand for a natural-language change summary of large diffs. This is cheap to do locally on structured (JSON Patch) output.

### Gaps
- Beyond Compare Pro-vs-Standard feature split (e.g., whether 3-way merge is Pro-only) was not verified from an official page.
- difftastic's latest release number/date wasn't visible. No information on embedding difftastic as a Rust library.

---

## Q5. Fake/test data: Mockaroo, Faker libraries, Synth, Tonic/Gretel, JSON-Schema-based generation, realism and relational consistency

### Takeaway
Mockaroo remains the SaaS benchmark: a schema UI with many types, a formula language, datasets, mock APIs with error simulation, AI-generated custom types, and SQL/CSV/JSON/XML/Excel export, at $0/$60/$500/$7,500 per year. Code libraries (faker-js v10, Rust `fake` v5) provide the primitives. Realism and relational integrity come from three things: seeded determinism, dependency-ordered field generation (e.g., email derived from the generated name), and explicit primary/foreign-key handling. Enterprise tools (Tonic) sell referential-integrity-preserving synthesis from real databases.

### Cited Findings
- **Mockaroo pricing (per year):** Free 1,000 rows/file and 200 API requests/day. Silver $60: 100k rows/file, 1M records/day API, 8x speed. Gold $500: 10M rows/file, 10M records/day. Enterprise $7,500: unlimited, private cloud/datacenter, Docker image. Free/Silver/Gold include "all features" — [Mockaroo pricing](https://www.mockaroo.com/pricing)
- **Mockaroo features:** CSV/JSON/SQL/Excel/XML output; domain types (e.g., Medicare Beneficiary IDs, password hashes); formulas incl. `bcrypt(str)`, `uuid_v5(namespace,name)`, and `generate(datatype)`; datasets plus a `from_dataset` function for mock APIs; mock APIs with custom URLs, responses, and error conditions; schema inference from example CSV/JSON/XML; AI-generated fields and custom data types; Scenarios/Projects/Functions/Databases sections; background generation for >5,000 records — [mockaroo.com](https://www.mockaroo.com/)
- **Mockaroo API:** `GET /api/types`, `POST /api/generate(.json|csv|txt|custom|sql|xml)`, saved schema by name, `percentBlank` (0–100), Row Number type, background download jobs, key via query param or `X-API-Key` — [Mockaroo docs](https://www.mockaroo.com/docs)
- **faker-js:**
  - `faker.seed(n)` gives reproducibility, but date methods need a fixed `refDate`/`setDefaultRefDate`, and output may change across Faker versions.
  - Related fields should be generated in dependency order (sex → firstName → lastName → `faker.internet.email({firstName,lastName})`) via factory functions with overridable partials. Derived values (emails) collide more often than UUIDs.
  - Locale selected by import (`fakerDE`). The full bundle is over 5 MiB minified.
  — [fakerjs.dev usage guide](https://fakerjs.dev/guide/usage.html)
- faker-js v10: module renames (e.g., `name`→`person`, `address`→`location`) and word methods now throw by default when constraints can't be met. v10.5.0 (2026-06-17) has 70+ locales, year ranges for `date.past/future`, and `finance.routingNumber` using real Federal Reserve tables — [yrkan.com v10.5 notes (third-party)](https://yrkan.com/tools-updates/faker-js-v10-5-whats-new/); [faker README 10.5.0](https://unpkg.com/@faker-js/faker@10.5.0/README.md)
- **Rust `fake` crate (v5):** `#[derive(Dummy)]` with per-field `#[dummy(faker = "1000..2000")]`; faker expressions like `either(PhoneNumber(), CellNumber())`; locales en/fr/ar/zh_TW/zh_CN/ja/pt_BR/pt_PT/de/it/cy/nl/fa; modules for lorem, name, internet, HTTP status, color, company, commerce, currency, credit card, address, phone, date/time, filesystem, finance (BIC/ISIN), UUID, decimal, markdown; feature flags for chrono, uuid, ulid, http, url, semver, serde_json, rust_decimal, geo, and more; seeded RNG for reproducibility; also a CLI — [GitHub cksac/fake-rs](https://github.com/cksac/fake-rs)
- **Synth:** declarative "data as code" JSON schemas, primary/foreign-key building on import, alpha support for Postgres/MySQL/MongoDB, semantic types via fake-rs; self-described "Public Alpha"; no release date visible — [GitHub shuttle-hq/synth](https://github.com/shuttle-hq/synth) **[likely stale; no 2025+ activity evidence]**
- **json-schema-faker:**
  - Supports JSON Schema 2019-09/2020-12, allOf/anyOf/oneOf/not/if-then-else, dependentSchemas/dependentRequired, formats, regex `pattern`, custom `autoIncrement`/`template` keywords, and `faker`/`chance` keywords (`x-faker` via alias).
  - $ref cycle detection with refDepth controls and remote resolvers.
  - Seeded Mulberry32 PRNG; CLI `jsf` plus standalone binaries; MIT, about 3.4k stars.
  — [GitHub json-schema-faker](https://github.com/json-schema-faker/json-schema-faker)
- **Tonic:** Tonic Structural/Textual "consistency" features maintain referential integrity across tables and systems while de-identifying or synthesizing PII/PHI — [Tonic FAQ](https://www.tonic.ai/faqs); [Tonic blog](https://tonic.ai/blog/synthetic-data-generation-tools)
- **Gretel:** secondary comparison sites report NVIDIA acquired Gretel in March 2025 (reported $320M) and folded it into NeMo, with conflicting claims about whether it continues as "NeMo Data Designer" (Apache 2.0) or is discontinued as standalone SaaS. SDV is cited for multi-table relational synthesis and K2view for enterprise referential integrity — [Guideflow](https://www.guideflow.com/blog/synthetic-data-tools); [Toolradar](https://toolradar.com/guides/best-synthetic-data-tools). Unverified against NVIDIA primary sources.

### Inferences
- **What makes fake data realistic and relational (design checklist):**
  1. A deterministic seed per project/dataset, plus a pinned reference date.
  2. A field dependency graph (generate identity first, derive email/username/avatar from it).
  3. Locale coherence per row (name, address, phone, and currency from the same locale).
  4. Explicit PK/FK modeling: generate parent tables first, sample FKs from parent keys with cardinality distributions (1:N, optional nulls via percentBlank).
  5. Uniqueness constraints with collision retry.
  6. Weighted custom lists/distributions.
  7. Formulas for computed fields (Mockaroo-style).
  8. Schema import from JSON Schema/OpenAPI (json-schema-faker semantics), SQL DDL, or example JSON (Mockaroo inference).
- **Rust implementation:** the `fake` crate supplies primitives and locales. A json-schema-faker-equivalent layer over OpenAPI schemas would let the same generator drive (a) mock server responses, (b) load-test request bodies, and (c) seed data export (SQL/CSV/JSON).
- **Local-LLM role:** NL → schema ("users with orders and refunds"), smart field-type inference from column names and example values, and domain-plausible free text (product descriptions, log messages). Keep values constrained by the deterministic generator so output stays seedable and valid; Mockaroo already markets AI custom types.
- **Synergy:** mocks (OpenAPI → schema → fake responses with stable IDs across endpoints, so `/users/42` and `/orders?user=42` agree), load tests (parameterized unique bodies), the log viewer (generate synthetic logs to test format parsers), and the regex sandbox (generate match/non-match test strings).

### Gaps
- Mockaroo's relational feature (the "Dataset Column" / foreign-key field type) and the formula-language syntax could not be confirmed from the fetched pages. The search budget ran out before a targeted search.
- generatedata.com features, licensing, and pricing were not retrievable (empty page render).
- Tonic pricing was not found.

---

## Q6. Log viewers: lnav, klogg, Logdy, Dozzle, stern, Grafana Logs Drilldown, Kibana Discover

### Takeaway
The best-in-class log feature set combines:
- lnav: 70+ auto-detected formats, timestamp-merged multi-file view, SQL (SQLite virtual tables) and PRQL over logs, histogram plus timeline views, remote files, headless mode
- klogg: 10+ GB files, SIMD multithreaded regex, AND/OR/NOT filters, highlighter sets
- Logdy: lightweight web UI over stdin/sockets with TypeScript parsers
- Dozzle: container-native streaming with SQL over JSON fields, alerts, and MCP
- Grafana/Kibana: "queryless" exploration (pattern clustering, field statistics)

### Cited Findings
- **lnav:**
  - Merges multiple files into one timestamp-ordered view with per-file color bars. Auto-detects 70+ formats (access logs, logfmt, syslog, journald JSON, Rust tracing, MySQL/PostgreSQL). Custom formats for JSON-lines or PCRE patterns. On-the-fly gzip/bzip2.
  - Regex in/out filters, histogram view (warnings/errors, jump to time), timeline view (operations as spans grouped by opid/thread/tag/file), pretty-print for XML/JSON (Shift+P), and SQL via SQLite virtual tables.
  - Live search-as-you-type and tailing, themes, custom keymaps, sessions (bookmarks/filters restored), and headless `-n -c` with CSV output.
  — [lnav.org/features](https://lnav.org/features)
- **lnav releases:**
  - v0.14.0: remote-file helper rewritten in Python 3 with an APE-binary fallback; timeline rows for user tags/partitions; `:write-jsonlines-to`; `:external-access` (localhost HTTP port for remote control plus custom browser "Apps").
  - v0.14.1 (5 Sep, year not shown): start/stop tag spans, PRQL `stats.timeseries`, copying a JSON log's raw message.
  - v0.15.0-beta1 (28 Sep): tabular CSV/TSV support, named searches, a `metrics_log` format, metric sparklines, a `file split` command.
  — [lnav releases](https://github.com/tstack/lnav/releases)
- **klogg:** reads from disk rather than RAM, handles "10+ Gb" files and >2,147,483,647 lines; multithreaded, SIMD regex search "2–4 times" faster than glogg; PCRE with AND/OR/NOT combinations; predefined filters; search within part of a file; multiple highlighter sets; follow/tail; scratchpad for notes and basic transformations; encoding auto-detection (uchardet); GPLv3; calendar versioning; actively maintained — [GitHub variar/klogg](https://github.com/variar/klogg)
- **Logdy:** a single binary with an embedded web UI; inputs are files (`follow`), stdin, sockets, and a REST API; custom parsers/columns in TypeScript with a typed code editor; v0.17.0 (1 Jun 2025); Apache-2.0; about 2.3k stars — [GitHub logdyhq/logdy-core](https://github.com/logdyhq/logdy-core)
- **Dozzle v11.3.4:** real-time Docker log streaming with search, level/time filtering, and **SQL queries over JSON fields**; structured parsing with multi-line stack-trace grouping; per-container metrics; multi-host Docker/Swarm/Kubernetes via TLS agents; safe image updates; alerts on log patterns/metrics → Slack/Discord/ntfy/webhook; container actions and shell; **MCP server** for AI assistants; GitHub/OIDC auth; single binary, no DB, MIT — [dozzle.dev](https://dozzle.dev/). DuckDB-based SQL arrived in v10+ — [Better Stack guide](https://betterstack.com/community/guides/scaling-docker/dozzle-docker/)
- **stern:** multi-pod/multi-container Kubernetes tailing with pod regex or `deployment/name` queries; auto add/remove pods; label/field/node selectors; include/exclude/highlight regex; color by pod/container; outputs default/raw/json/extjson/ppextjson plus Go templates with JSON/level/timestamp helpers; `--since` (default 48h), `--tail`; stdin mode; about 4.9k stars — [GitHub stern/stern](https://github.com/stern/stern)
- **Grafana Logs Drilldown:** "queryless" browsing of Loki logs; automatic log **patterns** to filter noise or focus on anomalies; labels and detected fields explored by volume/distribution and added to the query without LogQL; requires Grafana 11.6+ and Loki 3.2+; renamed from Explore Logs on Feb 20, 2025 — [GitHub grafana/logs-drilldown](https://github.com/grafana/logs-drilldown); [Grafana what's new 2024](https://grafana.com/whats-new/2024-04-09-explore-logs/); [GrafanaCon 2025 session](https://grafana.com/events/grafanacon/2025/logs-drilldown-grafana-loki/)
- **Kibana Discover:** KQL, Lucene, or ES|QL (ES|QL needs no data view); filters; field statistics; **log pattern analysis**; side-by-side document comparison; quick charts from fields or ES|QL results; saved searches to dashboards; CSV export limits; runtime fields; background search; change point detection (Stack 9.5+) — [Elastic docs: Discover](https://www.elastic.co/docs/explore-analyze/discover)
- DevToys/DevUtils/IT-Tools include no log viewer. klogg's scratchpad is the only log-viewer-to-utility bridge found — [klogg](https://github.com/variar/klogg); [devtoys.app](https://devtoys.app/)

### Inferences
- **/dev/all log viewer spec:**
  - Memory-mapped huge-file support (klogg's disk-backed index plus SIMD regex; Rust's `regex`/`memchr` fit well).
  - Format auto-detection with a user-extensible format registry (regex with named groups or JSON-lines field mapping, as in lnav).
  - Multi-file merge by timestamp, level coloring, a histogram strip with brush-to-filter, and include/exclude regex filters combinable with AND/OR/NOT.
  - Highlighter sets, bookmarks and sessions persisted per project, and JSON pretty-print in a detail pane.
  - SQL over parsed logs (lnav's SQLite virtual tables; Dozzle's DuckDB). An embedded SQLite or DuckDB in Rust is plausible.
  - Tail sources: file follow, stdin/pipe, Docker (Dozzle), k8s (stern-like selectors), SSH (lnav).
- **Pattern clustering** (Grafana/Kibana "patterns") is the queryless feature desktop viewers lack. It can be done deterministically (template mining). The local LLM can then label clusters, explain anomalies, and turn NL into a filter, SQL, or regex query.
- **Synergy:**
  - The HTTP client and mock server emit structured request logs straight into the viewer, correlated by request ID.
  - Load-test runs produce logs with a timeline/histogram (lnav timeline ≈ request spans).
  - The regex sandbox defines formats and highlighters.
  - Clicking a JWT/Base64/timestamp inside a log line routes it to the matching utility (klogg scratchpad, generalized).
  - Fake data generates synthetic logs to test parsers.
  - A diff compares log windows.

### Gaps
- LogExpert and glogg were not researched in detail (glogg appears superseded by klogg's fork).
- Kibana Discover's histogram, field-sidebar, and surrounding-documents UX are not described on the fetched overview page.
- lnav release years weren't shown on the releases page (v0.14.x is likely 2026).

---

## Q7. JWT tools: jwt.io, token.dev, jwt.ms and peers; verification, JWKS, claims, expiry, and pasting-token security

### Takeaway
jwt.io (Auth0/Okta) offers decode/encode, secret-based verification, and a "Claims Breakdown", and now promotes agent skills. token.dev now serves an Okta-branded JWT Debugger with JWT and PASETO tabs and an explicit "browser only" verification notice. Newer competitors differentiate on automatic JWKS retrieval (Duende, Clerk), exp/nbf/iat highlighting, PEM/JWK/JWKS key input, and bulk verification. The persistent security concern is that pasting real tokens into third-party websites leaks credentials. That is a strong argument for a local-first desktop JWT tool.

### Cited Findings
- **jwt.io:** Decoder and Encoder tabs, "Decode, verify, and generate JSON Web Tokens", optional secret with a Base64URL-encoded toggle, "Valid secret"/"Signature Verified" badges (sample secret "a-string-secret-at-least-256-bits-long"), a Claims Breakdown for header and payload, and a library directory. It promotes JWT "agent skills" via `npx skills add jsonwebtoken/jwt-skills`. The fetched page did not mention JWKS fetch, expiry indicators, or whether processing is client-side — [jwt.io](https://jwt.io/)
- **token.dev:** now an Okta-branded "JWT Debugger" with JWT and PASETO tabs, an algorithm selector, and signing-key input. It states "Verification of the JWT is done in the browser only!" and warns not to use keys from the site in your apps — [token.dev](https://token.dev/)
- **Duende JWT Decoder (Nov 2025):** automatically retrieves public key data to validate JWK-signed tokens, explains well-known claims inline, and has a presenter mode — [Duende blog](https://duendesoftware.com/blog/20251120-inspect-and-validate-json-web-tokens-with-the-duende-jwt-decoder)
- **Clerk JWT decoder:** RS256 only; if the issuer has a JWKS endpoint, the key is populated automatically — [Clerk docs](https://clerk.com/docs/guides/development/troubleshooting/jwt-decoder)
- **inbrowser.app:** in-browser decode; verifies HS/RS/PS/ES with a secret, PEM, JWK, or JWKS; highlights exp/nbf/iat — [inbrowser.app](https://inbrowser.app/tools/jwt-decoder-verifier/)
- **Security debate:** "Stop pasting JWTs into jwt.io" argues that pasting a token sends credentials to a third party with unknown logging/retention. The risk is low for short-lived read-only tokens and high for long-lived admin tokens, and the author recommends local decoding — [DEV Community](https://dev.to/rmb_37/stop-pasting-jwts-into-jwtio-12kn)
- Developers confuse decoding with verifying (surprise that jwt.io decodes without the secret) — [auth0/node-jsonwebtoken#706](https://www.github.com/auth0/node-jsonwebtoken/issues/706) **[older issue]**. The `jose` library's `decodeJwt` explicitly does not validate signatures — [jose decode_jwt](https://deno.land/x/jose@v5.6.1/util/decode_jwt.ts)
- JWTs in URL query parameters leak via server logs, browser history, and Referer headers — [Gruve glossary](https://gruve.ai/in/?p=993886). Sensitive data such as passwords found inside JWT payloads led to account takeover — [InfoSec Write-ups](https://infosecwriteups.com/how-i-found-a-jwt-token-vulnerability-that-led-to-full-account-takeover-a2b6385b7c5c)
- JWT decoders ship in DevToys (JWT encoder/decoder), DevUtils (JWT Debugger), and IT-Tools (JWT parser). CyberChef added PEM→JWK with `kid` in v11.5.0 — [devtoys.app](https://devtoys.app/); [devutils.com](https://devutils.com/); [it-tools index](https://raw.githubusercontent.com/CorentinTh/it-tools/main/src/tools/index.ts); [CyberChef CHANGELOG](https://github.com/gchq/CyberChef/blob/master/CHANGELOG.md)

### Inferences
- **Best-in-class local JWT tool spec:**
  - Decode with an explicit "decoded ≠ verified" status.
  - Verify with secret (UTF-8/Base64URL), PEM, JWK, or JWKS URL. Auto-discover JWKS from `iss` via OIDC `.well-known/openid-configuration` (Duende/Clerk pattern), with an explicit opt-in because it makes a network call.
  - Show the `kid` match.
  - Relative-time display for exp/nbf/iat with expired/not-yet-valid warnings and clock-skew tolerance.
  - Plain-English claims glossary (registered and common OIDC claims).
  - Lint warnings: `alg: none`, weak HS secret length (jwt.io's 256-bit hint), sensitive data in payload, very long lifetime.
  - Encoder/signer with key generation (RSA/EC/Ed25519; IT-Tools and DevToys extensions already ship RSA keypair generators), JWE/PASETO later, and bulk mode.
- **Privacy positioning:** "never leaves your machine" is a concrete, defensible differentiator against jwt.io-style sites, and it is the most-cited JWT concern.
- **Synergy with the HTTP client:**
  - Auto-detect `Authorization: Bearer` tokens in requests and responses, show an expiry countdown, and warn before sending expired tokens.
  - Extract tokens from login responses into environment variables (CyberChef "Register" analog).
  - Mocks: sign test JWTs with project-local keys and serve a mock JWKS endpoint so apps under test can validate tokens offline.
  - Load tests: generate many signed tokens with varied claims.
  - Log viewer: click a JWT in a log line to decode it.

### Gaps
- jwt.ms (Microsoft) could not be fetched or researched; its feature set is unconfirmed.
- jwt.io's processing location (client vs. server) and any JWKS support were not stated on the fetched page.
- No source documented a formal 2025–2026 jwt.io redesign.

---

## Q8. Other high-value utilities often missing from toolboxes, and how utilities feed each other and the bigger tools

### Takeaway
The biggest gaps in mainstream toolboxes sit right next to API work:
- cURL ↔ code conversion (curlconverter)
- JSON → types (quicktype, transform.tools)
- OpenAPI/AsyncAPI linting (Spectral)
- gRPC reflection exploration (grpcui)
- semantic JSON diff/patch (jd)
- secret scanning (gitleaks → Betterleaks)

Each has a mature OSS reference implementation. They matter more to /dev/all than to a standalone toolbox because they plug directly into the HTTP client, mocks, and load tests.

### Cited Findings
- **curlconverter:** about 30 targets (Python default, JS/Node variants, Go, Rust, Java, C, C#, PHP, Ruby, Swift, Kotlin, R, PowerShell, Wget) plus HAR, raw HTTP, JSON, and Ansible. Recognizes all 255 curl arguments (most ignored). Parses Bash via tree-sitter-bash (ANSI-C quoting, heredocs, stdin redirects). Usable as a JS library with warnings. HTTP only; non-Python generators are less thorough. MIT, 8.2k stars — [GitHub curlconverter](https://github.com/curlconverter/curlconverter)
- **quicktype:** inputs are JSON, JSON API URLs, JSON Schema, TypeScript, and GraphQL queries. 21 targets: Ruby, JS, Flow, Rust, Kotlin, Dart, Python, C#, Go, C++, Java, Scala, TypeScript, Swift, Objective-C, Elm, JSON Schema, Pike, PropTypes, Haskell, PHP. Generates models and serializers. Apache-2.0, 13.9k stars, Node 20+ — [GitHub glideapps/quicktype](https://github.com/glideapps/quicktype)
- **Spectral:** JSON/YAML linter with built-in rulesets for OpenAPI 2.0/3.0/3.1 (feature list also mentions 3.2), AsyncAPI 2.x, and Arazzo 1.0. Custom rulesets in JSON/YAML/JS/TS with custom functions; CLI and Docker; Apache-2.0, 3.2k stars — [GitHub stoplightio/spectral](https://github.com/stoplightio/spectral)
- **grpcui:** web UI using server reflection by default, or `.proto`/protoset files. Auto-generated request forms (nested messages, oneofs, maps, well-known types) plus a raw JSON tab. Metadata and headers/trailers. Supports all RPC types, but streams are sent and shown in bulk (no interactive bidi, unlike grpcurl). 5.9k stars — [GitHub fullstorydev/grpcui](https://github.com/fullstorydev/grpcui)
- **gitleaks:** detects secrets in git history (`git log -p`), directories, and stdin. TOML rules combine regex, entropy, keywords, and allowlists. Composite rules since v8.28.0. Optional decoding of percent/hex/base64 content; archive scanning; baselines; `gitleaks:allow`; SARIF/JSON/CSV/JUnit reports. **Now "feature complete": security patches only, with focus moving to Betterleaks.** MIT, about 30.2k stars — [GitHub gitleaks](https://github.com/gitleaks/gitleaks)
- **Long-tail utilities that appear in only one toolbox** (i.e., candidates many users would miss): docker run→compose, chmod, crontab generator, IPv4 subnet/range, MAC lookup, IPv6 ULA, OTP/TOTP, basic-auth header generator, MIME types, user-agent parser, HTTP status codes, email normalizer, IBAN/phone parsing, BIP39, PDF signature checker, safelink decoder (IT-Tools); HTML→JSX, SVG→CSS, PHP serialize, cURL→Code, String Inspector (DevUtils); Semver calculator, JSON Schema, XSD generator, RESX translator, file splitter (DevToys extensions); JSON Schema→Protobuf/OpenAPI/Zod (transform.tools); network protocol parsing (TLS record, IPv4/TCP/UDP headers), PGP key parsing, PEM→JWK, Remove ANSI codes (CyberChef 2026) — [it-tools index](https://raw.githubusercontent.com/CorentinTh/it-tools/main/src/tools/index.ts); [devutils.com](https://devutils.com/); [devtoys.app](https://devtoys.app/); [transform.tools](https://transform.tools/); [CyberChef CHANGELOG](https://github.com/gchq/CyberChef/blob/master/CHANGELOG.md)
- **MCP is becoming a standard surface for dev utilities and viewers:** regex101 workspaces (MCP), Dozzle (MCP server with OAuth), Elysiatools regex explainer (MCP), and jwt.io agent skills — [regex101 docs](https://docs.regex101.com); [dozzle.dev](https://dozzle.dev/); [elysiatools](https://elysiatools.com/zh/docs/ai-regex-explainer); [jwt.io](https://jwt.io/)

### Inferences
- **Additional candidates** (not verified in this research, but recurring in dev workflows and absent from the surveyed toolbox catalogs): DNS lookup/dig, TLS/SSL certificate chain checker (fetch a live cert into the existing X.509 decoder), port check/scan of localhost, WebSocket/SSE tester, raw protobuf decoder (no schema), `.env` file diff/validator, OpenAPI diff (breaking-change detection), GraphQL schema explorer/introspection viewer, HAR viewer, cron "next N runs" simulator with timezone, Unicode/confusables inspector, JSONPath/jq/JMESPath playground (DevToys has JSONPath only), Protobuf↔JSON, MessagePack/CBOR/BSON decode, timestamp-with-timezone converter, and HTTP header/cookie/CSP parser.
- **Synergy map for /dev/all (utility → big tool):**
  - **cURL converter** ↔ HTTP client: import pasted curl, export any request as curl or code. HAR import/export via the same IR.
  - **JSON→types (quicktype-style)** ↔ HTTP client responses and mock schemas: "generate TS/Rust types from this response".
  - **Fake data generator** ↔ mock server (schema-driven responses, stable IDs) ↔ load tests (unique bodies) ↔ DB seed export.
  - **JWT tool** ↔ auth flows in the HTTP client, mock JWKS endpoints, load-test token generation.
  - **Regex sandbox** ↔ log formats/highlighters, response assertions, pipeline Subsection/Register, secret rules.
  - **Semantic JSON diff** ↔ response comparison across environments, snapshot tests, mock-vs-real drift.
  - **Spectral-style linter** ↔ OpenAPI import before generating mocks and tests.
  - **gRPC reflection explorer** ↔ HTTP client's gRPC mode.
  - **Secret scanner (gitleaks-style regex + entropy)** ↔ warn before exporting or sharing collections, environments, recipes, or logs. Also auto-mask secrets in the log viewer.
  - **Log viewer** ↔ all runtime tools (mock hits, load-test results, request history) correlated by request ID and timestamp.
  - **Encoders/hash/timestamp converters** ↔ inline "decode this value" actions anywhere a value appears (response body, header, log line), driven by the shared detector registry (DevToys smart detection + CyberChef Magic).
- **Architecture implication:** a single typed-value + detector + operation registry in Rust can serve the standalone utility UI, clipboard detection, command palette, pipelines/recipes, inline context actions in the HTTP/log views, a CLI (DevToys CLI / lnav headless parity), and an MCP surface. The local LLM sits on top as a planner/explainer: NL → recipe, regex from examples, schema from description, log-cluster labeling. It does not replace the deterministic transforms.

### Gaps
- DNS/SSL/port-scan/WebSocket/protobuf-decode/env-diff utilities: no 2025–2026 sources were retrieved (search budget exhausted), so their best-in-class feature sets are not documented here.
- Betterleaks' feature set and maturity were not researched.
- grpcui, Spectral, and quicktype release dates/versions were not visible on the fetched pages, so current maintenance cadence is unconfirmed.
