//! Operational request deduplication only. Never restores a process or treats PID/Ready as truth.
use super::{
    CommitIntent, Duration, LocalInferenceError, RevisionRef, RuntimeRegistry, VerificationControl,
    commit, err, graph_error, identity,
};
use std::collections::BTreeMap;
const DOMAIN: &str = "zixcel/model/runtime/process/request/1";
type Requests = BTreeMap<String, (String, String)>;
impl RuntimeRegistry {
    pub(crate) fn process_requests(&self) -> Result<Requests, LocalInferenceError> {
        let _lock = self.lock(false, &VerificationControl::new(Duration::from_secs(2)))?;
        let store = self.store()?;
        let Some(head) = store.current(DOMAIN).map_err(|e| graph_error(&e))? else {
            return Ok(Requests::new());
        };
        decode(head.payload())
    }
    pub(crate) fn record_process_request(
        &self,
        request: &str,
        runtime: &str,
        process: &str,
    ) -> Result<(), LocalInferenceError> {
        let _lock = self.lock(true, &VerificationControl::new(Duration::from_secs(2)))?;
        let store = self.store()?;
        let current = store.current(DOMAIN).map_err(|e| graph_error(&e))?;
        let (previous, mut requests) = if let Some(head) = current {
            let receipt = store
                .receipt(DOMAIN, head.operation_id())
                .map_err(|e| graph_error(&e))?
                .ok_or_else(|| err("registry-corrupt"))?;
            (receipt.committed_revision, decode(head.payload())?)
        } else {
            (RevisionRef::default(), Requests::new())
        };
        let binding = (runtime.to_owned(), process.to_owned());
        if let Some(old) = requests.get(request) {
            return if old == &binding {
                Ok(())
            } else {
                Err(err("process-request-conflict"))
            };
        }
        if requests.len() >= 128 {
            return Err(err("process-request-capacity"));
        }
        requests.insert(request.into(), binding);
        let intent = CommitIntent {
            domain: DOMAIN.into(),
            operation_id: identity(DOMAIN, &(request, runtime, process))?,
            parents: previous.commit.iter().cloned().collect(),
            expected_revision: previous,
            payload: serde_json::to_vec(&requests).map_err(|_| err("registry-corrupt"))?,
        }
        .prepare()
        .map_err(|_| err("registry-corrupt"))?;
        commit(&store, &intent)?;
        Ok(())
    }
}
fn decode(bytes: &[u8]) -> Result<Requests, LocalInferenceError> {
    if bytes.len() > 32768 {
        return Err(err("registry-corrupt"));
    }
    let result: Requests = serde_json::from_slice(bytes).map_err(|_| err("registry-corrupt"))?;
    if result.len() > 128
        || result
            .iter()
            .any(|(k, (r, p))| ![k, r, p].into_iter().all(|v| super::digest(v)))
    {
        return Err(err("registry-corrupt"));
    }
    Ok(result)
}
