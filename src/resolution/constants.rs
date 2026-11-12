//! Looking up a global constant, the constant counterpart of
//! [`Backend::find_or_load_class`] and [`Backend::find_or_load_function`].
//!
//! Resolves a constant through the parsed defines, the autoload constant
//! index, known autoload files, and finally the embedded PHP stubs,
//! lazily parsing files as needed.

use crate::Backend;
use crate::php_type::PhpType;
use crate::types::DefineInfo;

/// Candidate declarations for a written constant. Only an unqualified,
/// unimported name may fall back from its namespace to the global scope.
pub(crate) fn constant_candidates<'a>(
    name: &'a str,
    resolved: Option<&'a str>,
    imports: &'a std::collections::HashMap<String, String>,
    constants: &'a std::collections::HashMap<String, String>,
    namespace: Option<&str>,
) -> [Option<std::borrow::Cow<'a, str>>; 2] {
    use std::borrow::Cow;
    if let Some(absolute) = name.strip_prefix('\\') {
        return [Some(Cow::Borrowed(absolute)), None];
    }
    if !name.contains('\\')
        && let Some(imported) = constants.get(name)
    {
        return [Some(Cow::Borrowed(imported)), None];
    }
    let primary = if let Some(resolved) = resolved {
        Cow::Borrowed(resolved.trim_start_matches('\\'))
    } else if let Some(relative) = name.strip_prefix("namespace\\") {
        namespace.map_or(Cow::Borrowed(relative), |ns| {
            Cow::Owned(format!("{ns}\\{relative}"))
        })
    } else if let Some((first, rest)) = name.split_once('\\')
        && let Some(imported) = crate::names::lookup_import(imports, first)
    {
        Cow::Owned(format!("{imported}\\{rest}"))
    } else {
        namespace.map_or(Cow::Borrowed(name), |ns| {
            Cow::Owned(format!("{ns}\\{name}"))
        })
    };
    let fallback = !name.contains('\\') && primary != name;
    [Some(primary), fallback.then_some(Cow::Borrowed(name))]
}

/// How far [`Backend::find_or_load_define`] looks for a declaration.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum DefineSearch {
    /// The parsed defines, and the one file the autoload constant index
    /// names for each candidate.
    Indexed,
    /// Those, then every known autoload file not parsed yet, for a
    /// `define()` the byte-level scan skipped inside an
    /// `if (!defined(…))` guard.
    Exhaustive,
}

impl Backend {
    /// Whether control flow proves this exact constant name is defined.
    pub(crate) fn constant_is_guarded(
        &self,
        uri: &str,
        content: &str,
        name: &str,
        offset: u32,
    ) -> bool {
        let Some(state) = self.file_state(uri) else {
            return false;
        };
        let guards = state.constant_guards.get_or_init(|| {
            std::sync::Arc::new(
                crate::diagnostics::existence_guards::compute_constant_guards(content),
            )
        });
        let block = crate::types::NamespaceSpan::containing(&state.namespaces, offset);
        let candidates = constant_candidates(
            name,
            state
                .resolved_names
                .as_ref()
                .and_then(|names| names.get(offset)),
            block
                .filter(|_| state.namespaces.len() > 1)
                .map_or(&state.use_map, |block| &block.use_map),
            block
                .filter(|_| state.namespaces.len() > 1)
                .map_or(&state.symbol_imports.constants, |block| {
                    &block.symbol_imports.constants
                }),
            block.and_then(|block| block.namespace.as_deref()),
        );
        candidates.iter().flatten().any(|name| {
            guards
                .get(crate::constant_index::constant_key(name, false).as_ref())
                .is_some_and(|ranges| {
                    ranges
                        .iter()
                        .any(|&(start, end)| start <= offset as usize && (offset as usize) < end)
                })
        })
    }

    /// Runtime type of an undefined bare name on PHP versions that still
    /// convert it to a string. A missing imported name uses its target basename;
    /// explicitly qualified source names always fail.
    pub(crate) fn undefined_constant_type(
        &self,
        name: &str,
        uri: Option<&str>,
        content: &str,
        offset: u32,
    ) -> Option<PhpType> {
        if self.php_version().major >= 8 || name.contains('\\') {
            return None;
        }
        if let Some(uri) = uri {
            if self.constant_is_guarded(uri, content, name, offset) {
                return None;
            }
            if let Some(state) = self.file_state(uri) {
                let block = crate::types::NamespaceSpan::containing(&state.namespaces, offset);
                if let Some(imported) = block
                    .filter(|_| state.namespaces.len() > 1)
                    .map_or(&state.symbol_imports.constants, |block| {
                        &block.symbol_imports.constants
                    })
                    .get(name)
                {
                    return Some(PhpType::literal_string_value(crate::util::short_name(
                        imported,
                    )));
                }
            }
        }
        Some(PhpType::literal_string_value(name))
    }

