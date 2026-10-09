// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Process-wide bounded blocking work for MCP and native tool Schema validation.

use jiaclaw_core::JiaClawError;
use std::sync::{Arc, OnceLock};
use tokio::{sync::Semaphore, time::Instant};

pub(crate) const MAX_SCHEMA_WORKERS: usize = 4;
static SCHEMA_WORKERS: OnceLock<SchemaWorkers> = OnceLock::new();

#[derive(Clone)]
pub(crate) struct SchemaWorkers {
    pub(crate) slots: Arc<Semaphore>,
}

#[derive(Clone, Copy)]
pub(crate) enum SchemaPhase {
    Initialization,
    Arguments,
    Result,
    NativeDefinitions,
    NativeBatch,
}

impl SchemaPhase {
    pub(crate) fn failure(self, reason: &str) -> JiaClawError {
        match self {
            Self::Initialization => {
                JiaClawError::Configuration(format!("MCP: schema {reason} during initialization"))
            }
            Self::Arguments => JiaClawError::ToolExecution(format!(
                "MCP: argument validation {reason}; request was not submitted"
            )),
            Self::Result => JiaClawError::ToolExecution(format!(
                "MCP: output validation {reason}; remote call completed but output was not accepted"
            )),
            Self::NativeDefinitions => JiaClawError::Configuration(format!(
                "native tool schema {reason}; no model request submitted"
            )),
            Self::NativeBatch => JiaClawError::StateKnotIntegration(format!(
                "Brokerrouter: tool batch validation {reason}; no tools dispatched"
            )),
        }
    }
}

pub(crate) fn process_workers() -> &'static SchemaWorkers {
    SCHEMA_WORKERS.get_or_init(SchemaWorkers::new)
}

impl SchemaWorkers {
    pub(crate) fn new() -> Self {
        Self {
            slots: Arc::new(Semaphore::new(MAX_SCHEMA_WORKERS)),
        }
    }

    /// Admission belongs to the actual blocking work, including time spent queued in Tokio.
    /// Dropping its waiter cannot cancel a running job or free its capacity prematurely.
    pub(crate) async fn run<T, F>(
        &self,
        phase: SchemaPhase,
        deadline: Instant,
        work: F,
    ) -> Result<T, JiaClawError>
    where
        T: Send + 'static,
        F: FnOnce() -> Result<T, JiaClawError> + Send + 'static,
    {
        if Instant::now() >= deadline {
            return Err(phase.failure("deadline exceeded"));
        }
        let permit = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| phase.failure("capacity busy"))?;
        let task = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            work()
        });
        let result = tokio::time::timeout_at(deadline, task)
            .await
            .map_err(|_| phase.failure("deadline exceeded"))?
            // Never include a panic payload, schema or argument in the public error.
            .map_err(|_| phase.failure("worker failed"))?;
        // A ready JoinHandle may be observed after the cutoff before Tokio polls its timer.
        // In particular, never accept late output or admit effects from late preflight results.
        if Instant::now() >= deadline {
            return Err(phase.failure("deadline exceeded"));
        }
        result
    }
}
