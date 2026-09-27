/// High-level convergence state returned by a reconciliation cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileState {
    /// Observed state matches desired state and no action is required.
    Converged,
    /// One or more actions should move observed state toward desired state.
    Progressing,
    /// Policy or in-flight work prevents immediate convergence.
    Blocked,
}

/// Pure, auditable output of one reconciliation cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcilePlan<R, A> {
    state: ReconcileState,
    resource_update: Option<R>,
    actions: Vec<A>,
}

impl<R, A> ReconcilePlan<R, A> {
    pub(crate) const fn new(
        state: ReconcileState,
        resource_update: Option<R>,
        actions: Vec<A>,
    ) -> Self {
        Self {
            state,
            resource_update,
            actions,
        }
    }

    /// Current convergence classification.
    #[must_use]
    pub const fn state(&self) -> ReconcileState {
        self.state
    }

    /// Resource carrying a new status, when persistence is necessary.
    #[must_use]
    pub const fn resource_update(&self) -> Option<&R> {
        self.resource_update.as_ref()
    }

    /// Consumes the plan and returns its optional resource update.
    #[must_use]
    pub fn into_resource_update(self) -> Option<R> {
        self.resource_update
    }

    /// Ordered side effects to execute with optimistic concurrency.
    #[must_use]
    pub fn actions(&self) -> &[A] {
        &self.actions
    }

    /// Returns whether neither persistence nor an external action is needed.
    #[must_use]
    pub fn is_noop(&self) -> bool {
        self.resource_update.is_none() && self.actions.is_empty()
    }
}
