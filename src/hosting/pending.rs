//! A native callback keeps its recovery observation when deferred or reordered.
use serde_json::Value;
use std::ops::Deref;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Callback {
    pub value: Value,
    pub departure_observed: bool,
    pub repair_observed: bool,
    pub session_observed: bool,
    pub session_epoch: Option<u64>,
}

impl From<Value> for Callback {
    fn from(value: Value) -> Self {
        Self {
            value,
            departure_observed: false,
            repair_observed: false,
            session_observed: false,
            session_epoch: None,
        }
    }
}

impl Deref for Callback {
    type Target = Value;
    fn deref(&self) -> &Value {
        &self.value
    }
}
