//! Sync server for trackdo.
//!
//! Validates, orders, stores and broadcasts operations defined by
//! `trackdo-protocol`. Conflict resolution stays on the clients.

pub mod auth;
pub mod http;
pub mod snapshot;
pub mod store;

#[cfg(test)]
mod tests {
    #[test]
    fn speaks_the_pinned_protocol_version() {
        assert_eq!(trackdo_protocol::PROTOCOL_VERSION, 2);
    }
}
