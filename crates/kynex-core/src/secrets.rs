//! Secrets in the OS keychain: Windows Credential Manager, the macOS Keychain,
//! or the Secret Service on Linux (GNOME Keyring, KWallet). Nothing secret
//! goes into `state.json`, logs or a command line; where there's no keychain,
//! Kynex says so rather than keeping a secret in plain text.
//!
//! Every call may block (a keychain prompt, D-Bus on Linux); run them off the
//! UI thread.

use std::sync::{Arc, Mutex};

use keyring_core::{CredentialStore, Error};

/// The secrets Kynex keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Secret {
    /// The OpenRouter API key (`sk-or-…`).
    OpenRouterKey,
}

impl Secret {
    /// Its name in the keychain, under Kynex's service.
    fn name(self) -> &'static str {
        match self {
            Secret::OpenRouterKey => "openrouter-api-key",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Secret::OpenRouterKey => "the OpenRouter key",
        }
    }
}

/// A keychain: the OS's ([`Keychain::os`]), or any store, for tests.
#[derive(Clone)]
pub struct Keychain {
    store: Arc<CredentialStore>,
    service: String,
}

impl Keychain {
    /// The OS keychain, or why there is none, in words for the user.
    pub fn os() -> Result<Keychain, String> {
        // Kept once found; looked for again after a failure, in case the
        // Secret Service started since.
        static FOUND: Mutex<Option<Arc<CredentialStore>>> = Mutex::new(None);
        let mut found = FOUND.lock().unwrap_or_else(|e| e.into_inner());
        let store = match &*found {
            Some(store) => store.clone(),
            None => {
                let store = platform_store().map_err(|e| unavailable(&e))?;
                *found = Some(store.clone());
                store
            }
        };
        Ok(Keychain::with_store(store, service()))
    }

    /// A keychain over `store`, keeping Kynex's secrets under `service`.
    pub fn with_store(store: Arc<CredentialStore>, service: impl Into<String>) -> Keychain {
        Keychain { store, service: service.into() }
    }

    /// The secret, or `None` if it isn't saved.
    pub fn get(&self, secret: Secret) -> Result<Option<String>, String> {
        match self.entry(secret)?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(Error::NoEntry) => Ok(None),
            Err(e) => Err(failed("read", secret, &e)),
        }
    }

    /// Saves the secret, replacing any saved before. Surrounding whitespace
    /// (from a paste) is dropped; an empty one is refused.
    pub fn set(&self, secret: Secret, value: &str) -> Result<(), String> {
        let value = value.trim();
        if value.is_empty() {
            return Err(format!("{} is empty.", capitalized(secret.label())));
        }
        self.entry(secret)?.set_password(value).map_err(|e| failed("save", secret, &e))
    }

    /// Removes the secret; fine if it wasn't saved.
    pub fn remove(&self, secret: Secret) -> Result<(), String> {
        match self.entry(secret)?.delete_credential() {
            Ok(()) | Err(Error::NoEntry) => Ok(()),
            Err(e) => Err(failed("remove", secret, &e)),
        }
    }

    fn entry(&self, secret: Secret) -> Result<keyring_core::Entry, String> {
        self.store.build(&self.service, secret.name(), None).map_err(|e| failed("open", secret, &e))
    }
}

/// The service Kynex's secrets are kept under: `kynex`, or one per data dir
/// for a test instance (`KYNEX_DATA_DIR`), so it never sees the real ones.
fn service() -> String {
    service_for(std::env::var_os("KYNEX_DATA_DIR").map(|_| crate::paths::data_dir()).as_deref())
}

fn service_for(test_data_dir: Option<&std::path::Path>) -> String {
    match test_data_dir {
        Some(dir) => format!("kynex ({})", dir.display()),
        None => "kynex".into(),
    }
}

fn platform_store() -> Result<Arc<CredentialStore>, Error> {
    #[cfg(windows)]
    {
        Ok(windows_native_keyring_store::Store::new()?)
    }
    #[cfg(target_os = "macos")]
    {
        Ok(apple_native_keyring_store::keychain::Store::new()?)
    }
    #[cfg(target_os = "linux")]
    {
        Ok(zbus_secret_service_keyring_store::Store::new()?)
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        Err(Error::NotSupportedByStore("this OS".into()))
    }
}

/// Why there's no keychain, and that nothing is kept in plain text instead.
fn unavailable(e: &Error) -> String {
    let hint = if cfg!(target_os = "linux") {
        " Kynex uses the Secret Service: start GNOME Keyring or KWallet (or another Secret Service), then try again."
    } else {
        ""
    };
    format!("No keychain is available ({e}), so Kynex can't save secrets; it won't keep them in plain text.{hint}")
}

fn failed(what: &str, secret: Secret, e: &Error) -> String {
    format!("Couldn't {what} {} in the keychain: {e}", secret.label())
}

fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock() -> Keychain {
        Keychain::with_store(keyring_core::mock::Store::new().unwrap(), "kynex-test")
    }

    #[test]
    fn saves_reads_replaces_and_removes() {
        let k = mock();
        assert_eq!(k.get(Secret::OpenRouterKey).unwrap(), None);
        k.set(Secret::OpenRouterKey, "  sk-or-one\n").unwrap();
        assert_eq!(k.get(Secret::OpenRouterKey).unwrap().as_deref(), Some("sk-or-one"));
        k.set(Secret::OpenRouterKey, "sk-or-two").unwrap();
        assert_eq!(k.get(Secret::OpenRouterKey).unwrap().as_deref(), Some("sk-or-two"));
        k.remove(Secret::OpenRouterKey).unwrap();
        assert_eq!(k.get(Secret::OpenRouterKey).unwrap(), None);
        // Removing what isn't there is fine.
        k.remove(Secret::OpenRouterKey).unwrap();
    }

    #[test]
    fn refuses_an_empty_secret() {
        let err = mock().set(Secret::OpenRouterKey, "   ").unwrap_err();
        assert_eq!(err, "The OpenRouter key is empty.");
    }

    #[test]
    fn no_keychain_says_so_and_refuses_plain_text() {
        let msg = unavailable(&Error::NoStorageAccess("no D-Bus session".into()));
        assert!(msg.contains("No keychain is available") && msg.contains("plain text"), "{msg}");
        assert_eq!(msg.contains("Secret Service"), cfg!(target_os = "linux"));
    }

    #[test]
    fn errors_never_hold_the_secret() {
        let msg = failed("read", Secret::OpenRouterKey, &Error::BadEncoding(b"sk-or-secret".to_vec()));
        assert!(!msg.contains("sk-or-secret"), "{msg}");
    }

    #[test]
    fn test_instances_keep_their_own_secrets() {
        assert_eq!(service_for(None), "kynex");
        let dir = std::path::Path::new("scratch");
        assert_eq!(service_for(Some(dir)), "kynex (scratch)");
    }

    /// Round trip through the real keychain. Run with `cargo test -- --ignored`.
    #[test]
    #[ignore = "writes to and removes from your OS keychain"]
    fn os_keychain_round_trip() {
        let k = Keychain::with_store(platform_store().unwrap(), "kynex-test");
        k.set(Secret::OpenRouterKey, "sk-or-kynex-test").unwrap();
        assert_eq!(k.get(Secret::OpenRouterKey).unwrap().as_deref(), Some("sk-or-kynex-test"));
        k.remove(Secret::OpenRouterKey).unwrap();
        assert_eq!(k.get(Secret::OpenRouterKey).unwrap(), None);
    }
}
