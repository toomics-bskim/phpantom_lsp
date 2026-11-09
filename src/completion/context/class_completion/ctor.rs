//! Constructor-snippet building for `new` and attribute completion.
use tower_lsp_server::gen_lsp_types::InsertTextFormat;

use crate::Backend;
use crate::completion::builder::build_callable_snippet;
use crate::types::{ParameterInfo, SharedVec};

impl Backend {
    /// Build the insert text (and optional format) for a `new` context
    /// class name completion.
    ///
    /// If constructor parameters are available, generates a callable
    /// snippet; otherwise generates `Name()$0`.
    pub(in crate::completion) fn build_new_insert(
        name: &str,
        ctor_params: Option<&[ParameterInfo]>,
    ) -> (String, Option<InsertTextFormat>) {
        if let Some(params) = ctor_params
            && !params.is_empty()
        {
            let snippet = build_callable_snippet(name, params);
            (snippet, Some(InsertTextFormat::Snippet))
        } else {
            (format!("{name}()$0"), Some(InsertTextFormat::Snippet))
        }
    }

    /// Load the effective constructor parameters for a class, if available.
    ///
    /// Tries `load_stub_class` (which checks `uri_classes_index` first, then
    /// in-memory stubs) to avoid disk I/O.  Returns `None` when the
    /// class cannot be found or has no constructor.
    pub(super) fn ctor_params_for(&self, class_name: &str) -> Option<SharedVec<ParameterInfo>> {
        let cls = self.load_stub_class(class_name)?;
        if let Some(ctor) = cls.get_constructor() {
            return Some(ctor.parameters.clone());
        }
        if cls.parent_class.is_none() && cls.used_traits.is_empty() {
            return None;
        }
        let merged = crate::virtual_members::resolve_class_fully_cached(
            &cls,
            &|name| self.find_or_load_class(&name),
            &self.resolved_class_cache,
        );
        let ctor = merged.get_constructor()?;
        Some(ctor.parameters.clone())
    }
}
