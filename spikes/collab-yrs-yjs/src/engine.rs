use crate::InputError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineMetadata {
    pub candidate: &'static str,
    pub rust_engine: &'static str,
    pub rust_engine_version: &'static str,
}

#[must_use]
pub const fn metadata() -> EngineMetadata {
    EngineMetadata {
        candidate: "yrs-yjs",
        rust_engine: "yrs",
        rust_engine_version: "0.27.3",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputLimits {
    pub snapshot_bytes_max: usize,
    pub update_bytes_max: usize,
}

impl InputLimits {
    pub fn validate_snapshot(self, snapshot: &[u8]) -> Result<(), InputError> {
        validate_input("snapshot", snapshot, self.snapshot_bytes_max)
    }

    pub fn validate_update(self, update: &[u8]) -> Result<(), InputError> {
        validate_input("update", update, self.update_bytes_max)
    }
}

fn validate_input(input_name: &'static str, input: &[u8], max_bytes: usize) -> Result<(), InputError> {
    if input.is_empty() {
        return Err(InputError::Empty { input: input_name });
    }
    if input.len() > max_bytes {
        return Err(InputError::LimitExceeded {
            input: input_name,
            actual_bytes: input.len(),
            max_bytes,
        });
    }
    Ok(())
}

pub trait CollabEngine: Sized {
    type Diff;
    type Error: std::error::Error + Send + Sync + 'static;
    type Frontier;

    fn load(snapshot: &[u8], limits: InputLimits) -> Result<Self, Self::Error>;
    fn import_update(&mut self, update: &[u8], limits: InputLimits) -> Result<Self::Diff, Self::Error>;
    fn export_snapshot(&self) -> Result<Vec<u8>, Self::Error>;
    fn export_from(&self, frontier: &Self::Frontier) -> Result<Vec<u8>, Self::Error>;
    fn frontier(&self) -> Self::Frontier;
}
