//! What a build composing the daemon adds to it (ADR-082 §5).
//!
//! [`DaemonExtensions`] is the value [`super::build_shared_services`] takes.
//! [`DaemonExtensions::none`] adds nothing, and the daemon then behaves exactly
//! as core's own does. The hooks are part of the extension API, versioned
//! with `EXTENSION_API_VERSION` (ADR-082 §8).

use nodespace_core::extensions::is_extension_id;

/// What a build composing the daemon adds to it. Built once and handed to
/// [`super::build_shared_services`], which checks it before it builds
/// anything; every database the daemon then opens gets the same set.
///
/// ```
/// use nodespace_daemon::DaemonExtensions;
///
/// let extensions = DaemonExtensions::none().supported_extension("fixture");
/// assert_eq!(extensions.supported_extensions(), ["fixture"]);
/// assert!(DaemonExtensions::none().supported_extensions().is_empty());
/// ```
#[derive(Clone, Debug, Default)]
pub struct DaemonExtensions {
    supported_extensions: Vec<String>,
}

impl DaemonExtensions {
    /// Nothing added: core's own daemon.
    pub fn none() -> Self {
        Self::default()
    }

    /// Declares `id` an extension this daemon supports, so a database whose
    /// settings node lists it in `required_extensions` opens (ADR-083 §2).
    /// Core supports none: without this a database that lists any id is
    /// refused.
    ///
    /// The id must have the form [`is_extension_id`] describes; any other id
    /// is rejected when the daemon starts (see [`DaemonExtensionsError`]).
    /// Declaring an id twice is the same as declaring it once.
    pub fn supported_extension(mut self, id: impl Into<String>) -> Self {
        let id = id.into();
        if !self.supported_extensions.contains(&id) {
            self.supported_extensions.push(id);
        }
        self
    }

    /// The extension ids this daemon supports, in the order they were
    /// declared. A database that lists any other id is refused when opened.
    pub fn supported_extensions(&self) -> &[String] {
        &self.supported_extensions
    }

    /// Checks everything this value adds, as the daemon does at startup.
    ///
    /// # Errors
    ///
    /// The first problem found; see [`DaemonExtensionsError`].
    pub fn check(&self) -> Result<(), DaemonExtensionsError> {
        if let Some(id) = self
            .supported_extensions
            .iter()
            .find(|id| !is_extension_id(id))
        {
            return Err(DaemonExtensionsError::InvalidExtensionId(id.clone()));
        }
        Ok(())
    }
}

/// Why the daemon refused a [`DaemonExtensions`] at startup.
///
/// `non_exhaustive`: a new hook adds the ways it can be refused, which is a
/// minor change to the extension API.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DaemonExtensionsError {
    /// A declared extension id does not have the form
    /// [`DaemonExtensions::supported_extension`] requires.
    InvalidExtensionId(String),
}

impl std::fmt::Display for DaemonExtensionsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidExtensionId(id) => write!(
                f,
                "'{id}' is not a valid extension id: use a lowercase letter followed by \
                 lowercase letters, digits, '-' or '_'"
            ),
        }
    }
}

impl std::error::Error for DaemonExtensionsError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declaring_an_id_twice_declares_it_once() {
        assert_eq!(
            DaemonExtensions::none()
                .supported_extension("fixture")
                .supported_extension("other")
                .supported_extension("fixture")
                .supported_extensions(),
            ["fixture", "other"]
        );
    }

    #[test]
    fn check_rejects_the_first_invalid_supported_id() {
        assert_eq!(DaemonExtensions::none().check(), Ok(()));
        assert_eq!(
            DaemonExtensions::none()
                .supported_extension("fixture")
                .supported_extension("fixture")
                .check(),
            Ok(())
        );
        assert_eq!(
            DaemonExtensions::none()
                .supported_extension("fixture")
                .supported_extension("Bad")
                .supported_extension("")
                .check(),
            Err(DaemonExtensionsError::InvalidExtensionId("Bad".to_string()))
        );
    }
}
