//! The test tiers of spec-003 §6 (task B4): parsing the `--test` selection,
//! and the `unit` tier — each built repository's own `make test` in build
//! order, preceded in `zl-zpr-visaservice` by `make pregen ZPLC=<dist>/zplc`
//! so the visa service's policy fixtures are compiled by the set's own
//! compiler (the dynamic form of gate 3).

use anyhow::{Result, bail};

/// The tiers this tool implements today, in run order. Approved decision on
/// zipline#61 (Q1): `default` — and no `--test` flag at all — means every
/// implemented tier, so `docker` joins this list when B5 lands and nothing
/// about the flag changes.
const IMPLEMENTED: &[&str] = &["unit"];

/// Every tier spec-003 §6 names, implemented or not, in run order. A known
/// but unimplemented name gets a "lands in B5" error rather than the unknown-
/// name error, so the remedy is obvious from the message.
const KNOWN: &[&str] = &["unit", "netns", "docker"];

/// Which tiers a run executes, parsed from `--test` (spec-003 §7). Empty
/// means `--test none`: build only, no tier runs and none is reported.
#[derive(Debug, PartialEq)]
pub struct Selection {
    /// Tier names in run order, deduplicated.
    tiers: Vec<&'static str>,
}

impl Selection {
    /// Parses the `--test` flag. `None` (flag absent) and `default` select
    /// every implemented tier; `none` selects nothing; `all` asks for every
    /// known tier; otherwise the value is a comma-separated list of tier
    /// names. An unknown name, a known-but-unimplemented name, and the
    /// special words mixed into a list are all usage errors — they surface
    /// as exit 2 through `run`'s `Err` path.
    pub fn parse(flag: Option<&str>) -> Result<Selection> {
        // Absent and `default` are the same selection by definition
        // (approved Q1 on zipline#61): every implemented tier.
        let text = flag.unwrap_or("default");
        match text {
            "default" => {
                return Ok(Selection {
                    tiers: IMPLEMENTED.to_vec(),
                });
            }
            "none" => return Ok(Selection { tiers: Vec::new() }),
            // `all` asks for every known tier, and an asked-for tier that
            // cannot run is an error, never a silent skip (spec-003 §6) —
            // so while any tier is unimplemented, `all` is an error too.
            "all" => {
                bail!(
                    "--test all asks for every tier, but netns and docker land in B5; \
                     use --test unit (or default) until then"
                );
            }
            _ => {}
        }

        // A comma-separated list of tier names. The special whole-selection
        // words are rejected inside a list: `unit,none` has no coherent
        // meaning.
        let mut tiers: Vec<&'static str> = Vec::new();
        for name in text.split(',') {
            let name = name.trim();
            match KNOWN.iter().find(|known| **known == name) {
                Some(known) if IMPLEMENTED.contains(known) => {
                    if !tiers.contains(known) {
                        tiers.push(known);
                    }
                }
                Some(known) => bail!(
                    "--test {known} is not available yet: the netns and docker tiers land in B5"
                ),
                None => bail!(
                    "--test {name:?} is not a tier; valid values: none, default, all, \
                     or a comma-separated list of {}",
                    KNOWN.join(", ")
                ),
            }
        }
        Ok(Selection { tiers })
    }

    /// True when no tier was selected (`--test none`).
    pub fn is_empty(&self) -> bool {
        self.tiers.is_empty()
    }

    /// True when `tier` was selected. Consumed by the tier runner (B4
    /// step 2); the allow goes with that step, like `Tier`'s did.
    #[allow(dead_code)]
    pub fn contains(&self, tier: &str) -> bool {
        self.tiers.contains(&tier)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The flag absent entirely selects every implemented tier — today
    /// exactly `unit` (approved Q1 on zipline#61).
    #[test]
    fn absent_flag_selects_the_implemented_tiers() {
        let selection = Selection::parse(None).unwrap();
        assert!(!selection.is_empty());
        assert!(selection.contains("unit"));
        assert!(!selection.contains("netns"));
        assert!(!selection.contains("docker"));
    }

    /// `default` is the spelled-out form of the absent flag.
    #[test]
    fn default_matches_the_absent_flag() {
        assert_eq!(
            Selection::parse(Some("default")).unwrap(),
            Selection::parse(None).unwrap()
        );
    }

    /// `none` selects nothing: build only, byte-identical to B3 behaviour.
    #[test]
    fn none_selects_nothing() {
        let selection = Selection::parse(Some("none")).unwrap();
        assert!(selection.is_empty());
        assert!(!selection.contains("unit"));
    }

    /// A single implemented tier by name.
    #[test]
    fn unit_selects_exactly_unit() {
        let selection = Selection::parse(Some("unit")).unwrap();
        assert!(selection.contains("unit"));
        assert!(!selection.is_empty());
    }

    /// A comma-separated list is accepted, tolerating spaces and duplicates,
    /// as long as every name is implemented.
    #[test]
    fn comma_list_of_implemented_tiers_is_accepted() {
        let selection = Selection::parse(Some("unit, unit")).unwrap();
        assert!(selection.contains("unit"));
    }

    /// A known tier whose stage has not landed is rejected naming the stage,
    /// not treated as unknown: `netns` and `docker` land in B5.
    #[test]
    fn unimplemented_tier_is_rejected_naming_its_stage() {
        for name in ["netns", "docker", "unit,docker"] {
            let error = Selection::parse(Some(name)).unwrap_err().to_string();
            assert!(error.contains("B5"), "{name}: {error}");
        }
    }

    /// An unknown name is rejected listing the valid values, so the fix is
    /// obvious from the message (exit 2 through `run`'s `Err` path).
    #[test]
    fn unknown_tier_is_rejected_listing_the_valid_names() {
        for flag in ["bogus", "unit,bogus", ""] {
            let error = Selection::parse(Some(flag)).unwrap_err().to_string();
            assert!(error.contains("unit"), "{flag}: {error}");
            assert!(error.contains("none"), "{flag}: {error}");
        }
    }

    /// `none`, `default` and `all` describe whole selections, not tiers:
    /// mixing them into a list is a usage error.
    #[test]
    fn special_words_in_a_list_are_rejected() {
        for flag in ["unit,none", "none,unit", "unit,default", "unit,all"] {
            assert!(Selection::parse(Some(flag)).is_err(), "{flag}");
        }
    }

    /// `all` asks for every known tier, and today that is an error naming
    /// the unimplemented ones — an asked-for tier that cannot run must never
    /// silently degrade (spec-003 §6).
    #[test]
    fn all_is_an_error_while_tiers_are_unimplemented() {
        let error = Selection::parse(Some("all")).unwrap_err().to_string();
        assert!(error.contains("B5"), "{error}");
    }
}