    /// Find the `define()` or top-level `const` declaring one of
    /// `candidates` in project or vendor code, and return what `read`
    /// takes from it.
    ///
    /// Searches in order, lazily parsing files as needed:
    /// 0. The stub paths, whose declaration wins over every other.
    /// 1. `global_defines` — constants already parsed.
    /// 2. `autoload_constant_index` — the file the byte-level scan found
    ///    the constant in.
    /// 3. With [`DefineSearch::Exhaustive`], `autoload_file_paths` — every
    ///    known autoload file not parsed yet.
    ///
    /// The candidates are names in the order PHP would try them (see
    /// [`Self::lookup_global_constant_candidates`]), and indexed declarations are
    /// tried in that order before the exhaustive file scan.
    /// The embedded stubs are not searched: what they declare has no file
    /// a declaration can be read back out of.
    pub(crate) fn find_or_load_define<T>(
        &self,
        candidates: &[&str],
        search: DefineSearch,
        read: impl Fn(&DefineInfo) -> T,
    ) -> Option<T> {
        let find_parsed = || {
            let dmap = self.symbols.global_defines.read();
            candidates.iter().find_map(|name| dmap.get(name).map(&read))
        };
        for name in candidates {
            if let Some(found) = self.load_external_stub_define(&[name], &read) {
                return Some(found);
            }
            if let Some(found) = self.symbols.global_defines.read().get(name).map(&read) {
                return Some(found);
            }
            let path = self
                .symbols
                .autoload_constant_index
                .read()
                .get(name)
                .cloned();
            if let Some(path) = path {
                let uri = crate::util::path_to_uri(&path);
                if !self.parsed_uris.read().contains(&uri)
                    && let Some(content) = crate::util::read_php_source(&path)
                {
                    self.update_ast(&uri, &content);
                    if let Some(found) = self.symbols.global_defines.read().get(name).map(&read) {
                        return Some(found);
                    }
                }
            }
            if self.php_version().major < 8 {
                let mut paths: Vec<_> = self
                    .symbols
                    .autoload_constant_index
                    .read()
                    .case_variants(name)
                    .cloned()
                    .collect();
                paths.sort_unstable();
                paths.dedup();
                for path in paths {
                    let uri = crate::util::path_to_uri(&path);
                    if self.parsed_uris.read().contains(&uri) {
                        continue;
                    }
                    if let Some(content) = crate::util::read_php_source(&path) {
                        self.update_ast(&uri, &content);
                        if let Some(found) = self.symbols.global_defines.read().get(name).map(&read)
                        {
                            return Some(found);
                        }
                    }
                }
            }
        }

        if search == DefineSearch::Indexed {
            return None;
        }

        // Phase 3: lazily parse known autoload files for constants the
        // byte-level scanner missed (e.g. inside `if (!defined(...))`
        // guards).  Each file is parsed at most once: later lookups are
        // answered by Phase 1.
        let paths = self.symbols.autoload_file_paths.read().clone();
        for path in &paths {
            let uri = crate::util::path_to_uri(path);
            if self.parsed_uris.read().contains(&uri) {
                continue;
            }
            if let Some(content) = crate::util::read_php_source(path) {
                self.update_ast(&uri, &content);
                if let Some(found) = find_parsed() {
                    return Some(found);
                }
            }
        }
        None
    }

    /// Look up a global constant by name, returning its value if found.
    ///
    /// Searches project and vendor code the way
    /// [`Self::find_or_load_define`] does, exhaustively, then
    /// `stub_constant_index` (built-in PHP constants from stubs), lazily
    /// parsing the stub file via `update_ast` (which populates
    /// `global_defines`) and re-checking.
    ///
    /// Returns `Some(Some(val))` when the constant exists with a known
    /// value, `Some(None)` when it exists but the value is unknown, and
    /// `None` when the constant was not found at all.
    ///
    /// A fully-qualified reference (`\PATHINFO_ALL`, written that way inside a
    /// namespace to skip the fallback lookup) names the same constant as the
    /// bare form, so the leading separator is dropped before searching.
    pub(crate) fn lookup_global_constant(&self, name: &str) -> Option<Option<String>> {
        self.lookup_global_constant_candidates(&[name.strip_prefix('\\').unwrap_or(name)])
    }

