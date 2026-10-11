//! Deciding who may call a privileged service: a polkit action that allows active local
//! sessions only (e.g. `io.github.khaledsaeed18.Drainscope.read-energy`).

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use zbus::zvariant::Value;

pub type AuthFuture<'a> = Pin<Box<dyn Future<Output = zbus::Result<bool>> + Send + 'a>>;

pub trait Authorizer: Send + Sync {
    /// Whether `caller` (a unique bus name; `None` on peer-to-peer connections) may read.
    fn authorize<'a>(&'a self, caller: Option<&'a str>) -> AuthFuture<'a>;
}

/// The same answer for everyone. For tests only: the service always uses [`Polkit`].
#[derive(Debug, Clone, Copy)]
pub struct Fixed(pub bool);

impl Authorizer for Fixed {
    fn authorize<'a>(&'a self, _caller: Option<&'a str>) -> AuthFuture<'a> {
        let allowed = self.0;
        Box::pin(async move { Ok(allowed) })
    }
}

#[zbus::proxy(
    interface = "org.freedesktop.PolicyKit1.Authority",
    default_service = "org.freedesktop.PolicyKit1",
    default_path = "/org/freedesktop/PolicyKit1/Authority"
)]
trait Authority {
    fn check_authorization(
        &self,
        subject: &(&str, HashMap<&str, Value<'_>>),
        action_id: &str,
        details: HashMap<&str, &str>,
        flags: u32,
        cancellation_id: &str,
    ) -> zbus::Result<(bool, bool, HashMap<String, String>)>;
}

/// How long a polkit "yes" is reused for one bus name before asking again: a session that
/// becomes inactive (e.g. after switching users) loses access within this time.
pub const APPROVAL_TTL: Duration = Duration::from_secs(60);

const MAX_CACHED: usize = 256;

/// Polkit approvals by unique bus name, each with when it was given.
#[derive(Debug, Default)]
pub struct Approvals {
    given: HashMap<String, Instant>,
}

impl Approvals {
    /// Whether `caller` was approved less than [`APPROVAL_TTL`] before `now`.
    #[must_use]
    pub fn fresh(&self, caller: &str, now: Instant) -> bool {
        self.given
            .get(caller)
            .is_some_and(|at| now.saturating_duration_since(*at) < APPROVAL_TTL)
    }

    /// Records an approval at `now`. When [`MAX_CACHED`] names are held, expired ones are
    /// dropped first, then all of them if still full.
    pub fn insert(&mut self, caller: &str, now: Instant) {
        if self.given.len() >= MAX_CACHED && !self.given.contains_key(caller) {
            self.given
                .retain(|_, at| now.saturating_duration_since(*at) < APPROVAL_TTL);
            if self.given.len() >= MAX_CACHED {
                self.given.clear();
            }
        }
        self.given.insert(caller.to_owned(), now);
    }
}

/// Asks polkit, without interactive authentication. Positive answers are reused for
/// [`APPROVAL_TTL`] per unique bus name, which is never reused on a bus; denials are always
/// re-checked so a session becoming active is picked up.
#[derive(Debug)]
pub struct Polkit {
    authority: AuthorityProxy<'static>,
    action: &'static str,
    approvals: Mutex<Approvals>,
}

impl Polkit {
    /// # Errors
    /// If the polkit proxy can't be created.
    pub async fn new(system_bus: &zbus::Connection, action: &'static str) -> zbus::Result<Self> {
        Ok(Self {
            authority: AuthorityProxy::new(system_bus).await?,
            action,
            approvals: Mutex::new(Approvals::default()),
        })
    }

    async fn check(&self, caller: &str) -> zbus::Result<bool> {
        let cached = self
            .approvals
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .fresh(caller, Instant::now());
        if cached {
            return Ok(true);
        }
        let subject = (
            "system-bus-name",
            HashMap::from([("name", Value::from(caller))]),
        );
        let (authorized, _challenge, _details) = self
            .authority
            .check_authorization(&subject, self.action, HashMap::new(), 0, "")
            .await?;
        if authorized {
            self.approvals
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(caller, Instant::now());
        }
        Ok(authorized)
    }
}

impl Authorizer for Polkit {
    fn authorize<'a>(&'a self, caller: Option<&'a str>) -> AuthFuture<'a> {
        Box::pin(async move {
            match caller {
                Some(caller) => self.check(caller).await,
                None => Ok(false),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_is_fresh_after_insert() {
        let mut approvals = Approvals::default();
        let now = Instant::now();
        approvals.insert(":1.5", now);
        assert!(approvals.fresh(":1.5", now));
        assert!(!approvals.fresh(":1.6", now));
    }

    #[test]
    fn approval_expires_after_ttl() {
        let mut approvals = Approvals::default();
        let now = Instant::now();
        approvals.insert(":1.5", now);
        assert!(approvals.fresh(":1.5", now + Duration::from_secs(59)));
        assert!(!approvals.fresh(":1.5", now + APPROVAL_TTL));
    }

    #[test]
    fn approvals_are_bounded() {
        let mut approvals = Approvals::default();
        let now = Instant::now();
        for i in 0..=MAX_CACHED {
            approvals.insert(&format!(":1.{i}"), now);
        }
        assert!(approvals.given.len() <= MAX_CACHED);
        assert!(approvals.fresh(&format!(":1.{MAX_CACHED}"), now));
    }
}
