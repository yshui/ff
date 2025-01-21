use std::{future::Future, pin::Pin};

use anyhow::Context as _;
use itertools::Itertools as _;
use serde::{de::DeserializeOwned, Serialize};

mod github;
pub use github::GitHub;
pub trait Lock: erased_serde::Serialize + std::fmt::Debug {
    fn url(&self, spec: &str) -> url::Url;
    /// Whether the content pointed to by the `url` is immutable.
    /// For example, GitHub commit tarball URLs are immutable, an ordinary URL probably isn't.
    fn is_immutable(&self) -> bool;
    fn as_dyn_serialize(&self) -> &dyn erased_serde::Serialize;
    fn as_debug(&self) -> &dyn std::fmt::Debug;
    fn as_any(&self) -> &dyn std::any::Any;
    fn dyn_eq(&self, other: &dyn Lock) -> bool;
}

pub trait Source {
    type Lock: Lock + DeserializeOwned;
    type RevisionSpec: Serialize + DeserializeOwned;
    type Error: std::fmt::Debug + Into<anyhow::Error>;

    fn lock(
        spec: String,
        rev_spec: Self::RevisionSpec,
        lock: Option<&Self::Lock>,
    ) -> Pin<Box<dyn Future<Output = Result<LockResult<Self::Lock>, Self::Error>> + 'static>>;
    fn schemes() -> &'static [&'static str];
}

macro_rules! try_sources {
    ($s:expr, $spec:expr, $rev:expr, $lock:expr, $($t:ty),+) => {
        $(
            if <$t as Source>::schemes().contains(&$s) {
                let rev_spec: <$t as Source>::RevisionSpec = serde::Deserialize::deserialize($rev).context("invalid rev spec")?;
                let lock: Option<<$t as Source>::Lock> = $lock.map(|l| serde::Deserialize::deserialize(l)).transpose().context("invalid lock")?;
                let lock = <$t as Source>::lock($spec.to_string(), rev_spec, lock.as_ref()).await?;
                return Ok(lock.as_dyn());
            }
        )+
    };
}

#[derive(Debug)]
pub struct LockResult<T> {
    is_changed: bool,
    inner: T,
}

impl<T: Lock + 'static> LockResult<T> {
    fn as_dyn(self) -> LockResult<Box<dyn Lock>> {
        LockResult {
            is_changed: self.is_changed,
            inner: Box::new(self.inner),
        }
    }
}

impl LockResult<Box<dyn Lock>> {
    pub fn url(&self, spec: &str) -> url::Url {
        self.inner.url(spec)
    }
    pub fn is_changed(&self) -> bool {
        self.is_changed
    }
    pub fn is_immutable(&self) -> bool {
        self.inner.is_immutable()
    }
    pub fn as_dyn_serialize(&self) -> &dyn erased_serde::Serialize {
        self.inner.as_dyn_serialize()
    }
}

pub async fn lock(
    spec: String,
    rev_spec: toml::Value,
    lock: Option<toml::Value>,
) -> anyhow::Result<LockResult<Box<dyn Lock>>> {
    let (scheme, spec) = spec.split_once(':').context("invalid spec")?;
    try_sources!(scheme, spec, rev_spec, lock, GitHub);
    Err(anyhow::anyhow!("unknown scheme"))
}

trait ResponseExt {
    type Ok;
    fn anyhow(self) -> Result<Self::Ok, anyhow::Error>;
}

impl<T> ResponseExt for graphql_client::Response<T> {
    type Ok = T;
    fn anyhow(self) -> Result<T, anyhow::Error> {
        if let Some(errors) = self.errors {
            let all_errors = errors.into_iter().map(|e| e.to_string()).join("\n");
            Err(anyhow::anyhow!("{all_errors}"))
        } else if let Some(data) = self.data {
            Ok(data)
        } else {
            Err(anyhow::anyhow!("no data or errors"))
        }
    }
}
