//! Explicit CPU preparation and provenance. Not semantic/model routing or process readiness.
use super::{
    AdmittedDistribution, CommitIntent, CommitReceipt, Deserialize, Duration, LocalInferenceError,
    RevisionRef, RuntimeDistribution, RuntimeRegistry, Serialize, VerificationControl, commit, err,
    graph_error, identity,
};
use std::collections::{BTreeMap, BTreeSet};

const SELECTION_DOMAIN: &str = "zixcel/model/runtime/derivation/1";
/// Fixed owner rule: conservative x86-64 baseline, not filename-based dispatch.
pub const CPU_RULE: &str = "llama-9d817213a-x64-baseline-1";
// Exact audited b10741 artifact. The fixed CMake variant disables NATIVE/AVX/SSE42
// extensions; the ELF and its separate score function were inspected without loading it.
const BASELINE_SHA: &str = "469e17c98128fce2bd9e8397a6ec0e2c4b399e03a35a5c971ff8985f090688df";

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CpuSelectionObservation {
    pub os: String,
    pub architecture: String,
    pub required_features: Vec<String>,
    pub available_features: Vec<String>,
}
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CpuDerivation {
    pub derived_from: String,
    pub selection_purpose: String,
    pub selected_backend: String,
    pub selection_evidence: CpuSelectionObservation,
    pub derivation_rule_revision: String,
    pub distribution_ref: String,
}
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CpuPreparation {
    pub definition: RuntimeDistribution,
    pub provenance: CpuDerivation,
}

fn observe() -> CpuSelectionObservation {
    let mut available_features = Vec::new();
    #[cfg(target_arch = "x86_64")]
    if std::is_x86_feature_detected!("sse2") {
        available_features.push("sse2".into());
    }
    CpuSelectionObservation {
        os: std::env::consts::OS.into(),
        architecture: std::env::consts::ARCH.into(),
        required_features: vec!["sse2".into()],
        available_features,
    }
}
fn compatible(observation: &CpuSelectionObservation) -> Result<(), LocalInferenceError> {
    if observation.os != "linux"
        || observation.architecture != "x86_64"
        || observation.required_features != ["sse2"]
        || observation.available_features != ["sse2"]
    {
        return Err(err("host-incompatible"));
    }
    Ok(())
}

impl RuntimeRegistry {
    /// Pure compatibility preparation; exact bytes, closed subset, no commits or model copies.
    /// # Errors
    /// Unknown rule/artifact, corrupt bytes, incompatible host and unresolved closure reject.
    pub fn prepare_cpu_distribution(
        &self,
        source: &str,
        rule: &str,
    ) -> Result<CpuPreparation, LocalInferenceError> {
        validate_rule(rule)?;
        let observation = observe();
        compatible(&observation)?;
        self.verify_distribution(source)?;
        let image = self.snapshot()?;
        let original = image
            .distributions
            .iter()
            .find(|d| d.distribution_ref == source)
            .ok_or_else(|| err("distribution-not-found"))?;
        let definition = &original.definition;
        let selected = definition
            .files
            .iter()
            .find(|f| {
                definition.modules.contains(&f.path)
                    && image.artifacts.iter().any(|a| {
                        a.artifact_ref == f.artifact_ref && a.definition.sha256 == BASELINE_SHA
                    })
            })
            .ok_or_else(|| err("cpu-selection-unavailable"))?;
        let mut metadata = BTreeMap::new();
        let control = VerificationControl::new(Duration::from_secs(30));
        for f in &definition.files {
            let a = image
                .artifacts
                .iter()
                .find(|a| a.artifact_ref == f.artifact_ref)
                .ok_or_else(|| err("artifact-not-found"))?;
            let verified = crate::verification::verify(
                &self.object_path(&a.definition.sha256),
                &a.definition,
                &control,
            )?;
            metadata.insert(
                f.path.clone(),
                verified.dynamic.ok_or_else(|| err("unsupported-runtime"))?,
            );
        }
        let layout = definition.layout()?;
        let mut included = BTreeSet::new();
        let mut hosts = BTreeSet::new();
        let mut versions: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut pending = vec![definition.executable.as_str(), selected.path.as_str()];
        while let Some(name) = pending.pop() {
            let actual = *layout
                .get(name)
                .ok_or_else(|| err("distribution-dependency-unresolved"))?;
            if !included.insert(actual.to_owned()) {
                continue;
            }
            let meta = metadata
                .get(actual)
                .ok_or_else(|| err("distribution-dependency-unresolved"))?;
            for needed in &meta.needed {
                if let Some(target) = layout.get(needed.as_str()) {
                    pending.push(target);
                } else if definition.host.libraries.contains(needed) {
                    hosts.insert(needed.clone());
                } else {
                    return Err(err("distribution-dependency-unresolved"));
                }
            }
            for (library, required) in &meta.symbol_versions {
                if definition.host.libraries.contains(library) {
                    versions
                        .entry(library.clone())
                        .or_default()
                        .extend(required.iter().cloned());
                }
            }
        }
        let mut derived = definition.clone();
        derived.files.retain(|f| included.contains(&f.path));
        derived.aliases.retain(|a| {
            layout
                .get(a.path.as_str())
                .is_some_and(|p| included.contains(*p))
        });
        derived.modules = vec![selected.path.clone()];
        derived.host.libraries = hosts.into_iter().collect();
        derived.host.symbol_versions = versions
            .into_iter()
            .map(|(k, v)| (k, v.into_iter().collect()))
            .collect();
        derived.validate(&image.artifacts)?;
        metadata.retain(|k, _| included.contains(k));
        derived.verify_closure(&metadata)?;
        let provenance = CpuDerivation {
            derived_from: source.into(),
            selection_purpose: "cpu-execution".into(),
            selected_backend: selected.artifact_ref.clone(),
            selection_evidence: observation,
            derivation_rule_revision: rule.into(),
            distribution_ref: identity("runtime/distribution/1", &derived)?,
        };
        Ok(CpuPreparation {
            definition: derived,
            provenance,
        })
    }

