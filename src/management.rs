//! Shared owner command contract for CLI and host RPC adapters. No transport or UI.
use crate::{
    AdmittedArtifact, AdmittedDistribution, AdmittedRuntime, ArtifactAdmission,
    LocalInferenceError, RegistrySnapshot, RuntimeAdmission, RuntimeDistribution, RuntimeRegistry,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use zixcel_revision::{CommitReceipt, RevisionRef};

pub const MAX_MANAGEMENT_REQUEST_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(
    tag = "operation",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RegistryCommand {
    Process {
        command: crate::process::ProcessCommand,
    },
    PrepareCpuDistribution {
        reference: String,
        rule: String,
    },
    AdmitCpuDistribution {
        preparation: Box<crate::CpuPreparation>,
        expected_revision: RevisionRef,
    },
    Provision {},
    Recover {},
    Inspect {},
    DescribeRuntime {
        reference: String,
    },
    DescribeArtifact {
        reference: String,
    },
    VerifyArtifact {
        reference: String,
    },
    VerifyRuntime {
        reference: String,
    },
    VerifyDistribution {
        reference: String,
    },
    AdmitDistribution {
        definition: RuntimeDistribution,
        expected_revision: RevisionRef,
    },
    AdmitArtifact {
        source: PathBuf,
        definition: ArtifactAdmission,
        expected_revision: RevisionRef,
    },
    AdmitRuntime {
        definition: RuntimeAdmission,
        expected_revision: RevisionRef,
    },
}

impl RegistryCommand {
    /// Reads one bounded regular command file, without creating owner storage.
    /// # Errors
    /// Non-regular, oversized, linked or unavailable input rejects.
    pub fn from_file(path: &Path) -> Result<Self, LocalInferenceError> {
        let bytes = crate::fsutil::read_regular_file(path, MAX_MANAGEMENT_REQUEST_BYTES as u64)?;
        Self::from_json(&bytes)
    }
    /// Parses the same bounded owner request regardless of calling transport.
    /// # Errors
    /// Unknown fields/commands or oversized inputs reject before opening storage.
    pub fn from_json(bytes: &[u8]) -> Result<Self, LocalInferenceError> {
        if bytes.len() > MAX_MANAGEMENT_REQUEST_BYTES {
            return Err(LocalInferenceError::new("capacity-exceeded"));
        }
        let command: Self = serde_json::from_slice(bytes)
            .map_err(|_| LocalInferenceError::new("invalid-configuration"))?;
        match &command {
            Self::DescribeRuntime { reference }
            | Self::DescribeArtifact { reference }
            | Self::VerifyArtifact { reference }
            | Self::VerifyRuntime { reference }
            | Self::VerifyDistribution { reference }
                if reference.len() != 64
                    || !reference
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) =>
            {
                return Err(LocalInferenceError::new("invalid-configuration"));
            }
            Self::AdmitArtifact { source, .. } if source.as_os_str().len() > 4096 => {
                return Err(LocalInferenceError::new("invalid-configuration"));
            }
            _ => (),
        }
        Ok(command)
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "outcome", rename_all = "camelCase")]
pub enum RegistryReply {
    Process {
        observation: crate::process::ProcessReply,
    },
    CpuPrepared {
        preparation: crate::CpuPreparation,
    },
    CpuAdmitted {
        distribution: Box<AdmittedDistribution>,
        receipt: CommitReceipt,
        provenance_receipt: CommitReceipt,
    },
    Provisioned,
    Recovered,
    Snapshot(RegistrySnapshot),
    Runtime {
        runtime: AdmittedRuntime,
    },
    Artifact {
        artifact: AdmittedArtifact,
    },
    Verified {
        reference: String,
    },
    ArtifactAdmitted {
        artifact: AdmittedArtifact,
        receipt: CommitReceipt,
    },
    RuntimeAdmitted {
        runtime: AdmittedRuntime,
        receipt: CommitReceipt,
    },
    DistributionAdmitted {
        distribution: AdmittedDistribution,
        receipt: CommitReceipt,
    },
}

/// Runs only the explicit owner operation requested, returning no private local paths.
/// # Errors
/// Owner reasons remain unchanged; no synthetic defaults or recovery are used.
pub fn manage_registry(
    root: &Path,
    command: RegistryCommand,
) -> Result<RegistryReply, LocalInferenceError> {
    if let RegistryCommand::Process { command } = command {
        return Ok(RegistryReply::Process {
            observation: crate::process::request(root, &command)?,
        });
    }
    if matches!(command, RegistryCommand::Provision { .. }) {
        RuntimeRegistry::provision(root)?;
        return Ok(RegistryReply::Provisioned);
    }
    if matches!(command, RegistryCommand::Recover { .. }) {
        RuntimeRegistry::recover(root)?;
        return Ok(RegistryReply::Recovered);
    }
    let registry = RuntimeRegistry::open_existing(root)?;
    match command {
        RegistryCommand::Process { .. } => {
            unreachable!("process transport does not open admission transactions")
        }
        RegistryCommand::PrepareCpuDistribution { reference, rule } => {
            Ok(RegistryReply::CpuPrepared {
                preparation: registry.prepare_cpu_distribution(&reference, &rule)?,
            })
        }
        RegistryCommand::AdmitCpuDistribution {
            preparation,
            expected_revision,
        } => {
            let (distribution, receipt, provenance_receipt) =
                registry.admit_cpu_distribution(*preparation, expected_revision)?;
            Ok(RegistryReply::CpuAdmitted {
                distribution: Box::new(distribution),
                receipt,
                provenance_receipt,
            })
        }
        RegistryCommand::Provision {} | RegistryCommand::Recover {} => {
            unreachable!("explicit storage mutation handled without existing-state read")
        }
        RegistryCommand::Inspect {} => Ok(RegistryReply::Snapshot(registry.snapshot()?)),
        RegistryCommand::DescribeRuntime { reference } => Ok(RegistryReply::Runtime {
            runtime: registry.runtime(&reference)?,
        }),
        RegistryCommand::DescribeArtifact { reference } => Ok(RegistryReply::Artifact {
            artifact: registry.artifact(&reference)?,
        }),
        RegistryCommand::VerifyArtifact { reference } => {
            registry.verify_artifact(&reference)?;
            Ok(RegistryReply::Verified { reference })
        }
        RegistryCommand::VerifyRuntime { reference } => {
            match registry.verify_runtime(&reference) {
                Ok(()) => (),
                Err(e) if e.code() == "runtime-host-verification-required" => {
                    // Continue the explicitly deferred M4-A host phase; never start a
                    // process or substitute another runtime to make verification pass.
                    let observed =
                        crate::process::request(root, &crate::process::ProcessCommand::Inspect {})?;
                    if observed.rejection.is_some()
                        || !observed.process.is_some_and(|p| {
                            p.runtime_ref == reference
                                && p.state == crate::process::ProcessState::Ready
                        })
                    {
                        return Err(e);
                    }
                }
                Err(e) => return Err(e),
            }
            Ok(RegistryReply::Verified { reference })
        }
        RegistryCommand::VerifyDistribution { reference } => {
            registry.verify_distribution(&reference)?;
            Ok(RegistryReply::Verified { reference })
        }
        RegistryCommand::AdmitDistribution {
            definition,
            expected_revision,
        } => {
            let (distribution, receipt) =
                registry.admit_distribution(definition, expected_revision)?;
            Ok(RegistryReply::DistributionAdmitted {
                distribution,
                receipt,
            })
        }
        RegistryCommand::AdmitArtifact {
            source,
            definition,
            expected_revision,
        } => {
            let (artifact, receipt) =
                registry.admit_artifact(&source, definition, expected_revision)?;
            Ok(RegistryReply::ArtifactAdmitted { artifact, receipt })
        }
        RegistryCommand::AdmitRuntime {
            definition,
            expected_revision,
        } => {
            let (runtime, receipt) = registry.admit_runtime(definition, expected_revision)?;
            Ok(RegistryReply::RuntimeAdmitted { runtime, receipt })
        }
    }
}
