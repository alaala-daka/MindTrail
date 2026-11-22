use std::{future::Future, pin::Pin};
use super::types::{SemanticError, SemanticInput};

pub trait SemanticProvider: Send + Sync {
    fn generate<'a>(&'a self, input: &'a SemanticInput) -> Pin<Box<dyn Future<Output = Result<String, SemanticError>> + Send + 'a>>;
}
