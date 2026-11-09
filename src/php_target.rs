//! One target-selection contract for the CLI, LSP startup, and restart notices.

use crate::{composer::ComposerPackage, config::Config, types::PhpVersion};

pub(crate) struct PhpTarget {
    pub(crate) version: PhpVersion,
    pub(crate) warnings: Vec<String>,
}

pub(crate) fn resolve(config: &Config, package: Option<&ComposerPackage>) -> PhpTarget {
    let mut warnings = Vec::new();
    if let Some(value) = config.php.version.as_deref() {
        if let Some(version) = exact_target(value) {
            return PhpTarget { version, warnings };
        }
        warnings.push(format!("Invalid php.version `{value}`; use an exact supported version such as 5.5 or 8.5. Falling back to Composer detection"));
    }
    let version = package
        .and_then(|package| from_package(package, &mut warnings))
        .unwrap_or_default();
    PhpTarget { version, warnings }
}

fn exact_target(value: &str) -> Option<PhpVersion> {
    PhpVersion::from_exact(value).filter(|version| PhpVersion::TARGETS.contains(version))
}

pub(crate) fn from_package(
    package: &ComposerPackage,
    warnings: &mut Vec<String>,
) -> Option<PhpVersion> {
    if let Some(platform) = package
        .config
        .as_ref()
        .and_then(|config| config.platform.get("php"))
    {
        if let mago_composer::ComposerPackageConfigPlatformValue::String(value) = platform
            && let Some(version) = exact_target(value)
        {
            return Some(version);
        }
        warnings.push("Composer config.platform.php does not name a supported exact PHP version; falling back to require.php".into());
    }
    let constraint = package.require.get("php")?;
    let version = PhpVersion::from_composer_constraint(constraint);
    if version.is_none() {
        warnings.push(format!("Cannot select a supported PHP target from Composer require.php `{constraint}`; using PHP {}. Set php.version explicitly", PhpVersion::default()));
    }
    version
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn php_target_initial_selection_restores_filtered_stubs() {
        use std::collections::HashMap;
        let source = "<?php\n/** @removed 8.0 */\nclass LegacyTarget {}\n/** @removed 8.0 */\nfunction legacy_target() {}\n/** @removed 8.0 */\ndefine('LEGACY_TARGET', 1);\n";
        let backend = crate::Backend::new_test_with_all_stubs(
            HashMap::from([("LegacyTarget", source)]),
            HashMap::from([("legacy_target", source)]),
            HashMap::from([("LEGACY_TARGET", source)]),
        );
        for (major, minor) in [(5, 5), (8, 5), (5, 6), (5, 5)] {
            backend.set_php_version(PhpVersion::new(major, minor));
            assert_eq!(
                backend
                    .stub_constant_index
                    .read()
                    .contains_key("LEGACY_TARGET"),
                major == 5
            );
            assert_eq!(
                backend
                    .stub_function_index
                    .read()
                    .contains_key("legacy_target"),
                major == 5
            );
            assert_eq!(
                backend.stub_index.read().contains_key("LegacyTarget"),
                major == 5
            );
        }
        assert!(backend.find_or_load_class("LegacyTarget").is_some());
    }

    #[test]
    fn php_target_constraints_use_the_lowest_satisfiable_family() {
        for (constraint, expected) in [
            ("^8.0 || ^5.5", Some((5, 5))),
            ("^5.5 | ^8.0", Some((5, 5))),
            ("<7.0 >=5.6", Some((5, 6))),
            (">=5.5, >=5.6, <7.0", Some((5, 6))),
            ("<7", Some((5, 5))),
            ("<=5.5.0", Some((5, 5))),
            (">5.5.0 <5.6", Some((5, 5))),
            (">5.5", Some((5, 5))),
            ("5.6.0-beta2", Some((5, 6))),
            ("8.0.0-RC1", Some((8, 0))),
            ("^5.5@dev", Some((5, 5))),
            (">=5.5 !=5.5.* <7", None),
            (">=5.5 !=5.5.0 <5.6", Some((5, 5))),
            ("5.5 - 5.6", Some((5, 5))),
            ("~5.6.12", Some((5, 6))),
            ("5.6.x", Some((5, 6))),
            ("8.3.1", Some((8, 3))),
            ("^8", Some((8, 0))),
            ("*", Some((5, 5))),
            (">=8.4 <8.0 || ^5.6", Some((5, 6))),
            (">=5.6 <5.6", None),
            ("<5.5", None),
            ("^6.0", None),
            (">=9.0", None),
            ("dev-main", None),
            ("banana8.5", None),
            ("", None),
        ] {
            assert_eq!(
                PhpVersion::from_composer_constraint(constraint),
                expected.map(|(major, minor)| PhpVersion::new(major, minor)),
                "{constraint}"
            );
        }
    }

    #[test]
    fn php_target_override_platform_requirement_and_default_have_one_order() {
        for (override_value, platform, requirement, expected, warning_count) in [
            (Some("5.5"), Some("8.3.1"), Some("^8.0"), (5, 5), 0),
            (None, Some("5.6.40"), Some("^8.0"), (5, 6), 0),
            (None, None, Some("^8.0 || ^5.5"), (5, 5), 0),
            (None, None, None, (8, 5), 0),
            (Some("^5.5"), Some("5.6.40"), None, (5, 6), 1),
            (Some("5.5junk"), None, Some("<7"), (5, 5), 1),
            (Some("6.0"), None, Some("^5.6"), (5, 6), 1),
            (None, Some("^5.5"), Some("^5.6"), (5, 6), 1),
            (None, None, Some("<5.5"), (8, 5), 1),
            (None, None, Some("broken"), (8, 5), 1),
        ] {
            let mut cfg = Config::default();
            cfg.php.version = override_value.map(str::to_owned);
            let mut value = json!({});
            if let Some(platform) = platform {
                value["config"] = json!({"platform": {"php": platform}});
            }
            if let Some(requirement) = requirement {
                value["require"] = json!({"php": requirement});
            }
            let package = serde_json::from_value(value).unwrap();
            let target = resolve(&cfg, Some(&package));
            assert_eq!(target.version, PhpVersion::new(expected.0, expected.1));
            assert_eq!(
                target.warnings.len(),
                warning_count,
                "{:?}",
                target.warnings
            );
        }
        assert_eq!(
            resolve(&Config::default(), None).version,
            PhpVersion::default()
        );
    }

    #[test]
    fn php_target_metadata_does_not_use_constraint_selection() {
        assert_eq!(
            PhpVersion::from_metadata("5.3.0 removed later"),
            Some(PhpVersion::new(5, 3))
        );
        assert_eq!(
            PhpVersion::from_metadata("8.0 use another API"),
            Some(PhpVersion::new(8, 0))
        );
        for invalid in ["^5.5", "5.5.*", "5.5foo", "5.5.0.1", "5.5.", "5"] {
            assert_eq!(PhpVersion::from_exact(invalid), None, "{invalid}");
        }
    }
}
