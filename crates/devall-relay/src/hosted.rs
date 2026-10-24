//! Settings of the hosted relay run by the /dev/all project.
//!
//! TODO(owner): replace every value marked TODO after deploying `relay/` (see `relay/README.md`,
//! "Deploy"). Until then [`is_configured`] is false and the app offers self-hosting only.
//! None of these values are secrets: a Firebase Web API key only identifies the project (access
//! is governed by Firebase Auth + Firestore rules), so it may ship in the binary.

/// Relay edge (Cloudflare Worker). TODO(owner): your `relay.<domain>`.
pub const HOSTED_ENDPOINT: &str = "wss://relay.devall.dev";

/// Firebase Web API key (Project settings → General → Web API key). TODO(owner).
pub const FIREBASE_API_KEY: &str = "TODO-firebase-web-api-key";

/// Firebase project id. TODO(owner).
pub const FIREBASE_PROJECT_ID: &str = "TODO-firebase-project-id";

/// Hosted sign-in page (`relay/firebase/hosting/signin.html`). TODO(owner): your Hosting site.
pub const SIGNIN_URL: &str = "https://devall-relay.web.app/signin.html";

/// Supporter upgrade / billing page (`relay/firebase/hosting/upgrade.html`). TODO(owner).
pub const UPGRADE_URL: &str = "https://devall-relay.web.app/upgrade.html";

/// Cloud Functions base for the callables (`addCustomDomain`, …):
/// `https://<region>-<project id>.cloudfunctions.net`. TODO(owner).
pub const FUNCTIONS_URL: &str = "https://us-central1-TODO-firebase-project-id.cloudfunctions.net";

/// Google's token endpoint that exchanges a Firebase refresh token for an ID token.
pub const SECURETOKEN_URL: &str = "https://securetoken.googleapis.com/v1/token";

/// Whether the constants above were filled in (the hosted relay can be used).
pub fn is_configured() -> bool {
    !FIREBASE_API_KEY.starts_with("TODO") && !FIREBASE_PROJECT_ID.starts_with("TODO")
}
