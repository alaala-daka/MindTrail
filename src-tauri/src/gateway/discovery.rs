use serde::Serialize;
use std::{fmt, io};

/// Discovery metadata; construction does not generate or publish a token.
#[derive(Serialize)]
pub(super) struct Endpoint {
    port: u16,
    token: String,
    version: u8,
    store_id: String,
}
impl Endpoint {
    pub(super) fn new(port: u16, token: String, store_id: String) -> io::Result<Self> {
        if port == 0 { return Err(io::Error::new(io::ErrorKind::InvalidInput, "endpoint port must be nonzero")); }
        if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) { return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid endpoint token")); }
        if store_id.len() != 64 || !store_id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) { return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid store identity")); }
        Ok(Self { port, token, version: 1, store_id })
    }
}
impl fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Endpoint").field("port", &self.port).field("token", &"[REDACTED]").field("version", &self.version).field("store_id", &self.store_id).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn endpoint_json_and_redacted_debug() {
        let id = "a".repeat(64);
        let token = "b".repeat(64);
        let endpoint = Endpoint::new(1234, token.clone(), id.clone()).unwrap();
        assert_eq!(serde_json::to_string(&endpoint).unwrap(), format!("{{\"port\":1234,\"token\":\"{token}\",\"version\":1,\"store_id\":\"{id}\"}}"));
        let debug = format!("{endpoint:?}"); assert!(!debug.contains(&token)); assert!(debug.contains("[REDACTED]"));
        assert!(Endpoint::new(0, token.clone(), id.clone()).is_err());
        for bad in ["".to_owned(), "a".repeat(63), "a".repeat(65), "A".repeat(64), format!("{}g", "a".repeat(63)), format!("{} ", "a".repeat(63)), format!("{}\"", "a".repeat(63)), format!("{}\\", "a".repeat(63))] { let error = Endpoint::new(1, bad.clone(), id.clone()).err().unwrap(); assert_eq!(error.to_string(), "invalid endpoint token"); assert!(!error.to_string().contains(&bad) || bad.is_empty()); }
        assert!(Endpoint::new(1, token, "invalid".into()).is_err());
    }
}