    /// [`Self::lookup_global_constant`] for `name` written in `namespace`
    /// with no imports in force, as in a class constant's initializer read
    /// from another file: an unqualified name is that namespace's constant
    /// when one is declared and the global one otherwise.
    pub(crate) fn lookup_constant_written_in(
        &self,
        name: &str,
        namespace: Option<&str>,
    ) -> Option<Option<String>> {
        match namespace {
            Some(namespace) if !name.contains('\\') => {
                let namespaced = format!("{namespace}\\{name}");
                self.lookup_global_constant_candidates(&[&namespaced, name])
            }
            _ => self.lookup_global_constant(name),
        }
    }

    /// [`Self::lookup_global_constant`] over a list of candidate names, in
    /// the order PHP would try them.
    ///
    /// A constant is indexed under its fully-qualified name, and the name a
    /// reference writes rarely is one: an unqualified `FOO` inside
    /// `namespace App` means `App\FOO` if that exists and global `FOO`
    /// otherwise, and a qualified `Config\FOO` means whatever the file's
    /// `use` table makes of `Config`.  Callers build the candidates with
    /// [`Backend::resolve_constant_name_at`](crate::Backend), which is what
    /// knows the file.
    ///
    /// Indexed declarations and built-ins are checked in candidate order.
    /// Only a miss across all candidates requires an exhaustive autoload scan.
    pub(crate) fn lookup_global_constant_candidates(
        &self,
        candidates: &[&str],
    ) -> Option<Option<String>> {
        for name in candidates {
            if let Some(value) = self.php_version_constant(name) {
                return Some(value);
            }
            if let Some((value, namespace, imports)) =
                self.find_or_load_define(&[name], DefineSearch::Indexed, |info| {
                    (
                        info.value.clone(),
                        info.namespace,
                        info.constant_imports.clone(),
                    )
                })
            {
                return Some(self.prepare_define_value(value, namespace, imports.as_deref()));
            }
            let stub_source = self.stub_constant_index.read().get(*name).copied();
            if let Some(source) = stub_source {
                self.update_ast(
                    &format!("{}const/{}", crate::stubs::STUB_URI_PREFIX, name),
                    source,
                );
                return Some(
                    self.symbols
                        .global_defines
                        .read()
                        .get(name)
                        .and_then(|info| info.value.clone()),
                );
            }
        }
        self.find_or_load_define(candidates, DefineSearch::Exhaustive, |info| {
            (
                info.value.clone(),
                info.namespace,
                info.constant_imports.clone(),
            )
        })
        .map(|(value, namespace, imports)| {
            self.prepare_define_value(value, namespace, imports.as_deref())
        })
    }

    fn prepare_define_value(
        &self,
        value: Option<String>,
        namespace: Option<crate::atom::Atom>,
        imports: Option<&std::collections::HashMap<String, String>>,
    ) -> Option<String> {
        value.map(|value| {
            if namespace.is_some() || self.php_version().major < 8 {
                self.qualify_value_constants(value, namespace.as_deref(), imports)
            } else {
                value
            }
        })
    }

    pub(crate) fn constant_identity(&self, candidates: &[&str]) -> Option<String> {
        let name = self.declared_constant_name(candidates)?;
        let defines = self.symbols.global_defines.read();
        Some(defines.resolved_key(name).unwrap_or(name).to_string())
    }

    /// Test existence without evaluating the declaration's initializer.
    pub(crate) fn constant_is_declared_at(
        &self,
        name: &str,
        offset: u32,
        file: &crate::types::FileContext,
    ) -> bool {
        let candidates = constant_candidates(
            name,
            file.resolved_names
                .as_ref()
                .and_then(|names| names.get(offset)),
            file.use_map_at(offset),
            &file.symbol_imports_at(offset).constants,
            file.namespace_at(offset).as_deref(),
        );
        let names = candidates
            .each_ref()
            .map(|name| name.as_deref().unwrap_or(""));
        let names = &names[..1 + usize::from(candidates[1].is_some())];
        self.declared_constant_name(names).is_some()
    }

