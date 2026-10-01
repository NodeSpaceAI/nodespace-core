//! The wire form of the required-extensions refusal (ADR-083 §2): how the
//! daemon reports that a database lists an extension it does not support, and
//! how a client recognises that report.
//!
//! The status is `FAILED_PRECONDITION`, and its message is
//! [`crate::extension_names::refusal_message`], so a raw gRPC client or an
//! agent tool reads the same sentence every other surface shows. The binary
//! metadata key [`METADATA_KEY`] carries
//! `{"reason":"requires_extension","unsupported_extensions":[...]}`, which lets
//! a client tell this refusal apart from every other `FAILED_PRECONDITION` the
//! daemon returns without parsing the message. A `-bin` key because the ids are
//! read from the database file and may hold any bytes.

use tonic::metadata::{Binary, MetadataValue};
use tonic::{Code, Status};

/// The binary metadata key that marks a required-extensions refusal.
pub const METADATA_KEY: &str = "x-requires-extension-bin";

/// The `reason` the refusal's metadata carries.
pub const REASON: &str = "requires_extension";

/// The status the daemon returns for a request routed to a database that
/// requires the `unsupported` extensions.
pub fn status(unsupported: &[String]) -> Status {
    let mut status =
        Status::failed_precondition(crate::extension_names::refusal_message(unsupported));
    let payload = serde_json::json!({
        "reason": REASON,
        "unsupported_extensions": unsupported,
    });
    // A JSON object of strings always serializes; the `if let` only keeps a
    // panic out of the request path.
    if let Ok(json) = serde_json::to_vec(&payload) {
        status
            .metadata_mut()
            .insert_bin(METADATA_KEY, MetadataValue::<Binary>::from_bytes(&json));
    }
    status
}

/// The unsupported extension ids of a required-extensions refusal, or `None`
/// for any other status — including another `FAILED_PRECONDITION`, which the
/// daemon also returns for unrelated refusals.
pub fn unsupported_extensions(status: &Status) -> Option<Vec<String>> {
    if status.code() != Code::FailedPrecondition {
        return None;
    }
    let bytes = status.metadata().get_bin(METADATA_KEY)?.to_bytes().ok()?;
    let payload: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    if payload.get("reason")?.as_str()? != REASON {
        return None;
    }
    payload
        .get("unsupported_extensions")?
        .as_array()?
        .iter()
        .map(|id| id.as_str().map(str::to_owned))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| v.to_string()).collect()
    }

    #[test]
    fn the_status_round_trips_its_ids_and_carries_the_refusal_message() {
        let status = status(&ids(&["pro", "fixture"]));

        assert_eq!(status.code(), Code::FailedPrecondition);
        assert_eq!(
            status.message(),
            crate::extension_names::refusal_message(&["pro", "fixture"])
        );
        assert_eq!(
            unsupported_extensions(&status),
            Some(ids(&["pro", "fixture"]))
        );
    }

    #[test]
    fn the_metadata_payload_names_the_reason() {
        let status = status(&ids(&["fixture"]));
        let bytes = status
            .metadata()
            .get_bin(METADATA_KEY)
            .expect("refusal metadata present")
            .to_bytes()
            .unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            payload,
            serde_json::json!({
                "reason": "requires_extension",
                "unsupported_extensions": ["fixture"],
            })
        );
    }

    #[test]
    fn another_failed_precondition_is_not_this_refusal() {
        assert_eq!(
            unsupported_extensions(&Status::failed_precondition("some other refusal")),
            None
        );
    }

    #[test]
    fn the_metadata_on_another_code_is_ignored() {
        let mut other = Status::internal("boom");
        *other.metadata_mut() = status(&ids(&["fixture"])).metadata().clone();
        assert_eq!(unsupported_extensions(&other), None);
    }

    #[test]
    fn a_payload_with_another_reason_is_ignored() {
        let mut other = Status::failed_precondition("x");
        let json = serde_json::to_vec(&serde_json::json!({
            "reason": "something_else",
            "unsupported_extensions": ["fixture"],
        }))
        .unwrap();
        other
            .metadata_mut()
            .insert_bin(METADATA_KEY, MetadataValue::<Binary>::from_bytes(&json));
        assert_eq!(unsupported_extensions(&other), None);
    }
}
