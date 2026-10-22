//! An [`RcCall`] for tests: records every call and answers from a script, so how many rc calls
//! a transport method makes, and with which parameters, is a unit test instead of a benchmark
//! against a real server.

use std::sync::Mutex;

use async_trait::async_trait;
use filebridge_transport_api::Result;
use serde_json::{Value, json};

use crate::RcCall;
use crate::fs::Scrubber;

type Script = Box<dyn Fn(&str, &Value) -> Option<Result<Value>> + Send + Sync>;

/// Answers what `script` answers; a method the script leaves alone gets `{}`.
pub(crate) struct Recording {
    script: Script,
    calls: Mutex<Vec<(String, Value)>>,
    scrubber: Scrubber,
}

impl Recording {
    pub(crate) fn new(
        script: impl Fn(&str, &Value) -> Option<Result<Value>> + Send + Sync + 'static,
    ) -> Self {
        Self {
            script: Box::new(script),
            calls: Mutex::new(Vec::new()),
            scrubber: Scrubber::default(),
        }
    }

    /// The parameters of every call to `method`, in order.
    pub(crate) fn calls_to(&self, method: &str) -> Vec<Value> {
        self.calls
            .lock()
            .expect("recording lock")
            .iter()
            .filter(|(m, _)| m == method)
            .map(|(_, p)| p.clone())
            .collect()
    }
}

#[async_trait]
impl RcCall for Recording {
    async fn call(&self, method: &str, params: Value) -> Result<Value> {
        let answer = (self.script)(method, &params).unwrap_or_else(|| Ok(json!({})));
        self.calls
            .lock()
            .expect("recording lock")
            .push((method.to_owned(), params));
        answer
    }

    fn scrubber(&self) -> &Scrubber {
        &self.scrubber
    }
}
