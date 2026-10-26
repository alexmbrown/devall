//! `devall:` links from outside the app — a browser, a chat or an email, Markdown in another
//! editor, Win+R / `open devall:…`. The OS starts the app with the URL, or a second launch hands
//! it to the running window (`tauri-plugin-single-instance`); `tauri-plugin-deep-link` reports it.
//! Each URL is checked here (`devall_core::links::parse_incoming`) and queued until the UI takes
//! it (`deep_link_take`, on start-up and on every `deep-link` event), so a cold start's link isn't
//! lost before React mounts. The UI (`src/lib/links/incoming.ts`) checks it again, resolves the
//! project and only ever navigates.
//!
//! Registering the `devall` scheme with the OS:
//! - **Windows**: installers get it from `plugins.deep-link.desktop.schemes` (tauri.conf.json); the
//!   app also writes `HKCU\Software\Classes\devall` on every start (`register_all`), so the local
//!   install — an exe copied without an installer — always points the scheme at the newest exe.
//! - **macOS**: the bundle's `CFBundleURLTypes` (from the same config). LaunchServices routes links
//!   to a registered `.app` (`scripts/install-local.sh` registers the installed one); there is no
//!   runtime registration, so `tauri dev` (no bundle) never receives links.
//! - **Linux** (best effort): the bundles' `.desktop` file.
//!
//! Dev builds (`debug_assertions`) stay out of the way of an installed copy, which shares the app
//! identifier: no single-instance lock (`tauri dev` would exit and focus the installed app) and no
//! scheme registration (the dev exe would take the scheme over) — unless `DEVALL_DEEP_LINK_DEV=1`.
//! A dev exe started with a single `devall:` argument also uses the single-instance lock, so a link
//! clicked while an opted-in dev build owns the scheme reaches the running dev app.
//!
//! Registration: `pub mod deep_link;`, `deep_link::plugins(builder)` before any other plugin,
//! `deep_link::setup(app)` in setup (manages `DeepLinkState`), command `deep_link_take`. Emits
//! `deep-link` (no payload) when links are waiting.

use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

/// Tells the UI that links are waiting in `deep_link_take`.
const EVENT: &str = "deep-link";
/// Links kept while the UI doesn't take them (a flood of launches can't grow the queue).
const MAX_PENDING: usize = 20;
/// Characters of a refused link the UI shows.
const MAX_SHOWN: usize = 160;

/// Release builds always; dev builds only with `DEVALL_DEEP_LINK_DEV=1`.
fn opted_in() -> bool {
    !cfg!(debug_assertions) || std::env::var("DEVALL_DEEP_LINK_DEV").is_ok_and(|v| v == "1")
}

/// Started with a single `devall:` argument. For a dev build that only happens once an opted-in
/// dev build registered the scheme (or by hand, for testing): it then hands the link to the running
/// dev app instead of starting a second one, even without the variable (a browser doesn't pass it).
fn started_for_link() -> bool {
    let mut args = std::env::args().skip(1);
    matches!((args.next(), args.next()), (Some(a), None) if devall_core::links::has_scheme(&a))
}

/// Whether this process takes links from the OS (single instance + deep-link plugin).
fn enabled() -> bool {
    opted_in() || started_for_link()
}

/// A link from outside the app, checked.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Incoming {
    /// The link to follow (canonical, with its `?project=` qualifier) when it passed the checks.
    #[serde(skip_serializing_if = "Option::is_none")]
    link: Option<String>,
    /// Why it was refused: the UI shows it and does nothing else.
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    /// What arrived, shortened, for messages.
    raw: String,
}

impl Incoming {
    fn check(raw: &str) -> Self {
        let shown = shorten(raw);
        match devall_core::links::parse_incoming(raw) {
            Ok(link) => Self { link: Some(link.to_qualified_link()), error: None, raw: shown },
            Err(e) => Self { link: None, error: Some(e), raw: shown },
        }
    }
}

