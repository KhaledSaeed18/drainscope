//! D-Bus contracts for the Sampler1 and Monitor1 interfaces.
//!
//! The introspection XML in `data/dbus/interfaces/` is authoritative; servers (in the bins)
//! are tested against it. This crate holds names, error types and client proxies only.

pub mod sampler {
    pub const BUS_NAME: &str = "io.github.khaledsaeed18.Drainscope.Sampler";
    pub const OBJECT_PATH: &str = "/io/github/khaledsaeed18/Drainscope/Sampler";
    pub const INTERFACE: &str = "io.github.khaledsaeed18.Drainscope.Sampler1";
    pub const POLKIT_ACTION: &str = "io.github.khaledsaeed18.Drainscope.read-energy";

    /// Errors returned by `Sampler1`, named `io.github.khaledsaeed18.Drainscope.Error.<Variant>`.
    #[derive(Debug, zbus::DBusError)]
    #[zbus(prefix = "io.github.khaledsaeed18.Drainscope.Error")]
    pub enum SamplerError {
        #[zbus(error)]
        ZBus(zbus::Error),
        /// polkit denied the caller.
        NotAuthorized(String),
        /// Called again within `MinIntervalMs`.
        RateLimited(String),
        /// No RAPL zones on this machine.
        Unsupported(String),
        /// Reading the counters failed.
        Failed(String),
    }

    /// One `ReadCounters` reply: (monotonic ns, generation, (domain, cumulative µJ)).
    pub type Counters = (u64, u64, Vec<(String, u64)>);

    #[zbus::proxy(
        interface = "io.github.khaledsaeed18.Drainscope.Sampler1",
        default_service = "io.github.khaledsaeed18.Drainscope.Sampler",
        default_path = "/io/github/khaledsaeed18/Drainscope/Sampler"
    )]
    pub trait Sampler1 {
        fn read_counters(&self) -> Result<Counters, SamplerError>;

        #[zbus(property)]
        fn domains(&self) -> zbus::Result<Vec<String>>;

        #[zbus(property)]
        fn min_interval_ms(&self) -> zbus::Result<u32>;

        #[zbus(property)]
        fn quantum_uj(&self) -> zbus::Result<u64>;
    }
}

pub mod monitor {
    pub const BUS_NAME: &str = "io.github.khaledsaeed18.Drainscope.Monitor";
    pub const OBJECT_PATH: &str = "/io/github/khaledsaeed18/Drainscope/Monitor";
    pub const INTERFACE: &str = "io.github.khaledsaeed18.Drainscope.Monitor1";

    /// `GetSummary` reply: (on battery, since unplug (Unix s), battery % used,
    /// (key, joules, % of battery, % of attributable energy)).
    pub type Summary = (bool, i64, f64, Vec<(String, f64, f64, f64)>);
    /// `GetUsage` row: (key, kind, total J, cpu J, gpu J, other J).
    pub type UsageRow = (String, String, f64, f64, f64, f64);
    /// `GetSleepSessions` row: (start, end (Unix s), Wh lost, % lost, sleep mode).
    pub type SleepRow = (i64, i64, f64, f64, String);

    #[zbus::proxy(
        interface = "io.github.khaledsaeed18.Drainscope.Monitor1",
        default_service = "io.github.khaledsaeed18.Drainscope.Monitor",
        default_path = "/io/github/khaledsaeed18/Drainscope/Monitor"
    )]
    pub trait Monitor1 {
        fn get_summary(&self) -> zbus::Result<Summary>;

        fn get_usage(
            &self,
            since: i64,
            until: i64,
            group_by: &str,
            power_source: &str,
        ) -> zbus::Result<Vec<UsageRow>>;

        fn get_coverage(&self, since: i64, until: i64, power_source: &str) -> zbus::Result<u64>;

        fn get_sleep_sessions(&self, since: i64) -> zbus::Result<Vec<SleepRow>>;

        /// (tick length in ms, (key, watts) for consumers active during the tick).
        #[zbus(signal)]
        fn tick(&self, duration_ms: u32, usage: Vec<(String, f64)>) -> zbus::Result<()>;

        #[zbus(property)]
        fn status(&self) -> zbus::Result<String>;

        #[zbus(property)]
        fn model_version(&self) -> zbus::Result<u32>;

        #[zbus(property)]
        fn domains(&self) -> zbus::Result<Vec<String>>;
    }
}
