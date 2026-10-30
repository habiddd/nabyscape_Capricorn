//! One shared blocking HTTP agent. Requests run on short-lived worker threads,
//! so the process never carries an async runtime.

use std::sync::OnceLock;
use std::time::Duration;

pub const USER_AGENT: &str = concat!(
    "NabyScape/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/BillyNabil/nabyscape)"
);

/// The process-wide agent: connection reuse, 10 s connect / 30 s read timeouts.
pub fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .user_agent(USER_AGENT)
            .timeout_connect(Duration::from_secs(10))
            .timeout_read(Duration::from_secs(30))
            .max_idle_connections(4)
            .build()
    })
}
