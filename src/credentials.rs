//! Request authentication secrets stored in the operating system credential vault.

use serde::{Deserialize, Serialize};

const SERVICE_NAME: &str = "io.github.yashG0.resonance-api-testing-platform";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestCredentials {
    pub auth_value: String,
    pub auth_secret: String,
}

#[derive(Debug, thiserror::Error)]
pub enum CredentialVaultError {
    #[error("the operating system credential store could not be accessed: {0}")]
    Backend(String),
    #[error("a saved request credential is missing from the operating system store")]
    Missing,
    #[error("a saved credential could not be decoded")]
    InvalidData,
}

pub trait CredentialVault: Send + Sync {
    fn store(
        &self,
        reference: &str,
        credentials: &RequestCredentials,
    ) -> Result<(), CredentialVaultError>;

    fn load(&self, reference: &str) -> Result<Option<RequestCredentials>, CredentialVaultError>;

    fn delete(&self, reference: &str) -> Result<(), CredentialVaultError>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemCredentialVault;

impl CredentialVault for SystemCredentialVault {
    fn store(
        &self,
        reference: &str,
        credentials: &RequestCredentials,
    ) -> Result<(), CredentialVaultError> {
        let entry = entry(reference)?;
        let encoded =
            serde_json::to_string(credentials).map_err(|_| CredentialVaultError::InvalidData)?;
        entry
            .set_password(&encoded)
            .map_err(|error| CredentialVaultError::Backend(error.to_string()))
    }

    fn load(&self, reference: &str) -> Result<Option<RequestCredentials>, CredentialVaultError> {
        let entry = entry(reference)?;
        match entry.get_password() {
            Ok(encoded) => serde_json::from_str(&encoded)
                .map(Some)
                .map_err(|_| CredentialVaultError::InvalidData),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(CredentialVaultError::Backend(error.to_string())),
        }
    }

    fn delete(&self, reference: &str) -> Result<(), CredentialVaultError> {
        let entry = entry(reference)?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(CredentialVaultError::Backend(error.to_string())),
        }
    }
}

fn entry(reference: &str) -> Result<keyring::Entry, CredentialVaultError> {
    keyring::Entry::new(SERVICE_NAME, reference)
        .map_err(|error| CredentialVaultError::Backend(error.to_string()))
}
