pub mod conflicts;
pub mod database;
pub mod diagnostics;
pub mod import;
pub mod logs;
pub mod mcp;
pub mod mention;
pub mod model;
pub mod node;
pub mod playbook;
pub mod query;
pub mod relationship;
pub mod schema;
pub mod search;
pub mod seed;
pub mod session;
pub mod skill;
pub mod uninstall;
pub mod wait;

/// Fail with `message` when a lookup matched nothing. A reset that found no
/// target must not exit 0: a caller reading the exit code would believe the
/// reset happened. Nothing is written to stdout, so `--json` callers see an
/// empty stream and a non-zero status.
pub(crate) fn require_found(found: bool, message: String) -> anyhow::Result<()> {
    if found {
        Ok(())
    } else {
        anyhow::bail!(message)
    }
}

#[cfg(test)]
mod tests {
    use super::require_found;

    #[test]
    fn a_missing_target_is_an_error_with_the_message() {
        let err = require_found(false, "No seeded skill found with key \"x\".".into())
            .expect_err("a missing target must fail");
        assert_eq!(err.to_string(), "No seeded skill found with key \"x\".");
        assert!(require_found(true, "unused".into()).is_ok());
    }
}
