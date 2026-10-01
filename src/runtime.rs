use std::fs;
use std::path::{Path, PathBuf};

use http::Uri;

use crate::fsutil::{
    MAX_DOCUMENT_BYTES, open_registry_directory, prepare_directory, read_regular_file,
    safe_identifier, write_new,
};
use crate::{LocalInferenceError, RUNTIME_SCHEMA, RuntimeRoute};

#[derive(Clone, Debug)]
pub struct RuntimeRouteRegistry {
    state_root: PathBuf,
}

impl RuntimeRouteRegistry {
    /// Opens existing endpoint routes. This is not exact model/runtime admission.
    ///
    /// # Errors
    /// Returns an error for unsafe paths or state I/O failure.
    pub fn open_existing(state_root: &Path) -> Result<Self, LocalInferenceError> {
        let state_root = open_registry_directory(state_root)?;
        open_registry_directory(&state_root.join("runtimes"))?;
        Ok(Self { state_root })
    }

    /// Explicitly provisions empty endpoint-route storage, never a default route.
    /// # Errors
    /// Unsafe paths and storage failures reject provisioning.
    pub fn provision(state_root: &Path) -> Result<Self, LocalInferenceError> {
        let state_root = prepare_directory(state_root)?;
        prepare_directory(&state_root.join("runtimes"))?;
        Ok(Self { state_root })
    }

    /// Adds one immutable provider-neutral runtime route.
    ///
    /// # Errors
    /// Returns an error for invalid routes, duplicate identifiers, or I/O.
    pub fn add(
        &self,
        id: &str,
        engine: &str,
        protocol: &str,
        endpoint: &str,
        capabilities: Vec<String>,
    ) -> Result<RuntimeRoute, LocalInferenceError> {
        let route = RuntimeRoute {
            schema: RUNTIME_SCHEMA.to_owned(),
            id: id.to_owned(),
            engine: engine.to_owned(),
            protocol: protocol.to_owned(),
            endpoint: endpoint.to_owned(),
            capabilities,
            enabled: true,
        };
        validate_route(&route)?;
        if self.routes()?.len() >= 256 {
            return Err(LocalInferenceError::new("capacity-exceeded"));
        }
        let bytes = serde_json::to_vec_pretty(&route)
            .map_err(|_| LocalInferenceError::new("runtime-route-invalid"))?;
        write_new(
            &self
                .state_root
                .join("runtimes")
                .join(format!("{}.json", route.id)),
            &bytes,
        )?;
        Ok(route)
    }

    /// Lists all validated runtime routes.
    ///
    /// # Errors
    /// Returns an error when a persisted route cannot be read or validated.
    pub fn routes(&self) -> Result<Vec<RuntimeRoute>, LocalInferenceError> {
        let mut routes = Vec::new();
        for entry in fs::read_dir(self.state_root.join("runtimes"))
            .map_err(|_| LocalInferenceError::new("state-io-failed"))?
        {
            let entry = entry.map_err(|_| LocalInferenceError::new("state-io-failed"))?;
            if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let bytes = read_regular_file(&entry.path(), MAX_DOCUMENT_BYTES.min(64 * 1024))?;
            let route: RuntimeRoute = serde_json::from_slice(&bytes)
                .map_err(|_| LocalInferenceError::new("runtime-route-invalid"))?;
            validate_route(&route)?;
            routes.push(route);
            if routes.len() > 256 {
                return Err(LocalInferenceError::new("capacity-exceeded"));
            }
        }
        routes.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(routes)
    }

    /// Resolves one runtime route by identifier.
    ///
    /// # Errors
    /// Returns an error when registry state is invalid or the route is unknown.
    pub fn route(&self, id: &str) -> Result<RuntimeRoute, LocalInferenceError> {
        self.routes()?
            .into_iter()
            .find(|route| route.id == id)
            .ok_or_else(|| LocalInferenceError::new("runtime-route-unknown"))
    }
}

fn validate_route(route: &RuntimeRoute) -> Result<(), LocalInferenceError> {
    let endpoint: Uri = route
        .endpoint
        .parse()
        .map_err(|_| LocalInferenceError::new("runtime-route-invalid"))?;
    let scheme = endpoint.scheme_str();
    let authority = endpoint.authority();
    if route.schema != RUNTIME_SCHEMA
        || !safe_identifier(&route.id)
        || !safe_identifier(&route.engine)
        || !matches!(
            route.protocol.as_str(),
            "openai-compatible" | "zixcel-inference-v1"
        )
        || !matches!(scheme, Some("http" | "https"))
        || authority.is_none()
        || authority.is_some_and(|value| value.as_str().contains('@'))
        || route.capabilities.is_empty()
        || route.capabilities.len() > 256
        || route.endpoint.len() > 4096
        || route
            .capabilities
            .iter()
            .any(|value| !safe_identifier(value))
    {
        return Err(LocalInferenceError::new("runtime-route-invalid"));
    }
    Ok(())
}