    /// Explicit ordinary distribution admission followed by provenance publication.
    /// Retry preserves the original distribution receipt and repairs only missing provenance.
    /// # Errors
    /// Stale evidence/rule/host or ordinary admission failures reject. A provenance failure
    /// after admission is explicit and never rolls back the already committed distribution.
    pub fn admit_cpu_distribution(
        &self,
        preparation: CpuPreparation,
        expected: RevisionRef,
    ) -> Result<(AdmittedDistribution, CommitReceipt, CommitReceipt), LocalInferenceError> {
        let current = self.prepare_cpu_distribution(
            &preparation.provenance.derived_from,
            &preparation.provenance.derivation_rule_revision,
        )?;
        if current != preparation {
            return Err(err("cpu-selection-evidence-mismatch"));
        }
        let (distribution, admission_receipt) =
            self.admit_distribution(preparation.definition, expected)?;
        let provenance = preparation.provenance;
        let operation = identity("runtime/derivation/1", &provenance)?;
        let _lock = self.lock(true, &VerificationControl::new(Duration::from_secs(2)))?;
        let store = self.store()?;
        if let Some(receipt) = store
            .receipt(SELECTION_DOMAIN, &operation)
            .map_err(|e| graph_error(&e))?
        {
            return Ok((distribution, admission_receipt, receipt));
        }
        let head = store
            .current(SELECTION_DOMAIN)
            .map_err(|e| graph_error(&e))?;
        let previous = match head {
            Some(head) => {
                store
                    .receipt(SELECTION_DOMAIN, head.operation_id())
                    .map_err(|e| graph_error(&e))?
                    .ok_or_else(|| err("registry-corrupt"))?
                    .committed_revision
            }
            None => RevisionRef::default(),
        };
        if previous.sequence >= 128 {
            return Err(err("derivation-publication-capacity"));
        }
        let prepared = CommitIntent {
            domain: SELECTION_DOMAIN.into(),
            operation_id: operation,
            parents: previous.commit.iter().cloned().collect(),
            expected_revision: previous,
            payload: serde_json::to_vec(&provenance).map_err(|_| err("registry-corrupt"))?,
        }
        .prepare()
        .map_err(|_| err("derivation-publication-failed"))?;
        let published =
            commit(&store, &prepared).map_err(|_| err("derivation-publication-failed"))?;
        Ok((distribution, admission_receipt, published))
    }
}

fn validate_rule(rule: &str) -> Result<(), LocalInferenceError> {
    if rule == CPU_RULE {
        Ok(())
    } else {
        Err(err("selection-rule-unsupported"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_host_observation_rejects_missing_or_substituted_features() {
        let good = CpuSelectionObservation {
            os: "linux".into(),
            architecture: "x86_64".into(),
            required_features: vec!["sse2".into()],
            available_features: vec!["sse2".into()],
        };
        assert!(compatible(&good).is_ok());
        for bad in [
            CpuSelectionObservation {
                available_features: vec![],
                ..good.clone()
            },
            CpuSelectionObservation {
                architecture: "aarch64".into(),
                ..good.clone()
            },
            CpuSelectionObservation {
                required_features: vec![],
                ..good
            },
        ] {
            assert_eq!(
                compatible(&bad).expect_err("host mismatch").code(),
                "host-incompatible"
            );
        }
    }
}
