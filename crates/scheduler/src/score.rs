use crate::SchedulerConfigError;

/// Normalized value between zero and 10,000 basis points.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BasisPoints(u16);

impl BasisPoints {
    /// Lowest possible normalized value.
    pub const ZERO: Self = Self(0);
    /// Highest possible normalized value.
    pub const MAX: Self = Self(10_000);

    /// Creates a checked normalized value.
    pub const fn new(value: u16) -> Option<Self> {
        if value <= Self::MAX.0 {
            Some(Self(value))
        } else {
            None
        }
    }

    /// Returns the raw basis-point value.
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

/// Configurable weights for the scheduler's five scoring dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScoreWeights {
    quality: BasisPoints,
    capability: BasisPoints,
    availability: BasisPoints,
    latency: BasisPoints,
    cost_efficiency: BasisPoints,
}

impl ScoreWeights {
    /// Creates weights that must add up to exactly 10,000 basis points.
    pub fn new(
        quality: BasisPoints,
        capability: BasisPoints,
        availability: BasisPoints,
        latency: BasisPoints,
        cost_efficiency: BasisPoints,
    ) -> Result<Self, SchedulerConfigError> {
        let total = u32::from(quality.get())
            + u32::from(capability.get())
            + u32::from(availability.get())
            + u32::from(latency.get())
            + u32::from(cost_efficiency.get());
        if total != 10_000 {
            return Err(SchedulerConfigError::InvalidWeightTotal(total));
        }
        Ok(Self {
            quality,
            capability,
            availability,
            latency,
            cost_efficiency,
        })
    }

    /// Quality weight.
    #[must_use]
    pub const fn quality(self) -> BasisPoints {
        self.quality
    }

    /// Capability and role-affinity weight.
    #[must_use]
    pub const fn capability(self) -> BasisPoints {
        self.capability
    }

    /// Free-capacity weight.
    #[must_use]
    pub const fn availability(self) -> BasisPoints {
        self.availability
    }

    /// Expected-latency weight.
    #[must_use]
    pub const fn latency(self) -> BasisPoints {
        self.latency
    }

    /// Expected-cost weight.
    #[must_use]
    pub const fn cost_efficiency(self) -> BasisPoints {
        self.cost_efficiency
    }
}

impl Default for ScoreWeights {
    fn default() -> Self {
        Self::new(
            BasisPoints::new(3_500).expect("default quality weight is valid"),
            BasisPoints::new(2_500).expect("default capability weight is valid"),
            BasisPoints::new(1_500).expect("default availability weight is valid"),
            BasisPoints::new(1_000).expect("default latency weight is valid"),
            BasisPoints::new(1_500).expect("default cost weight is valid"),
        )
        .expect("default scheduler weights total 10000")
    }
}

/// Auditable normalized components and final weighted score.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScoreBreakdown {
    quality: BasisPoints,
    capability: BasisPoints,
    availability: BasisPoints,
    latency: BasisPoints,
    cost_efficiency: BasisPoints,
    total: BasisPoints,
}

impl ScoreBreakdown {
    pub(crate) const fn new(
        quality: BasisPoints,
        capability: BasisPoints,
        availability: BasisPoints,
        latency: BasisPoints,
        cost_efficiency: BasisPoints,
        total: BasisPoints,
    ) -> Self {
        Self {
            quality,
            capability,
            availability,
            latency,
            cost_efficiency,
            total,
        }
    }

    /// Historical quality component.
    #[must_use]
    pub const fn quality(self) -> BasisPoints {
        self.quality
    }
    /// Capability and preferred-role component.
    #[must_use]
    pub const fn capability(self) -> BasisPoints {
        self.capability
    }
    /// Node free-capacity component.
    #[must_use]
    pub const fn availability(self) -> BasisPoints {
        self.availability
    }
    /// Relative expected-latency component.
    #[must_use]
    pub const fn latency(self) -> BasisPoints {
        self.latency
    }
    /// Relative expected-cost component.
    #[must_use]
    pub const fn cost_efficiency(self) -> BasisPoints {
        self.cost_efficiency
    }
    /// Final weighted score.
    #[must_use]
    pub const fn total(self) -> BasisPoints {
        self.total
    }
}
