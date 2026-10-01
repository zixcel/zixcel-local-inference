#![forbid(unsafe_code)]
#![doc = "Data-driven local inference catalog and verified artifact lifecycle for Zixcel."]

mod admission;
mod catalog;
mod configuration;
mod distribution;
mod dynamic_elf;
mod error;
mod fsutil;
mod install;
mod management;
#[cfg(target_os = "linux")]
pub mod process;
#[cfg(target_os = "linux")]
pub mod inference;
mod model;
mod runtime;
mod verification;
mod verification_control;

pub use admission::{
    AdmittedArtifact, AdmittedRuntime, ArtifactAdmission, ArtifactFormat, ArtifactProvenance,
    CpuDerivation, CpuPreparation, CpuSelectionObservation, RegistrySnapshot, RuntimeAdmission,
    RuntimeRegistry,
};
pub use catalog::{CatalogStore, SourceRecord};
pub use configuration::{ModelConfiguration, RuntimeConfiguration};
pub use distribution::{
    AdmittedDistribution, DistributionAlias, DistributionFile, HostCompatibility,
    RuntimeDistribution,
};
pub use dynamic_elf::DynamicMetadata;
pub use error::LocalInferenceError;
pub use install::{
    install_from_file, installation_statuses, installed_models, remove_installation,
};
pub use management::{RegistryCommand, RegistryReply, manage_registry};
pub use model::{
    AcquisitionPlan, Approval, ArtifactDescriptor, ArtifactIntegrity, ArtifactSource, Candidate,
    Catalog, CatalogDeliveryRequest, InstallationStatus, InstalledModel, ModelDescriptor,
    ReleaseDescriptor, ResolvedModel, RuntimeRoute, Selection, SignedCatalog,
};
pub use runtime::RuntimeRouteRegistry;
pub use verification_control::VerificationControl;

pub const SIGNED_CATALOG_SCHEMA: &str = "zixcel://local-inference/signed-catalog/v1";
pub const CATALOG_SCHEMA: &str = "zixcel://local-inference/catalog/v2";
pub const SOURCE_SCHEMA: &str = "zixcel://local-inference/catalog-source/v1";
pub const DELIVERY_SCHEMA: &str = "zixcel://local-inference/catalog-delivery-request/v1";
pub const ACQUISITION_SCHEMA: &str = "zixcel://local-inference/acquisition-plan/v2";
pub const INSTALLATION_SCHEMA: &str = "zixcel://local-inference/installation/v2";
pub const RUNTIME_SCHEMA: &str = "zixcel://local-inference/runtime-route/v1";
