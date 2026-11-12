//! Candidate entries built from one immutable parse, before publication.
use super::*;
use crate::names::OwnedResolvedNames;
use crate::types::ClassInfo;

pub(crate) type ReferenceEntries = Vec<(ReferenceIndexKey, bool)>;

/// Both background parses and already-published maps use this same name context.
pub(crate) struct ReferenceFile<'a> {
    pub(crate) classes: &'a [Arc<ClassInfo>],
    pub(crate) use_map: &'a HashMap<String, String>,
    pub(crate) namespaces: &'a [NamespaceSpan],
    pub(crate) resolved_names: Option<&'a OwnedResolvedNames>,
}

impl ReferenceFile<'_> {
    /// Build the `(key, counts_as_reference)` pairs a file contributes to
    /// the index. The caller interns the URI once per file and shares it
    /// across every key, rather than allocating a fresh `String` per span.
    ///
    /// A span joins the index under every name it could be searched by,
    /// but only one of those names is the symbol it actually names: a
    /// reference to `App\Widget` is a candidate under `Widget` too, and
    /// counting it there would credit a global `\Widget` with it.  So the
    /// count rides on the resolved name alone, and a declaration counts
    /// under no name at all.
    pub(crate) fn build(
        &self,
        backend: &Backend,
        uri: &str,
        symbol_map: &SymbolMap,
    ) -> Vec<(ReferenceIndexKey, bool)> {
        if !backend.is_reference_indexable_uri(uri) {
            return Vec::new();
        }

        let mut entries: Vec<(ReferenceIndexKey, bool)> = Vec::new();
        let is_resource = backend.is_resource_uri(uri);
        let legacy_constants = backend.php_version().major < 8;
        for span in &symbol_map.spans {
            let is_declaration = matches!(
                &span.kind,
                SymbolKind::ClassDeclaration { .. }
                    | SymbolKind::FunctionCall {
                        is_definition: true,
                        ..
                    }
                    | SymbolKind::ConstantReference {
                        is_definition: true,
                        ..
                    }
                    | SymbolKind::MemberDeclaration { .. }
            );

            for (key, counts_as_reference) in self.reference_keys_for_span(is_resource, span) {
                if legacy_constants && let ReferenceIndexKey::Constant(name) = &key {
                    entries.push((
                        ReferenceIndexKey::ConstantFolded(name.to_ascii_lowercase()),
                        false,
                    ));
                }
                entries.push((key, counts_as_reference && !is_declaration));
            }
        }

        // A key behind a typed receiver is only settled lazily, and whoever
        // wants the answer has to find the file before it can be asked for.
        // The candidate joins the index unconfirmed, under every kind it
        // could turn out to be: this index narrows a workspace scan to the
        // files that *might* hold a key, and every consumer of it re-checks
        // what it found. Outside the framework a candidate belongs to it can
        // never be confirmed, and morph sites are recorded for every plain
        // `$x->prop === 'literal'`, so skip them there.
        let is_laravel = backend.resolved_class_cache.read().laravel().is_enabled();
        let frameworks = ReceiverFrameworks {
            laravel: is_laravel,
            symfony: backend.is_symfony_project(),
            service_container: backend.reads_service_container(),
            twig: backend.is_twig_project(),
            yii: backend.is_yii_project(),
            drupal: backend.is_drupal_project(),
        };
        let enabled = |kind| frameworks.enables(kind);
        for site in &symbol_map.typed_receiver_sites {
            if !site.receiver.names_any(enabled) {
                continue;
            }
            let mut kinds: Vec<StringKeyKind> = Vec::with_capacity(2);
            for (_, kind) in site.receiver.targets() {
                if enabled(*kind) && !kinds.contains(kind) {
                    kinds.push(*kind);
                }
            }
            for kind in kinds {
                entries.push((
                    ReferenceIndexKey::StringKey {
                        kind,
                        key: site.key.clone(),
                    },
                    false,
                ));
            }
        }
        if is_laravel {
            for site in &symbol_map.morph_column_sites {
                entries.push((
                    ReferenceIndexKey::StringKey {
                        kind: crate::symbol_map::StringKeyKind::MorphAlias,
                        key: site.key.clone(),
                    },
                    false,
                ));
            }
        }

        {
            for class in self.classes {
                for prop in &class.properties {
                    if prop.name_offset == 0 {
                        continue;
                    }
                    entries.push((
                        ReferenceIndexKey::Member {
                            name: prop
                                .name
                                .strip_prefix('$')
                                .unwrap_or(&prop.name)
                                .to_string(),
                            is_static: prop.is_static,
                        },
                        true,
                    ));
                }
            }
        }

        entries
    }

    /// The keys a span joins the index under, each paired with whether the
    /// span counts as a reference to that key (see
    /// [`Self::build`]).
    fn reference_keys_for_span(
        &self,
        is_resource: bool,
        span: &crate::symbol_map::SymbolSpan,
    ) -> Vec<(ReferenceIndexKey, bool)> {
        match &span.kind {
            SymbolKind::ClassReference { name, is_fqn, .. } => {
                // A resource document only ever records fully-qualified
                // names, so the short-name candidate key `class_keys` adds
                // for a source spelling that could differ from the resolved
                // FQN can never apply.  Skipping it keeps a large generated
                // XML from adding one index entry per distinct class it
                // names on top of the one that answers lookups.
                if *is_fqn && is_resource {
                    return vec![(
                        ReferenceIndexKey::class_owned(normalize_symbol_name(name)),
                        true,
                    )];
                }
                let resolved = if *is_fqn {
                    normalize_symbol_name(name)
                } else if let Some(fqn) = self.resolved_name_at(span.start) {
                    fqn
                } else {
                    let (use_map, namespace) = self.use_map_and_namespace_at(span.start);
                    normalize_symbol_name(resolve_to_fqn(name, use_map, &namespace))
                };
                class_keys(&resolved, name)
            }
            SymbolKind::ClassDeclaration { name } => {
                let namespace = namespace_in_spans(self.namespaces, span.start);
                let fqn = build_fqn(name, namespace);
                class_keys(&fqn, name)
            }
            SymbolKind::SelfStaticParent(kind) if *kind != SelfStaticParentKind::This => {
                let Some(current_class) =
                    crate::class_lookup::find_class_at_offset(self.classes, span.start)
                else {
                    return Vec::new();
                };
                let fqn = match kind {
                    SelfStaticParentKind::Parent => current_class
                        .parent_class
                        .as_ref()
                        .map(normalize_symbol_name),
                    _ => Some(current_class.fqn().to_string()),
                };
                fqn.map(|name| class_keys(&name, short_name(&name)))
                    .unwrap_or_default()
            }
            SymbolKind::FunctionCall { name, .. } => {
                let resolved = if let Some(fqn) = self.resolved_name_at(span.start) {
                    fqn
                } else {
                    let (use_map, namespace) = self.use_map_and_namespace_at(span.start);
                    normalize_symbol_name(resolve_to_fqn(name, use_map, &namespace))
                };
                let global_fallback = self.names_global_fallback(span.start, name, &resolved);
                function_keys(&resolved, name, global_fallback)
            }
            SymbolKind::ConstantReference { name, .. } => {
                let resolved = self
                    .resolved_name_at(span.start)
                    .unwrap_or_else(|| normalize_symbol_name(name));
                let global_fallback = self.names_global_fallback(span.start, name, &resolved);
                constant_keys(&resolved, name, global_fallback)
            }
            SymbolKind::MemberAccess {
                member_name,
                is_static,
                ..
            }
            | SymbolKind::MemberDeclaration {
                name: member_name,
                is_static,
                ..
            } => {
                vec![(
                    ReferenceIndexKey::Member {
                        name: member_name.to_string(),
                        is_static: *is_static,
                    },
                    true,
                )]
            }
            SymbolKind::StringKey { kind, key, .. } => {
                vec![(string_key_reference_key(*kind, key), true)]
            }
            _ => Vec::new(),
        }
    }

    fn resolved_name_at(&self, offset: u32) -> Option<String> {
        self.resolved_names?.get(offset).map(normalize_symbol_name)
    }

    fn use_map_and_namespace_at(&self, offset: u32) -> (&HashMap<String, String>, Option<String>) {
        if self.namespaces.len() > 1
            && let Some(span) = NamespaceSpan::containing(self.namespaces, offset)
        {
            return (&span.use_map, span.namespace.clone());
        }
        (
            self.use_map,
            namespace_in_spans(self.namespaces, offset).map(str::to_string),
        )
    }

    // Only an unqualified function or constant resolved in its own namespace
    // can fall back globally. Imports name their target directly.
    fn names_global_fallback(&self, offset: u32, source: &str, resolved: &str) -> bool {
        if source.contains('\\') || !resolved.contains('\\') {
            return false;
        }
        namespace_in_spans(self.namespaces, offset)
            .is_some_and(|namespace| build_fqn(source, Some(namespace)) == resolved)
    }
}
