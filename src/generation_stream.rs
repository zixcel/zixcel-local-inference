//! Engine-neutral NDJSON events after a backend adapter has decoded its protocol.
//! Observed deltas are provisional until a terminal event and clean EOF validate.
use crate::LocalInferenceError;
use crowsi_transport_foundation::{Decoder, Limits};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use zixcel_cost::{CostEstimate, CostRateCard, TokenUsage};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "event", rename_all = "camelCase", deny_unknown_fields)]
pub enum GenerationEvent {
    Delta { sequence: u64, text: String },
    Finished { usage: TokenUsage },
}

#[derive(Debug)]
pub struct GenerationReceipt {
    pub text: String,
    pub usage: TokenUsage,
    pub cost: Option<CostEstimate>,
    pub first_text_after: Option<Duration>,
    pub elapsed: Duration,
}

pub struct GenerationStream {
    decoder: Decoder,
    maximum_tokens: u32,
    bytes: usize,
    sequence: u64,
    text: String,
    usage: Option<TokenUsage>,
    failed: bool,
    started: Instant,
    first_text_after: Option<Duration>,
}

impl GenerationStream {
    /// Bounded stream using the existing physical inference output limits.
    /// # Errors
    /// Token bounds outside 1..=256 reject without allocating a stream.
    pub fn new(maximum_tokens: u32) -> Result<Self, LocalInferenceError> {
        if !(1..=256).contains(&maximum_tokens) {
            return Err(error());
        }
        let limits = Limits {
            accepted: 65536,
            buffered: 65537,
            frame: 65538,
            pending_bytes: 65538,
            pending_messages: 1,
        };
        Ok(Self {
            decoder: Decoder::new(limits).map_err(|_| error())?,
            maximum_tokens,
            bytes: 0,
            sequence: 0,
            text: String::new(),
            usage: None,
            failed: false,
            started: Instant::now(),
            first_text_after: None,
        })
    }

    /// Delivers sequenced, provisional text through a synchronous receiver.
    /// Receiver errors poison the stream; the caller must cancel engine work.
    /// # Errors
    /// Malformed, excessive, reordered or post-terminal data reject permanently.
    pub fn push(
        &mut self,
        bytes: &[u8],
        mut receive: impl FnMut(&str) -> std::io::Result<()>,
    ) -> Result<(), LocalInferenceError> {
        let result = self.consume(bytes, &mut receive);
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn consume(
        &mut self,
        mut bytes: &[u8],
        receive: &mut impl FnMut(&str) -> std::io::Result<()>,
    ) -> Result<(), LocalInferenceError> {
        if self.failed || bytes.len() > 65536_usize.saturating_sub(self.bytes) {
            return Err(error());
        }
        self.bytes += bytes.len();
        while !bytes.is_empty() {
            let (n, frame) = self.decoder.feed(bytes).map_err(|_| error())?;
            bytes = &bytes[n..];
            if let Some(frame) = frame {
                if self.usage.is_some() {
                    return Err(error());
                }
                match serde_json::from_slice::<GenerationEvent>(&frame.payload)
                    .map_err(|_| error())?
                {
                    GenerationEvent::Delta { sequence, text } => {
                        if sequence != self.sequence + 1
                            || sequence > 256
                            || text.is_empty()
                            || text.len() > 4096_usize.saturating_sub(self.text.len())
                        {
                            return Err(error());
                        }
                        self.sequence = sequence;
                        self.first_text_after
                            .get_or_insert_with(|| self.started.elapsed());
                        receive(&text)
                            .map_err(|_| LocalInferenceError::new("generation-receiver-failed"))?;
                        self.text.push_str(&text);
                    }
                    GenerationEvent::Finished { usage } => {
                        if usage.cached_input_tokens > usage.input_tokens
                            || usage.output_tokens > u64::from(self.maximum_tokens)
                        {
                            return Err(error());
                        }
                        self.usage = Some(usage);
                    }
                }
            }
        }
        Ok(())
    }

    /// Completes only at transport EOF with a newline-terminated finish event.
    /// Cost remains caller-rated and is never calculated from chunk counts.
    /// # Errors
    /// Missing completion, partial trailing frames or invalid cost reject.
    pub fn finish(
        mut self,
        rates: Option<&CostRateCard>,
    ) -> Result<GenerationReceipt, LocalInferenceError> {
        if self.failed || self.decoder.finish().map_err(|_| error())?.is_some() {
            return Err(error());
        }
        let usage = self.usage.ok_or_else(error)?;
        let cost = rates
            .map(|rates| zixcel_cost::estimate(&usage, rates))
            .transpose()
            .map_err(|_| error())?;
        Ok(GenerationReceipt {
            text: self.text,
            usage,
            cost,
            first_text_after: self.first_text_after,
            elapsed: self.started.elapsed(),
        })
    }
}
fn error() -> LocalInferenceError {
    LocalInferenceError::new("generation-stream-invalid")
}
