//! Source grammar is scoped to a synchronous parse/request, independently of
//! the target used to select stub declarations. Stub syntax stays modern.

use mago_php_version::PHPVersion;
use mago_syntax::settings::ParserSettings;
use std::cell::Cell;

thread_local! {
    static SOURCE_VERSION: Cell<PHPVersion> = Cell::new(PHPVersion::default());
}

/// Grammar for the active synchronous pass, modern outside a scope.
pub(crate) fn source_version() -> PHPVersion {
    SOURCE_VERSION.get()
}

/// Share the active grammar between the initial parse and recovery attempts.
pub(crate) fn source_settings() -> ParserSettings {
    ParserSettings {
        php_version: source_version(),
        ..ParserSettings::default()
    }
}

/// Restores the previous grammar even when parsing unwinds.
pub(crate) struct SourceVersionGuard(PHPVersion, std::marker::PhantomData<std::rc::Rc<()>>);

impl SourceVersionGuard {
    /// Select grammar until this guard drops, nesting source and stub passes.
    pub(crate) fn new(version: PHPVersion) -> Self {
        Self(SOURCE_VERSION.replace(version), std::marker::PhantomData)
    }
}

impl Drop for SourceVersionGuard {
    fn drop(&mut self) {
        SOURCE_VERSION.set(self.0);
    }
}

impl crate::Backend {
    /// Stub descriptions use modern syntax regardless of their API target.
    pub(crate) fn source_php_version(&self, uri: &str) -> PHPVersion {
        if uri.starts_with("phpantom-stub") || self.is_external_stub_uri(uri) {
            PHPVersion::default()
        } else {
            let version = self.php_version();
            PHPVersion::new(version.major as u32, version.minor as u32, 0)
        }
    }
}