    /// `value`, a constant's initializer written in `namespace`, with each
    /// unqualified constant it names spelled fully qualified as the one PHP
    /// reads: the namespace's own when it is declared, the global one
    /// otherwise.
    ///
    /// Whoever reads the value has a namespace of its own, or none, so the
    /// choice is made here, where the one it was written in is known.
    fn qualify_value_constants(
        &self,
        value: String,
        namespace: Option<&str>,
        imports: Option<&std::collections::HashMap<String, String>>,
    ) -> String {
        crate::parser::rewrite_unqualified_constant_reads(&value, |name| {
            if let Some(imported) = imports.and_then(|imports| imports.get(name)) {
                return Some(if self.declared_constant_name(&[imported]).is_some() {
                    format!("\\{imported}")
                } else {
                    PhpType::literal_string_value(crate::util::short_name(imported)).to_string()
                });
            }
            let qualified = namespace.map(|ns| format!("{ns}\\{name}"));
            let names = [qualified.as_deref().unwrap_or(name), name];
            let names = &names[..1 + usize::from(qualified.is_some())];
            self.declared_constant_name(names)
                .map(|name| format!("\\{name}"))
                .or_else(|| {
                    (self.php_version().major < 8)
                        .then(|| PhpType::literal_string_value(name).to_string())
                })
        })
        .unwrap_or(value)
    }

    /// The first of `candidates` that names a declared constant, searched
    /// the way [`Self::lookup_global_constant_candidates`] searches, but
    /// without reading its value.  Reading it could qualify the names in it
    /// in turn, and two constants naming each other would never finish.
    fn declared_constant_name<'c>(&self, candidates: &[&'c str]) -> Option<&'c str> {
        for &name in candidates {
            if self.php_version_constant(name).is_some()
                || self
                    .find_or_load_define(&[name], DefineSearch::Indexed, |_| ())
                    .is_some()
                || self.stub_constant_index.read().contains_key(name)
            {
                return Some(name);
            }
        }
        self.find_or_load_define(candidates, DefineSearch::Exhaustive, |_| ())?;
        let defines = self.symbols.global_defines.read();
        candidates
            .iter()
            .copied()
            .find(|name| defines.contains_key(name))
    }

    /// The value of a constant that describes the running PHP version, or
    /// `None` when `name` is not one.
    ///
    /// The major and minor versions are the configured ones.  The rest
    /// depend on the patch release, which nothing configures, so they exist
    /// with an unknown value (`Some(None)`).
    fn php_version_constant(&self, name: &str) -> Option<Option<String>> {
        let version = || self.php_version();
        match name {
            "PHP_MAJOR_VERSION" => Some(Some(version().major.to_string())),
            "PHP_MINOR_VERSION" => Some(Some(version().minor.to_string())),
            _ if unversioned_php_version_constant_type(name).is_some() => Some(None),
            _ => None,
        }
    }

    /// The initializer text of a global constant, looked up only where it
    /// costs a map probe or one targeted file parse to find.
    ///
    /// [`Self::lookup_global_constant`] ends by parsing *every* known
    /// autoload file, to catch a `define()` the byte-level scanner skipped
    /// inside an `if (!defined(…))` guard. That is worth paying when the user
    /// hovered one specific name, but not for a speculative lookup: the type
    /// engine asks about names taken out of a docblock type operator
    /// (`key-of<X>`, `X[K]`), where `X` is usually a template parameter and
    /// every miss would charge the whole autoload set.
    pub(crate) fn lookup_indexed_global_constant(&self, name: &str) -> Option<String> {
        let (value, namespace, imports) =
            self.find_or_load_define(&[name], DefineSearch::Indexed, |info| {
                (
                    info.value.clone(),
                    info.namespace,
                    info.constant_imports.clone(),
                )
            })?;
        self.prepare_define_value(value, namespace, imports.as_deref())
    }
}

/// The type of a PHP version constant whose value depends on the patch
/// release (see `Backend::php_version_constant`), or `None` for any other
/// name.
pub(crate) fn unversioned_php_version_constant_type(name: &str) -> Option<PhpType> {
    match name {
        "PHP_VERSION_ID" | "PHP_RELEASE_VERSION" => Some(PhpType::int()),
        "PHP_VERSION" | "PHP_EXTRA_VERSION" => Some(PhpType::string()),
        _ => None,
    }
}
