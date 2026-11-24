use crate::agent::model::{Model, ModelError, Request, Response};
use std::collections::VecDeque;
use std::sync::Mutex;

/// ScriptedModel replays an in-memory queue of responses or errors for tests.
pub struct ScriptedModel {
    responses: Mutex<VecDeque<Result<Response, ModelError>>>,
}

impl ScriptedModel {
    pub fn new(responses: Vec<Result<Response, ModelError>>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
        }
    }
}

impl Model for ScriptedModel {
    fn complete(&self, _req: &Request) -> Result<Response, ModelError> {
        let mut queue = self
            .responses
            .lock()
            .map_err(|_| ModelError::Other("mutex lock poisoned".to_string()))?;

        queue.pop_front().unwrap_or(Err(ModelError::EmptyScript))
    }
}
