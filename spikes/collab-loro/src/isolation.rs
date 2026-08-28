#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsolationKind {
    TerminableInstance,
    WorkerSandbox,
    AsyncTimeout,
}

pub const REQUIRED_RUST_ISOLATION_KIND: IsolationKind = IsolationKind::TerminableInstance;

#[derive(Debug, Clone, Copy)]
pub struct ApplyRequest<'a> {
    pub update: &'a [u8],
    pub cpu_budget_ms: u64,
    pub wall_budget_ms: u64,
    pub memory_budget_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApplyResult {
    pub consumed_bytes: usize,
}

pub trait TerminationSignal: Send + Sync {
    fn is_termination_requested(&self) -> bool;
}

pub trait AllocationMeter {
    type Error: std::error::Error + Send + Sync + 'static;

    fn limit_bytes(&self) -> usize;
    fn used_bytes(&self) -> usize;
    fn try_reserve(&mut self, bytes: usize) -> Result<(), Self::Error>;
}

pub trait IsolatedApplyHost {
    type Error: std::error::Error + Send + Sync + 'static;
    type Meter: AllocationMeter;

    fn isolation_kind(&self) -> IsolationKind;
    fn apply(
        &self,
        request: ApplyRequest<'_>,
        termination: &dyn TerminationSignal,
        allocation_meter: &mut Self::Meter,
    ) -> Result<ApplyResult, Self::Error>;
    fn terminate(&self);
}
