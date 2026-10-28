//! Registration mechanism.
//!
//! Contract reference: CONTRACT.md section 2.5 fixes the registration contract:
//! - the registry is an explicit ordered map (`IndexMap`), so traversal order is
//!   registration order;
//! - `register` returns a `Disposer`; dropping the disposer revokes the
//!   contribution (registration is a reversible effect);
//! - a duplicate key fails with `E-REG-001`, a required-but-missing key fails
//!   with `E-REG-002`.
//!
//! Implementation note: the disposer holds a weak handle to the shared registry
//! plus the registered key, so revocation is a real state change observed by all
//! readers of the same registry, not a per-clone local flag.

use std::sync::{Arc, Mutex, Weak};

use indexmap::IndexMap;

use crate::error::ContractError;

/// Error code: key registered twice.
pub const E_REG_001: &str = "E-REG-001";
/// Error code: key missing while the caller requires presence.
pub const E_REG_002: &str = "E-REG-002";

/// Domain tag used on registration errors.
pub const DOMAIN_REG: &str = "REG";

/// Shared ordered registry store.
///
/// Keys keep registration order; lookups are by key. The store is shared so a
/// disposer can revoke its contribution wherever the registry is read.
#[derive(Debug)]
pub struct OrderedRegistry<T> {
    entries: Mutex<IndexMap<String, Arc<T>>>,
    locus: &'static str,
}

/// Handle to a shared ordered registry.
///
/// Cloning the handle shares the same store. `register` produced disposers hold
/// a weak handle derived from this one, so a disposer cannot keep the registry
/// alive and revocation is a real state change for every holder.
#[derive(Debug)]
pub struct SharedRegistry<T> {
    inner: Arc<OrderedRegistry<T>>,
}

impl<T> Clone for SharedRegistry<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<T: Send + Sync + 'static> Default for SharedRegistry<T> {
    fn default() -> Self {
        Self::new("contracts/src/registry.rs")
    }
}

impl<T: Send + Sync + 'static> SharedRegistry<T> {
    /// Build an empty shared registry; `locus` names the producing site.
    pub fn new(locus: &'static str) -> Self {
        Self {
            inner: Arc::new(OrderedRegistry::new(locus)),
        }
    }

    /// Register a value under `key`, returning a disposer.
    ///
    /// Fails with `E-REG-001` when `key` is already present.
    pub fn register(&self, key: &str, value: T) -> Result<Disposer, ContractError> {
        let mut guard = self.inner.entries.lock().expect("registry mutex poisoned");
        if guard.contains_key(key) {
            return Err(ContractError::new(
                E_REG_001,
                DOMAIN_REG,
                format!("key already registered: {key}"),
                self.inner.locus,
            ));
        }
        guard.insert(key.to_string(), Arc::new(value));
        drop(guard);
        let erased_owner: Arc<dyn ErasedRevoke> = Arc::clone(&self.inner) as Arc<dyn ErasedRevoke>;
        Ok(Disposer::new(
            key.to_string(),
            Arc::downgrade(&erased_owner),
        ))
    }

    /// Look up a key, mapping the stored value.
    pub fn get<F, R>(&self, key: &str, f: F) -> Option<R>
    where
        F: FnOnce(&T) -> R,
    {
        let guard = self.inner.entries.lock().expect("registry mutex poisoned");
        guard.get(key).map(|value| f(value.as_ref()))
    }

    /// Require a key to exist; `E-REG-002` otherwise.
    pub fn require<F, R>(&self, key: &str, f: F) -> Result<R, ContractError>
    where
        F: FnOnce(&T) -> R,
    {
        let guard = self.inner.entries.lock().expect("registry mutex poisoned");
        match guard.get(key) {
            Some(value) => Ok(f(value.as_ref())),
            None => Err(ContractError::new(
                E_REG_002,
                DOMAIN_REG,
                format!("required key not registered: {key}"),
                self.inner.locus,
            )),
        }
    }

    /// Ordered key list, in registration order.
    pub fn keys(&self) -> Vec<String> {
        let guard = self.inner.entries.lock().expect("registry mutex poisoned");
        guard.keys().cloned().collect()
    }

    /// Number of registered entries.
    pub fn len(&self) -> usize {
        let guard = self.inner.entries.lock().expect("registry mutex poisoned");
        guard.len()
    }

    /// Whether the registry holds no entry.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Ordered snapshot of all entries, in registration order.
    pub fn entries_snapshot(&self) -> Vec<(String, Arc<T>)> {
        let guard = self.inner.entries.lock().expect("registry mutex poisoned");
        guard
            .iter()
            .map(|(key, value)| (key.clone(), Arc::clone(value)))
            .collect()
    }
}

impl<T> OrderedRegistry<T> {
    /// Build an empty registry; `locus` names the producing site.
    fn new(locus: &'static str) -> Self {
        Self {
            entries: Mutex::new(IndexMap::new()),
            locus,
        }
    }
}

/// Revokes a registration contribution when dropped.
#[derive(Debug)]
pub struct Disposer {
    key: Option<String>,
    registry: Weak<dyn ErasedRevoke>,
}

impl Disposer {
    fn new(key: String, registry: Weak<dyn ErasedRevoke>) -> Self {
        Self {
            key: Some(key),
            registry,
        }
    }

    /// The key this disposer revokes.
    pub fn key(&self) -> Option<&str> {
        self.key.as_deref()
    }

    /// Revoke explicitly; dropping afterwards is a no-op.
    pub fn revoke(mut self) {
        self.revoke_inner();
    }

    fn revoke_inner(&mut self) {
        if let Some(key) = self.key.take() {
            if let Some(registry) = self.registry.upgrade() {
                registry.erase(&key);
            }
        }
    }
}

impl Drop for Disposer {
    fn drop(&mut self) {
        self.revoke_inner();
    }
}

/// Type-erased revocation face used by `Disposer`.
pub trait ErasedRevoke: Send + Sync {
    /// Remove the entry registered under `key`.
    fn erase(&self, key: &str);
}

impl<T: Send + Sync + 'static> ErasedRevoke for OrderedRegistry<T> {
    fn erase(&self, key: &str) {
        let mut guard = self.entries.lock().expect("registry mutex poisoned");
        guard.shift_remove(key);
    }
}

/// Service definition of the registration seam (CONTRACT.md section 2.5).
pub trait RegistryPort<T>: Send + Sync {
    /// Register a value; the returned disposer revokes it on drop.
    fn register(&self, key: &str, value: T) -> Result<Disposer, ContractError>;
    /// Look up a key.
    fn get(&self, key: &str) -> Option<T>
    where
        T: Clone;
    /// Ordered snapshot of all entries, in registration order.
    fn list_ordered(&self) -> Vec<(String, T)>
    where
        T: Clone;
}
