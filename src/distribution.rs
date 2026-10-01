//! Exact, non-executing distribution manifest. OS libraries are requirements,
//! never asserted to be immutable distribution bytes or proof of host readiness.
use crate::{AdmittedArtifact, ArtifactFormat, DynamicMetadata, LocalInferenceError};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DistributionFile {
    /// Exact leaf name in the supported flat $ORIGIN layout.
    pub path: String,
    pub artifact_ref: String,
}
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DistributionAlias {
    pub path: String,
    /// Relative in-distribution target; bounded acyclic chains are exact layout.
    pub target: String,
}
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostCompatibility {
    pub os: String,
    pub architecture: String,
    pub interpreter: String,
    /// Closed host ABI classes, not arbitrary paths or a runtime search list.
    pub libraries: Vec<String>,
    /// Required ABI symbol versions derived from exact ELF version-need tables.
    pub symbol_versions: BTreeMap<String, Vec<String>>,
}
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeDistribution {
    pub executable: String,
    pub files: Vec<DistributionFile>,
    pub aliases: Vec<DistributionAlias>,
    /// Explicit dlopen candidates, included even if absent from `DT_NEEDED`.
    pub modules: Vec<String>,
    pub host: HostCompatibility,
}
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdmittedDistribution {
    pub distribution_ref: String,
    pub definition: RuntimeDistribution,
}
pub(crate) fn leaf(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
}
fn error() -> LocalInferenceError {
    LocalInferenceError::new("distribution-invalid")
}

impl RuntimeDistribution {
    pub(crate) fn validate(
        &self,
        artifacts: &[AdmittedArtifact],
    ) -> Result<(), LocalInferenceError> {
        if self.files.is_empty()
            || self.files.len() > 48
            || self.aliases.len() > 48
            || self.modules.len() > 32
            || self.host.os != "linux"
            || !matches!(self.host.architecture.as_str(), "x86_64" | "aarch64")
            || self.host.interpreter
                != match self.host.architecture.as_str() {
                    "x86_64" => "/lib64/ld-linux-x86-64.so.2",
                    _ => "/lib/ld-linux-aarch64.so.1",
                }
            || self.host.libraries.is_empty()
            || self.host.libraries.len() > 16
        {
            return Err(error());
        }
        for pair in self.files.windows(2) {
            if pair[0].path >= pair[1].path {
                return Err(error());
            }
        }
        for pair in self.aliases.windows(2) {
            if pair[0].path >= pair[1].path {
                return Err(error());
            }
        }
        for values in [&self.modules, &self.host.libraries] {
            if values.windows(2).any(|pair| pair[0] >= pair[1]) {
                return Err(error());
            }
        }
        let names = self.layout()?;
        for file in &self.files {
            if !leaf(&file.path) {
                return Err(error());
            }
            let artifact = artifacts
                .iter()
                .find(|a| a.artifact_ref == file.artifact_ref)
                .ok_or_else(error)?;
            if artifact.definition.format != ArtifactFormat::DynamicElf64
                || artifact.architecture != self.host.architecture
            {
                return Err(error());
            }
        }
        if !names.contains_key(self.executable.as_str()) {
            return Err(error());
        }
        if self
            .modules
            .iter()
            .any(|name| !names.contains_key(name.as_str()) || name == &self.executable)
        {
            return Err(error());
        }
        for library in &self.host.libraries {
            if names.contains_key(library.as_str())
                || !matches!(
                    library.as_str(),
                    "libc.so.6"
                        | "libstdc++.so.6"
                        | "libgcc_s.so.1"
                        | "libm.so.6"
                        | "libpthread.so.0"
                        | "libdl.so.2"
                        | "librt.so.1"
                        | "libgomp.so.1"
                        | "libssl.so.3"
                        | "libcrypto.so.3"
                        | "ld-linux-x86-64.so.2"
                        | "ld-linux-aarch64.so.1"
                )
            {
                return Err(error());
            }
        }
        if self.host.symbol_versions.len() > 16
            || self.host.symbol_versions.iter().any(|(library, versions)| {
                !self.host.libraries.contains(library)
                    || versions.is_empty()
                    || versions.len() > 256
                    || versions.windows(2).any(|p| p[0] >= p[1])
                    || versions.iter().any(|v| !leaf(v))
            })
        {
            return Err(error());
        }
        Ok(())
    }

    pub(crate) fn verify_closure(
        &self,
        metadata: &BTreeMap<String, DynamicMetadata>,
    ) -> Result<(), LocalInferenceError> {
        let names = self.layout()?;
        let mut hosts = BTreeSet::new();
        let mut versions: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut reached = BTreeSet::new();
        let mut pending = vec![self.executable.as_str()];
        pending.extend(self.modules.iter().map(String::as_str));
        while let Some(name) = pending.pop() {
            let path = *names.get(name).ok_or_else(error)?;
            if !reached.insert(path) {
                continue;
            }
            let entry = metadata.get(path).ok_or_else(error)?;
            if entry.architecture != self.host.architecture {
                return Err(error());
            }
            if path == self.executable {
                if entry.interpreter.as_ref() != Some(&self.host.interpreter) {
                    return Err(error());
                }
            } else if entry.interpreter.is_some() {
                return Err(error());
            }
            if let Some(soname) = &entry.soname
                && names.get(soname.as_str()) != Some(&path)
            {
                return Err(error());
            }
            for needed in &entry.needed {
                if let Some(owned) = names.get(needed.as_str()) {
                    if entry.runpath.as_deref() != Some("$ORIGIN") {
                        return Err(error());
                    }
                    pending.push(owned);
                } else if self.host.libraries.contains(needed) {
                    hosts.insert(needed.clone());
                } else {
                    return Err(LocalInferenceError::new(
                        "distribution-dependency-unresolved",
                    ));
                }
            }
            for (library, values) in &entry.symbol_versions {
                if !entry.needed.contains(library) {
                    return Err(error());
                }
                if self.host.libraries.contains(library) {
                    versions
                        .entry(library.clone())
                        .or_default()
                        .extend(values.iter().cloned());
                }
            }
        }
        if reached.len() != self.files.len()
            || hosts.into_iter().collect::<Vec<_>>() != self.host.libraries
            || versions
                .into_iter()
                .map(|(k, v)| (k, v.into_iter().collect()))
                .collect::<BTreeMap<_, Vec<_>>>()
                != self.host.symbol_versions
        {
            return Err(error());
        }
        Ok(())
    }

    pub(crate) fn layout(&self) -> Result<BTreeMap<&str, &str>, LocalInferenceError> {
        let mut names: BTreeMap<&str, &str> = self
            .files
            .iter()
            .map(|f| (f.path.as_str(), f.path.as_str()))
            .collect();
        let mut aliases = BTreeMap::new();
        for alias in &self.aliases {
            if !leaf(&alias.path)
                || !leaf(&alias.target)
                || names.contains_key(alias.path.as_str())
                || aliases
                    .insert(alias.path.as_str(), alias.target.as_str())
                    .is_some()
            {
                return Err(error());
            }
        }
        for alias in &self.aliases {
            let mut target = alias.target.as_str();
            let mut seen = BTreeSet::new();
            while let Some(next) = aliases.get(target) {
                if !seen.insert(target) {
                    return Err(error());
                }
                target = next;
            }
            let actual = self
                .files
                .iter()
                .find(|f| f.path == target)
                .ok_or_else(error)?;
            names.insert(&alias.path, &actual.path);
        }
        Ok(names)
    }
}
