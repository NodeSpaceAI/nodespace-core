//! Display names for the extension ids a database can require, and the text
//! every surface shows when this build refuses such a database (ADR-083 §2,
//! ADR-084 §1).
//!
//! A database's settings node lists, in `required_extensions`, the extensions
//! a reader needs in order to read it correctly. This build supports none, so
//! the daemon refuses to open a database that lists any. The daemon, the CLI,
//! the tray and the desktop app all render that refusal from this module, so
//! every surface says the same thing.
//!
//! This is the one file in the repository that may name the paid product
//! (ADR-081 §8): it holds the display name of the `pro` extension id, used
//! only by the refusal.

/// The extension id whose display name this build knows.
const PAID_PRODUCT_ID: &str = "pro";

/// The display name of a known extension id, or `None` for any other id.
pub fn display_name(id: &str) -> Option<&'static str> {
    match id {
        PAID_PRODUCT_ID => Some("NodeSpace Pro"),
        _ => None,
    }
}

/// The label of the refusal's download link.
pub const DOWNLOAD_LABEL: &str = "Download NodeSpace Pro";

/// Where the refusal's download link points. A compile-time constant
/// (ADR-084 §1): it points at the website until a direct download link exists,
/// which a later release substitutes here.
pub const DOWNLOAD_URL: &str = "https://nodespace.ai";

/// What a refused database needs, as the phrase that completes "This database
/// …": the display name of the first id this module knows, otherwise the
/// unsupported ids themselves. Used as the marker in `nodespace database list`
/// and the tray's Databases menu.
pub fn requirement<S: AsRef<str>>(unsupported: &[S]) -> String {
    if let Some(name) = unsupported.iter().find_map(|id| display_name(id.as_ref())) {
        return format!("needs {name}");
    }
    let ids: Vec<&str> = unsupported.iter().map(AsRef::as_ref).collect();
    if ids.is_empty() {
        "needs an extension this app doesn't support".to_string()
    } else {
        format!(
            "needs an extension this app doesn't support ({})",
            ids.join(", ")
        )
    }
}

/// The refusal message for a database that requires the `unsupported`
/// extensions. The daemon sends it as the gRPC status message; the CLI and the
/// desktop app show it as the error.
pub fn refusal_message<S: AsRef<str>>(unsupported: &[S]) -> String {
    format!("This database {}", requirement(unsupported))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_paid_product_id_has_a_display_name_and_no_other_id_does() {
        assert_eq!(display_name("pro"), Some("NodeSpace Pro"));
        assert_eq!(display_name("fixture"), None);
        assert_eq!(display_name("PRO"), None, "ids are matched exactly");
        assert_eq!(display_name(""), None);
    }

    #[test]
    fn the_download_link_is_pinned() {
        assert_eq!(DOWNLOAD_LABEL, "Download NodeSpace Pro");
        assert_eq!(DOWNLOAD_URL, "https://nodespace.ai");
    }

    #[test]
    fn a_database_requiring_the_paid_product_names_it() {
        assert_eq!(
            refusal_message(&["pro"]),
            "This database needs NodeSpace Pro"
        );
        assert_eq!(requirement(&["pro"]), "needs NodeSpace Pro");
    }

    #[test]
    fn the_paid_product_is_named_even_beside_other_ids() {
        assert_eq!(
            refusal_message(&["fixture", "pro"]),
            "This database needs NodeSpace Pro"
        );
    }

    #[test]
    fn any_other_id_is_listed_as_unsupported() {
        assert_eq!(
            refusal_message(&["fixture"]),
            "This database needs an extension this app doesn't support (fixture)"
        );
        assert_eq!(
            refusal_message(&["a".to_string(), "b".to_string()]),
            "This database needs an extension this app doesn't support (a, b)"
        );
    }

    #[test]
    fn an_empty_list_still_reads_as_a_sentence() {
        let none: [&str; 0] = [];
        assert_eq!(
            refusal_message(&none),
            "This database needs an extension this app doesn't support"
        );
    }
}
