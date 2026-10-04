//! Consumers: the things energy is attributed to.

use std::fmt;
use std::str::FromStr;

/// A stable identity for an energy consumer. Its string form (`Display`/`FromStr`) is the
/// storage key, so changing it is a data migration.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ConsumerKey {
    /// An application or D-Bus-activated service, by reverse-DNS id (`org.mozilla.firefox`).
    /// Display names and icons come from desktop entries, resolved by the UI.
    App(String),
    /// A terminal tab, labelled with the name of the process doing the work.
    Terminal(String),
    /// A logind session scope of the current user (ssh, tty), by session id.
    Session(String),
    /// GNOME Shell: compositing and the desktop itself.
    Shell,
    /// A system service or other system unit, by unit name.
    SystemUnit(String),
    /// A service of the user's own systemd manager, by unit name.
    UserUnit(String),
    /// A container or virtual machine, by short id or machine name.
    Container(String),
    /// Everything run by other non-root users, aggregated for privacy.
    OtherUsers,
    /// Everything run by root outside system services (e.g. `sudo` sessions).
    Root,
    /// Kernel threads and interrupt handling: CPU time not inside any child cgroup.
    Kernel,
    /// Processes that exited during an interval, by the slice they ran in.
    Exited(String),
    /// The machine's true-idle floor.
    Idle,
    /// Chipset and platform energy outside package and DRAM (needs a trustworthy `psys`).
    Platform,
    /// Display, Wi-Fi, storage and conversion losses: battery energy not seen by RAPL.
    Devices,
    /// drainscope itself, shown honestly like any other consumer.
    Drainscope,
}

impl fmt::Display for ConsumerKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::App(id) => write!(f, "app:{id}"),
            Self::Terminal(label) => write!(f, "term:{label}"),
            Self::Session(id) => write!(f, "session:{id}"),
            Self::Shell => f.write_str("shell"),
            Self::SystemUnit(unit) => write!(f, "unit:{unit}"),
            Self::UserUnit(unit) => write!(f, "user-unit:{unit}"),
            Self::Container(id) => write!(f, "container:{id}"),
            Self::OtherUsers => f.write_str("other-users"),
            Self::Root => f.write_str("root"),
            Self::Kernel => f.write_str("kernel"),
            Self::Exited(slice) => write!(f, "exited:{slice}"),
            Self::Idle => f.write_str("idle"),
            Self::Platform => f.write_str("platform"),
            Self::Devices => f.write_str("devices"),
            Self::Drainscope => f.write_str("drainscope"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseConsumerKeyError {
    #[error("unknown consumer key `{0}`")]
    Unknown(String),
    #[error("consumer key `{0}` has an empty name")]
    EmptyName(String),
}

impl FromStr for ConsumerKey {
    type Err = ParseConsumerKeyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let simple = match s {
            "shell" => Some(Self::Shell),
            "other-users" => Some(Self::OtherUsers),
            "root" => Some(Self::Root),
            "kernel" => Some(Self::Kernel),
            "idle" => Some(Self::Idle),
            "platform" => Some(Self::Platform),
            "devices" => Some(Self::Devices),
            "drainscope" => Some(Self::Drainscope),
            _ => None,
        };
        if let Some(key) = simple {
            return Ok(key);
        }

        let (kind, name) = s
            .split_once(':')
            .ok_or_else(|| ParseConsumerKeyError::Unknown(s.to_owned()))?;
        let constructor: fn(String) -> Self = match kind {
            "app" => Self::App,
            "term" => Self::Terminal,
            "session" => Self::Session,
            "unit" => Self::SystemUnit,
            "user-unit" => Self::UserUnit,
            "container" => Self::Container,
            "exited" => Self::Exited,
            _ => return Err(ParseConsumerKeyError::Unknown(s.to_owned())),
        };
        if name.is_empty() {
            return Err(ParseConsumerKeyError::EmptyName(s.to_owned()));
        }
        Ok(constructor(name.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn displays_storage_keys() {
        assert_eq!(
            ConsumerKey::App("org.mozilla.firefox".into()).to_string(),
            "app:org.mozilla.firefox"
        );
        assert_eq!(
            ConsumerKey::UserUnit("pipewire.service".into()).to_string(),
            "user-unit:pipewire.service"
        );
        assert_eq!(ConsumerKey::Kernel.to_string(), "kernel");
    }

    #[test]
    fn names_may_contain_colons() {
        let key = ConsumerKey::UserUnit("dbus-:1.2-org.example@0.service".into());
        assert_eq!(key.to_string().parse::<ConsumerKey>(), Ok(key));
    }

    #[test]
    fn rejects_unknown_and_empty_keys() {
        assert_eq!(
            "bogus".parse::<ConsumerKey>(),
            Err(ParseConsumerKeyError::Unknown("bogus".into()))
        );
        assert_eq!(
            "nope:x".parse::<ConsumerKey>(),
            Err(ParseConsumerKeyError::Unknown("nope:x".into()))
        );
        assert_eq!(
            "app:".parse::<ConsumerKey>(),
            Err(ParseConsumerKeyError::EmptyName("app:".into()))
        );
    }

    fn any_key() -> impl Strategy<Value = ConsumerKey> {
        let name = "[a-zA-Z0-9._@:-]{1,40}";
        prop_oneof![
            name.prop_map(ConsumerKey::App),
            name.prop_map(ConsumerKey::Terminal),
            name.prop_map(ConsumerKey::Session),
            name.prop_map(ConsumerKey::SystemUnit),
            name.prop_map(ConsumerKey::UserUnit),
            name.prop_map(ConsumerKey::Container),
            name.prop_map(ConsumerKey::Exited),
            Just(ConsumerKey::Shell),
            Just(ConsumerKey::OtherUsers),
            Just(ConsumerKey::Root),
            Just(ConsumerKey::Kernel),
            Just(ConsumerKey::Idle),
            Just(ConsumerKey::Platform),
            Just(ConsumerKey::Devices),
            Just(ConsumerKey::Drainscope),
        ]
    }

    proptest! {
        #[test]
        fn display_round_trips(key in any_key()) {
            prop_assert_eq!(key.to_string().parse::<ConsumerKey>(), Ok(key));
        }
    }
}