/// At most `MAX_SHOWN` characters, without control characters.
fn shorten(s: &str) -> String {
    let mut chars = s.trim().chars().filter(|c| !c.is_control());
    let mut out: String = chars.by_ref().take(MAX_SHOWN).collect();
    if chars.next().is_some() {
        out.push('…');
    }
    out
}

#[derive(Default)]
pub struct DeepLinkState {
    pending: Mutex<Vec<Incoming>>,
}

impl DeepLinkState {
    fn push(&self, urls: impl IntoIterator<Item = String>) {
        let mut pending = self.pending.lock().unwrap();
        for url in urls {
            if pending.len() < MAX_PENDING {
                pending.push(Incoming::check(&url));
            }
        }
    }
}

/// Adds the single-instance plugin (first, so a second launch hands over its arguments and exits
/// before anything else starts) and the deep-link plugin, when this build handles OS links.
pub fn plugins<R: Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    if !enabled() {
        return builder;
    }
    // With its `deep-link` feature the plugin passes a second launch's URL to the deep-link plugin
    // (→ `on_open_url` below) before this callback runs.
    builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| focus_main(app))).plugin(tauri_plugin_deep_link::init())
}

/// Queues the link the app was started with and every later one; on Windows, points the scheme
/// at this exe.
pub fn setup<R: Runtime>(app: &tauri::App<R>) {
    app.manage(DeepLinkState::default());
    if !enabled() {
        return;
    }
    use tauri_plugin_deep_link::DeepLinkExt;
    let deep_link = app.deep_link();
    #[cfg(windows)]
    if opted_in()
        && let Err(e) = deep_link.register_all()
    {
        eprintln!("could not register the devall: link scheme: {e}");
    }
    // Both run on the main thread, which also delivers forwarded launches (Windows messages,
    // macOS open-URL events), so nothing arrives between them.
    if let Ok(Some(urls)) = deep_link.get_current() {
        receive(app.handle(), urls.into_iter().map(String::from));
    }
    let handle = app.handle().clone();
    deep_link.on_open_url(move |event| {
        receive(&handle, event.urls().into_iter().map(String::from));
        focus_main(&handle);
    });
}

fn receive<R: Runtime>(app: &AppHandle<R>, urls: impl IntoIterator<Item = String>) {
    app.state::<DeepLinkState>().push(urls);
    let _ = app.emit(EVENT, ());
}

/// Brings the main window to the front (a second launch, a link while running).
fn focus_main<R: Runtime>(app: &AppHandle<R>) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// The links that arrived from outside the app since the last call, checked.
#[tauri::command]
pub fn deep_link_take(state: State<'_, DeepLinkState>) -> Vec<Incoming> {
    std::mem::take(&mut *state.pending.lock().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_checked_and_queued_canonical() {
        let state = DeepLinkState::default();
        state.push(["DEVALL:Note/My%20Note.md?x=1&project=01JC8Z#top".to_string(), "devall:note/../secret".to_string()]);
        let got = std::mem::take(&mut *state.pending.lock().unwrap());
        assert_eq!(got[0], Incoming { link: Some("devall:note/My%20Note.md?project=01JC8Z".into()), error: None, raw: "DEVALL:Note/My%20Note.md?x=1&project=01JC8Z#top".into() });
        assert!(got[1].link.is_none() && got[1].error.is_some());
        state.push((0..30).map(|i| format!("devall:todo/{i}")));
        assert_eq!(state.pending.lock().unwrap().len(), MAX_PENDING);
    }

    #[test]
    fn refused_links_are_shown_short_and_clean() {
        assert_eq!(shorten(" devall:a\nb "), "devall:ab");
        let long = shorten(&"x".repeat(500));
        assert_eq!(long.chars().count(), MAX_SHOWN + 1);
        assert!(long.ends_with('…'));
    }
}
