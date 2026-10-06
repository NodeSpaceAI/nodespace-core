//! SQLite JSON paths that name a type's property bucket.
//!
//! A node keeps a type's fields under a key named for the type
//! (`properties["customer-profile"]["name"]`). A type id is kebab-case, so its
//! key is not a bare path label: it is quoted in the path
//! (`$."customer-profile".name`).

/// The JSON path of `field` in the property bucket `bucket`.
///
/// A bucket that is a plain identifier stays unquoted, so the path text is the
/// one the expression indexes on `task` and `project` fields are built on (an
/// index is used only for an expression written the same way). Any other
/// bucket is quoted.
///
/// `bucket` and `field` are interpolated into SQL text by the caller, so both
/// must already be known-safe identifiers; the quotes here only keep a hyphen
/// from reading as part of a path.
pub(crate) fn bucket_field_path(bucket: &str, field: &str) -> String {
    if bucket.chars().all(|c| c.is_alphanumeric() || c == '_') {
        format!("$.{bucket}.{field}")
    } else {
        format!("$.\"{bucket}\".{field}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_bucket_is_left_bare_so_expression_indexes_match() {
        assert_eq!(bucket_field_path("task", "status"), "$.task.status");
    }

    #[test]
    fn a_kebab_case_bucket_is_quoted() {
        assert_eq!(
            bucket_field_path("customer-profile", "name"),
            "$.\"customer-profile\".name"
        );
    }
}
