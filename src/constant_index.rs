//! Constant names retain case in their last segment; namespaces do not.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::OnceLock;

use crate::ci_map::CiMap;

#[derive(Debug, Clone)]
struct Entry<V> {
    spelling: Option<String>,
    insensitive: bool,
    value: V,
}

/// A constant index with PHP's exact-first lookup and optional legacy folding.
#[derive(Debug, Clone)]
pub struct ConstantIndex<V> {
    inner: HashMap<String, Entry<V>>,
    variants: OnceLock<CiMap<Vec<String>>>,
}

impl<V> Default for ConstantIndex<V> {
    fn default() -> Self {
        Self::new()
    }
}

/// The key PHP stores: namespaces are always folded, the final name only
/// for a legacy case-insensitive declaration.
pub(crate) fn constant_key(name: &str, insensitive: bool) -> Cow<'_, str> {
    let end = if insensitive {
        name.len()
    } else {
        name.rfind('\\').unwrap_or(0)
    };
    if name.as_bytes()[..end].iter().any(u8::is_ascii_uppercase) {
        let mut key = name.to_string();
        key[..end].make_ascii_lowercase();
        Cow::Owned(key)
    } else {
        Cow::Borrowed(name)
    }
}

impl<V> ConstantIndex<V> {
    /// Construct an empty index without allocating.
    pub fn new() -> Self {
        Self {
            inner: HashMap::new(),
            variants: OnceLock::new(),
        }
    }

    /// Find the storage key, preserving exact-name precedence over folding.
    pub(crate) fn resolved_key(&self, name: &str) -> Option<&str> {
        self.lookup_entry(name).map(|(key, _)| key.as_str())
    }

    fn lookup_entry(&self, name: &str) -> Option<(&String, &Entry<V>)> {
        let exact = constant_key(name, false);
        if let Some(entry) = self.inner.get_key_value(exact.as_ref()) {
            return Some(entry);
        }
        let folded = crate::ci_map::fold(name);
        self.inner
            .get_key_value(folded.as_ref())
            .filter(|(_, entry)| entry.insensitive)
    }

    /// Read a declaration using PHP's constant lookup rules.
    pub fn get(&self, name: &str) -> Option<&V> {
        self.lookup_entry(name).map(|(_, entry)| &entry.value)
    }

    /// Read the exact stored entry, for declaration withdrawal and identity.
    pub(crate) fn get_stored(&self, key: &str) -> Option<&V> {
        self.inner.get(key).map(|entry| &entry.value)
    }

    /// Whether the reference names a declaration.
    pub fn contains_key(&self, name: &str) -> bool {
        self.resolved_key(name).is_some()
    }

    /// Insert a case-sensitive declaration.
    pub fn insert(&mut self, name: String, value: V) -> Option<V> {
        self.insert_with_case(name, value, false)
    }

    /// Insert a declaration with its target-version case rule.
    pub(crate) fn insert_with_case(
        &mut self,
        name: String,
        value: V,
        insensitive: bool,
    ) -> Option<V> {
        self.variants.take();
        let (key, spelling) = match constant_key(&name, insensitive) {
            Cow::Borrowed(_) => (name, None),
            Cow::Owned(key) => (key, Some(name)),
        };
        self.inner
            .insert(
                key,
                Entry {
                    spelling,
                    insensitive,
                    value,
                },
            )
            .map(|entry| entry.value)
    }

    /// Remove the exact stored identity, without selecting a folded fallback.
    pub fn remove(&mut self, key: &str) -> Option<V> {
        self.variants.take();
        self.inner
            .remove(constant_key(key, false).as_ref())
            .map(|entry| entry.value)
    }

    /// Visit declarations using their original spelling.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &V)> {
        self.inner
            .iter()
            .map(|(key, entry)| (entry.spelling.as_deref().unwrap_or(key), &entry.value))
    }

    /// Visit names using their original spelling.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.iter().map(|(name, _)| name)
    }

    /// Visit declaration values.
    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.inner.values().map(|entry| &entry.value)
    }

    /// Number of stored declarations.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Whether no declarations are stored.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Retain entries matching the predicate and retire the discovery cache.
    pub fn retain(&mut self, mut keep: impl FnMut(&str, &mut V) -> bool) {
        self.variants.take();
        self.inner
            .retain(|key, entry| keep(entry.spelling.as_deref().unwrap_or(key), &mut entry.value));
    }

    /// Clear declarations and their discovery cache.
    pub fn clear(&mut self) {
        self.inner.clear();
        self.variants.take();
    }

    /// Candidate declarations when a discovery scan has not parsed the case
    /// flag yet. Built once per index revision, only on a legacy lookup miss.
    pub(crate) fn case_variants(&self, name: &str) -> impl Iterator<Item = &V> {
        let variants = self.variants.get_or_init(|| {
            let mut variants = CiMap::new();
            for key in self.inner.keys() {
                variants
                    .or_insert_with(key.clone(), Vec::new)
                    .push(key.clone());
            }
            variants
        });
        variants
            .get(name)
            .into_iter()
            .flatten()
            .filter_map(|key| self.get_stored(key))
    }

    /// Approximate heap usage, including canonical spellings and discovery keys.
    #[cfg(feature = "mem-audit")]
    pub(crate) fn audit_heap(&self, value_heap: impl Fn(&V) -> usize) -> usize {
        self.inner.capacity() * (std::mem::size_of::<(String, Entry<V>)>() + 1)
            + self
                .inner
                .iter()
                .map(|(key, entry)| {
                    key.capacity()
                        + entry.spelling.as_ref().map_or(0, String::capacity)
                        + value_heap(&entry.value)
                })
                .sum::<usize>()
            + self.variants.get().map_or(0, |variants| {
                variants.audit_heap(|keys| {
                    keys.capacity() * std::mem::size_of::<String>()
                        + keys.iter().map(String::capacity).sum::<usize>()
                })
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_lookup_keeps_exact_names_and_namespace_case_rules() {
        let mut index = ConstantIndex::new();
        index.insert_with_case("Demo\\TOKEN".into(), 1, true);
        index.insert("Demo\\Token".into(), 2);
        assert_eq!(index.get("demo\\token"), Some(&1));
        assert_eq!(index.get("DEMO\\TOKEN"), Some(&1));
        assert_eq!(index.get("demo\\Token"), Some(&2));
        assert_eq!(index.resolved_key("Demo\\ToKeN"), Some("demo\\token"));
        index.insert("Demo\\TOKEN".into(), 3);
        assert_eq!(index.get("demo\\TOKEN"), Some(&3));
        assert_eq!(index.get("demo\\ToKeN"), Some(&1));
        assert_eq!(index.remove("demo\\token"), Some(1));
        assert_eq!(index.get("demo\\ToKeN"), None);
        assert_eq!(index.get("DEMO\\Token"), Some(&2));
    }

    #[test]
    fn constant_discovery_cache_tracks_insert_remove_and_retain() {
        let mut index = ConstantIndex::new();
        index.insert("Mixed".into(), 1);
        assert_eq!(
            index.case_variants("mixed").copied().collect::<Vec<_>>(),
            [1]
        );
        index.insert("MIXED".into(), 2);
        assert_eq!(index.case_variants("mixed").count(), 2);
        index.remove("Mixed");
        assert_eq!(
            index.case_variants("mixed").copied().collect::<Vec<_>>(),
            [2]
        );
        index.retain(|_, value| *value != 2);
        assert_eq!(index.case_variants("mixed").count(), 0);
        index.insert("Mixed".into(), 3);
        index.clear();
        assert_eq!(index.case_variants("mixed").count(), 0);
    }
}
