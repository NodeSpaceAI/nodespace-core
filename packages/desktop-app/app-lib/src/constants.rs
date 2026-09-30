//! Shared constants used across the application.
//!
//! The daemon socket path is NOT a constant — it varies by build variant
//! (debug vs release, community vs Pro). Use daemon_setup::daemon_socket_relative()
//! for the active socket path, or grpc_client::resolve_socket_path() which also
//! honors the NODESPACED_SOCKET env override.
