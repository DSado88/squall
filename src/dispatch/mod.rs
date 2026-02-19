pub mod cli;
pub mod http;
pub mod registry;

use std::time::Instant;

/// Internal request type — both HTTP and CLI backends accept this.
pub struct ProviderRequest {
    pub prompt: String,
    pub model: String,
    pub deadline: Instant,
}

/// Internal result type — both backends return this.
pub struct ProviderResult {
    pub text: String,
    pub model: String,
    pub provider: String,
    pub latency_ms: u64,
}
