//! rustls needs exactly one process-wide crypto provider when more than one is compiled in (the
//! full app links both ring and aws-lc-rs). kube builds its TLS config from that default.

use std::sync::Once;

/// Installs aws-lc-rs as the process default unless something else already did. Idempotent.
pub fn ensure_crypto_provider() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}
