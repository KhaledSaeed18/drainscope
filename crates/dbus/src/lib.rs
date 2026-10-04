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
}
