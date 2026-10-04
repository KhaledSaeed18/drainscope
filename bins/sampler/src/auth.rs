//! Deciding who may read the counters: polkit action
//! `io.github.khaledsaeed18.Drainscope.read-energy` (active local sessions only).

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Mutex, PoisonError};

use drainscope_dbus::sampler::POLKIT_ACTION;
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

/// Asks polkit, without interactive authentication. Positive answers are cached per unique
/// bus name, which is never reused on a bus; denials are re-checked so a session becoming
/// active is picked up.
#[derive(Debug)]
pub struct Polkit {
    authority: AuthorityProxy<'static>,
    allowed: Mutex<HashSet<String>>,
}

const MAX_CACHED: usize = 256;

impl Polkit {
    /// # Errors
    /// If the polkit proxy can't be created.
    pub async fn new(system_bus: &zbus::Connection) -> zbus::Result<Self> {
        Ok(Self {
            authority: AuthorityProxy::new(system_bus).await?,
            allowed: Mutex::new(HashSet::new()),
        })
    }

    async fn check(&self, caller: &str) -> zbus::Result<bool> {
        let cached = self
            .allowed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(caller);
        if cached {
            return Ok(true);
        }
        let subject = (
            "system-bus-name",
            HashMap::from([("name", Value::from(caller))]),
        );
        let (authorized, _challenge, _details) = self
            .authority
            .check_authorization(&subject, POLKIT_ACTION, HashMap::new(), 0, "")
            .await?;
        if authorized {
            let mut allowed = self.allowed.lock().unwrap_or_else(PoisonError::into_inner);
            if allowed.len() >= MAX_CACHED {
                allowed.clear();
            }
            allowed.insert(caller.to_owned());
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
