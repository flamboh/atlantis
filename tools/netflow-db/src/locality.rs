use std::{
    collections::BTreeSet,
    fs,
    net::IpAddr,
    path::{Path, PathBuf},
};

use ipnet::IpNet;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

use crate::{domain::EndpointLocality, provenance::fingerprint};

const LOCALITY_IDENTITY_VERSION: u32 = 1;

#[derive(Debug, Error)]
pub enum LocalityError {
    #[error("locality rule {rule}: {message}")]
    InvalidRule { rule: usize, message: String },
    #[error("locality rule {rule}: unable to read address file {path}: {source}")]
    Io {
        rule: usize,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("locality rule {rule}: {path}:{line}: invalid address {value:?}")]
    InvalidFileAddress {
        rule: usize,
        path: PathBuf,
        line: usize,
        value: String,
    },
    #[error("unable to fingerprint locality rules: {0}")]
    Fingerprint(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum LocalityRuleConfig {
    TosAnonymized {},
    Prefixes {
        prefixes: Vec<String>,
    },
    Addresses {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        addresses: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<PathBuf>,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocalityRules {
    tos_anonymized: bool,
    prefixes: Vec<IpNet>,
    addresses: BTreeSet<IpAddr>,
}

impl LocalityRules {
    pub fn from_config(rules: &[LocalityRuleConfig], base: &Path) -> Result<Self, LocalityError> {
        let mut tos_anonymized = false;
        let mut prefixes = Vec::new();
        let mut addresses = BTreeSet::new();
        for (index, rule) in rules.iter().enumerate() {
            let rule_number = index + 1;
            let invalid = |message: String| LocalityError::InvalidRule {
                rule: rule_number,
                message,
            };
            match rule {
                LocalityRuleConfig::TosAnonymized {} => {
                    if tos_anonymized {
                        return Err(invalid("tos_anonymized is listed more than once".into()));
                    }
                    tos_anonymized = true;
                }
                LocalityRuleConfig::Prefixes { prefixes: values } => {
                    if values.is_empty() {
                        return Err(invalid("prefixes must list at least one CIDR".into()));
                    }
                    for value in values {
                        prefixes.push(parse_prefix(value).map_err(invalid)?);
                    }
                }
                LocalityRuleConfig::Addresses {
                    addresses: values,
                    path,
                } => match (values, path) {
                    (Some(values), None) => {
                        if values.is_empty() {
                            return Err(invalid("addresses must list at least one address".into()));
                        }
                        for value in values {
                            addresses.insert(parse_address(value).map_err(invalid)?);
                        }
                    }
                    (None, Some(path)) => {
                        let path = if path.is_absolute() {
                            path.clone()
                        } else {
                            base.join(path)
                        };
                        let loaded = read_address_file(&path, rule_number)?;
                        if loaded.is_empty() {
                            return Err(invalid(format!(
                                "address file {} lists no addresses",
                                path.display()
                            )));
                        }
                        addresses.extend(loaded);
                    }
                    _ => {
                        return Err(invalid(
                            "addresses rules need exactly one of `addresses` or `path`".into(),
                        ));
                    }
                },
            }
        }
        Ok(Self {
            tos_anonymized,
            prefixes: IpNet::aggregate(&prefixes),
            addresses,
        })
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        !self.tos_anonymized && self.prefixes.is_empty() && self.addresses.is_empty()
    }

    #[must_use]
    pub fn classify(&self, address: IpAddr, tos_anonymized: bool) -> EndpointLocality {
        if (self.tos_anonymized && tos_anonymized)
            || self.addresses.contains(&address)
            || self.prefixes.iter().any(|prefix| prefix.contains(&address))
        {
            EndpointLocality::Internal
        } else {
            EndpointLocality::External
        }
    }

    #[must_use]
    pub fn classify_flow(
        &self,
        src_ip: IpAddr,
        dst_ip: IpAddr,
        src_anonymized: bool,
        dst_anonymized: bool,
    ) -> (EndpointLocality, EndpointLocality) {
        (
            self.classify(src_ip, src_anonymized),
            self.classify(dst_ip, dst_anonymized),
        )
    }

    pub fn identity_payload(&self) -> Result<Value, LocalityError> {
        let prefixes = self
            .prefixes
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let addresses = self
            .addresses
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let digest = |values: &[String]| {
            fingerprint(values).map_err(|error| LocalityError::Fingerprint(error.to_string()))
        };
        Ok(json!({
            "version": LOCALITY_IDENTITY_VERSION,
            "match": "any",
            "tos_anonymized": self.tos_anonymized,
            "prefixes": {"count": prefixes.len(), "sha256": digest(&prefixes)?},
            "addresses": {"count": addresses.len(), "sha256": digest(&addresses)?},
        }))
    }
}

#[must_use]
pub const fn tos_anonymized_flags(src_tos: u8) -> (bool, bool) {
    (src_tos & 0b10 != 0, src_tos & 0b01 != 0)
}

fn parse_prefix(value: &str) -> Result<IpNet, String> {
    let prefix = value
        .trim()
        .parse::<IpNet>()
        .map_err(|_| format!("invalid CIDR prefix {value:?}"))?;
    if prefix.trunc() != prefix {
        return Err(format!(
            "CIDR prefix {value:?} has host bits set; use {}",
            prefix.trunc()
        ));
    }
    Ok(prefix)
}

fn parse_address(value: &str) -> Result<IpAddr, String> {
    value
        .trim()
        .parse::<IpAddr>()
        .map_err(|_| format!("invalid IP address {value:?}"))
}

fn read_address_file(path: &Path, rule: usize) -> Result<BTreeSet<IpAddr>, LocalityError> {
    let contents = fs::read_to_string(path).map_err(|source| LocalityError::Io {
        rule,
        path: path.to_owned(),
        source,
    })?;
    let mut addresses = BTreeSet::new();
    for (index, line) in contents.lines().enumerate() {
        let value = line.split('#').next().unwrap_or_default().trim();
        if value.is_empty() {
            continue;
        }
        let address = value
            .parse::<IpAddr>()
            .map_err(|_| LocalityError::InvalidFileAddress {
                rule,
                path: path.to_owned(),
                line: index + 1,
                value: value.to_owned(),
            })?;
        addresses.insert(address);
    }
    Ok(addresses)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::EndpointLocality::{External, Internal};
    use serde_json::json;
    use tempfile::tempdir;

    fn rules(value: Value) -> Result<LocalityRules, LocalityError> {
        let configs: Vec<LocalityRuleConfig> = serde_json::from_value(value).unwrap();
        LocalityRules::from_config(&configs, Path::new("/"))
    }

    fn ip(value: &str) -> IpAddr {
        value.parse().unwrap()
    }

    #[test]
    fn endpoints_are_internal_when_any_rule_matches_for_ipv4_and_ipv6() {
        let rules = rules(json!([
            {"type": "prefixes", "prefixes": ["192.0.2.0/25", "2001:db8:1::/48"]},
            {"type": "addresses", "addresses": ["198.51.100.7", "2001:db8:ffff::1"]}
        ]))
        .unwrap();

        assert_eq!(rules.classify(ip("192.0.2.1"), false), Internal);
        assert_eq!(rules.classify(ip("192.0.2.200"), false), External);
        assert_eq!(rules.classify(ip("198.51.100.7"), false), Internal);
        assert_eq!(rules.classify(ip("198.51.100.8"), false), External);
        assert_eq!(rules.classify(ip("2001:db8:1:2::9"), false), Internal);
        assert_eq!(rules.classify(ip("2001:db8:2::9"), false), External);
        assert_eq!(rules.classify(ip("2001:db8:ffff::1"), false), Internal);
        assert_eq!(
            rules.classify(ip("192.0.2.200"), true),
            External,
            "the ToS flag only counts when tos_anonymized is configured"
        );
    }

    #[test]
    fn tos_anonymized_marks_flagged_endpoints_internal_alongside_other_rules() {
        let rules = rules(json!([
            {"type": "tos_anonymized"},
            {"type": "addresses", "addresses": ["192.0.2.10"]}
        ]))
        .unwrap();

        assert_eq!(tos_anonymized_flags(0), (false, false));
        assert_eq!(tos_anonymized_flags(1), (false, true));
        assert_eq!(tos_anonymized_flags(2), (true, false));
        assert_eq!(tos_anonymized_flags(0b1111_1111), (true, true));
        let (src_anonymized, dst_anonymized) = tos_anonymized_flags(2);
        assert_eq!(
            rules.classify_flow(
                ip("203.0.113.1"),
                ip("203.0.113.2"),
                src_anonymized,
                dst_anonymized
            ),
            (Internal, External)
        );
        assert_eq!(
            rules.classify_flow(ip("192.0.2.10"), ip("203.0.113.2"), false, false),
            (Internal, External),
            "a literal internal address is internal without the ToS flag"
        );
    }

    #[test]
    fn rule_order_does_not_change_classification_or_identity() {
        let forward = rules(json!([
            {"type": "tos_anonymized"},
            {"type": "prefixes", "prefixes": ["192.0.2.0/24"]},
            {"type": "addresses", "addresses": ["2001:db8::1", "198.51.100.1"]}
        ]))
        .unwrap();
        let reversed = rules(json!([
            {"type": "addresses", "addresses": ["198.51.100.1", "2001:db8::1"]},
            {"type": "prefixes", "prefixes": ["192.0.2.0/25", "192.0.2.128/25"]},
            {"type": "tos_anonymized"}
        ]))
        .unwrap();

        assert_eq!(forward, reversed);
        assert_eq!(
            forward.identity_payload().unwrap(),
            reversed.identity_payload().unwrap()
        );
        for address in ["192.0.2.77", "198.51.100.1", "2001:db8::1", "203.0.113.9"] {
            for anonymized in [false, true] {
                assert_eq!(
                    forward.classify(ip(address), anonymized),
                    reversed.classify(ip(address), anonymized)
                );
            }
        }
    }

    #[test]
    fn identity_changes_with_rules_and_hides_address_lists() {
        let base = rules(json!([
            {"type": "tos_anonymized"},
            {"type": "addresses", "addresses": ["192.0.2.1"]}
        ]))
        .unwrap();
        let more_addresses = rules(json!([
            {"type": "tos_anonymized"},
            {"type": "addresses", "addresses": ["192.0.2.1", "192.0.2.2"]}
        ]))
        .unwrap();
        let without_tos = rules(json!([
            {"type": "addresses", "addresses": ["192.0.2.1"]}
        ]))
        .unwrap();
        let with_prefix = rules(json!([
            {"type": "tos_anonymized"},
            {"type": "addresses", "addresses": ["192.0.2.1"]},
            {"type": "prefixes", "prefixes": ["2001:db8::/32"]}
        ]))
        .unwrap();

        let payloads = [&base, &more_addresses, &without_tos, &with_prefix]
            .map(|rules| rules.identity_payload().unwrap());
        for (left_index, left) in payloads.iter().enumerate() {
            for right in payloads.iter().skip(left_index + 1) {
                assert_ne!(left, right);
            }
        }
        let text = payloads[3].to_string();
        assert!(!text.contains("192.0.2.1") && !text.contains("2001:db8"));
        assert_eq!(payloads[1]["addresses"]["count"], 2);
    }

    #[test]
    fn address_files_are_loaded_relative_to_the_base_with_comments() {
        let directory = tempdir().unwrap();
        fs::write(
            directory.path().join("internal.txt"),
            "# service hosts\n192.0.2.5  # www\n\n2001:db8::53\n",
        )
        .unwrap();
        let configs: Vec<LocalityRuleConfig> =
            serde_json::from_value(json!([{"type": "addresses", "path": "internal.txt"}])).unwrap();

        let rules = LocalityRules::from_config(&configs, directory.path()).unwrap();

        assert_eq!(rules.classify(ip("192.0.2.5"), false), Internal);
        assert_eq!(rules.classify(ip("2001:db8::53"), false), Internal);
        assert_eq!(rules.classify(ip("192.0.2.6"), false), External);

        fs::write(
            directory.path().join("internal.txt"),
            "192.0.2.5\nnot-an-ip\n",
        )
        .unwrap();
        let error = LocalityRules::from_config(&configs, directory.path()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains(":2: invalid address \"not-an-ip\""),
            "{error}"
        );
    }

    #[test]
    fn invalid_rules_are_rejected() {
        for (value, expected) in [
            (
                json!([{"type": "prefixes", "prefixes": ["192.0.2.0/33"]}]),
                "invalid CIDR prefix",
            ),
            (
                json!([{"type": "prefixes", "prefixes": ["192.0.2.1/24"]}]),
                "host bits set; use 192.0.2.0/24",
            ),
            (
                json!([{"type": "addresses", "addresses": ["192.0.2.256"]}]),
                "invalid IP address",
            ),
            (
                json!([{"type": "addresses", "addresses": []}]),
                "at least one address",
            ),
            (
                json!([{"type": "prefixes", "prefixes": []}]),
                "at least one CIDR",
            ),
            (
                json!([{"type": "addresses"}]),
                "exactly one of `addresses` or `path`",
            ),
            (
                json!([{"type": "addresses", "addresses": ["192.0.2.1"], "path": "x"}]),
                "exactly one of `addresses` or `path`",
            ),
            (
                json!([{"type": "tos_anonymized"}, {"type": "tos_anonymized"}]),
                "rule 2: tos_anonymized is listed more than once",
            ),
        ] {
            let error = rules(value.clone()).unwrap_err();
            assert!(error.to_string().contains(expected), "{value}: {error}");
        }
    }

    #[test]
    fn unknown_rule_types_and_fields_fail_deserialization() {
        for (value, expected) in [
            (
                json!([{"type": "asn", "asns": [64496]}]),
                "unknown variant `asn`",
            ),
            (
                json!([{"type": "prefixes", "prefixes": [], "prefix": []}]),
                "unknown field `prefix`",
            ),
            (
                json!([{"type": "tos_anonymized", "bits": 3}]),
                "unknown field `bits`",
            ),
            (
                json!([{"prefixes": ["192.0.2.0/24"]}]),
                "missing field `type`",
            ),
        ] {
            let error = serde_json::from_value::<Vec<LocalityRuleConfig>>(value.clone())
                .unwrap_err()
                .to_string();
            assert!(error.contains(expected), "{value}: {error}");
        }
    }

    #[test]
    fn empty_rules_classify_everything_external() {
        let rules = LocalityRules::from_config(&[], Path::new("/")).unwrap();
        assert!(rules.is_empty());
        assert_eq!(rules.classify(ip("192.0.2.1"), true), External);
    }
}
