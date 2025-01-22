use std::{future::Future, pin::Pin};

use anyhow::Context as _;
use chrono::{DateTime, FixedOffset};
use itertools::Itertools as _;
use serde::de::DeserializeOwned;

mod github;
pub trait Lock: erased_serde::Serialize + std::fmt::Debug {
    fn url(&self, spec: &str) -> url::Url;
    /// Whether the content pointed to by the `url` is immutable.
    /// For example, GitHub commit tarball URLs are immutable, an ordinary URL probably isn't.
    fn is_immutable(&self) -> bool;
    fn version(&self) -> Option<String>;
    fn last_modified(&self) -> Option<DateTime<FixedOffset>>;
    fn as_dyn_serialize(&self) -> &dyn erased_serde::Serialize;
    fn as_debug(&self) -> &dyn std::fmt::Debug;
    fn dyn_clone(&self) -> Box<dyn Lock>;
}

impl Clone for Box<dyn Lock> {
    fn clone(&self) -> Self {
        self.dyn_clone()
    }
}

impl serde::Serialize for Box<dyn Lock> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.as_dyn_serialize().serialize(serializer)
    }
}

pub trait Source {
    type Lock: Lock + DeserializeOwned;
    type RevisionSpec: DeserializeOwned;
    type Error: std::fmt::Debug + Into<anyhow::Error>;

    fn lock(
        spec: String,
        rev_spec: Self::RevisionSpec,
        lock: Option<&Self::Lock>,
    ) -> Pin<Box<dyn Future<Output = Result<LockResult<Self::Lock>, Self::Error>> + 'static>>;
    fn schemes() -> &'static [&'static str];
}

mod http {
    use chrono::{DateTime, FixedOffset};
    use serde::{Deserialize, Serialize};
    use std::pin::Pin;

    pub struct Http;
    pub struct Spec;
    impl<'d> Deserialize<'d> for Spec {
        fn deserialize<D>(_: D) -> Result<Self, D::Error>
        where
            D: serde::Deserializer<'d>,
        {
            Ok(Spec)
        }
    }
    #[derive(Debug, Serialize)]
    pub struct Lock;
    impl<'d> Deserialize<'d> for Lock {
        fn deserialize<D>(_: D) -> Result<Self, D::Error>
        where
            D: serde::Deserializer<'d>,
        {
            Ok(Lock)
        }
    }
    impl super::Source for Http {
        type Lock = Lock;
        type RevisionSpec = Spec;
        type Error = anyhow::Error;

        fn lock(
            _spec: String,
            _rev_spec: Self::RevisionSpec,
            _lock: Option<&Self::Lock>,
        ) -> Pin<
            Box<
                dyn std::future::Future<Output = Result<super::LockResult<Self::Lock>, Self::Error>>
                    + 'static,
            >,
        > {
            Box::pin(async { Ok(super::LockResult::Unchanged(Lock)) })
        }

        fn schemes() -> &'static [&'static str] {
            &["http", "https"]
        }
    }
    impl super::Lock for Lock {
        fn url(&self, spec: &str) -> url::Url {
            spec.parse().unwrap()
        }
        fn is_immutable(&self) -> bool {
            false
        }
        fn as_dyn_serialize(&self) -> &dyn erased_serde::Serialize {
            self
        }
        fn version(&self) -> Option<String> {
            None
        }
        fn last_modified(&self) -> Option<DateTime<FixedOffset>> {
            None
        }
        fn as_debug(&self) -> &dyn std::fmt::Debug {
            self
        }
        fn dyn_clone(&self) -> Box<dyn super::Lock> {
            Box::new(Lock)
        }
    }
}

macro_rules! try_sources {
    ($s:expr, $spec:expr, $rev:expr, $lock:expr, $($t:ty),+) => {
        $(
            if <$t as Source>::schemes().contains(&$s) {
                let rev_spec: <$t as Source>::RevisionSpec = serde::Deserialize::deserialize($rev).context("invalid rev spec")?;
                let lock: Option<<$t as Source>::Lock> = $lock.map(|l| serde::Deserialize::deserialize(l)).transpose().context("invalid lock")?;
                let lock = <$t as Source>::lock($spec.to_string(), rev_spec, lock.as_ref()).await?;
                return Ok(lock.into_dyn());
            }
        )+
    };
}

#[derive(Debug, Clone)]
pub enum LockResult<T> {
    Unchanged(T),
    Changed(Option<T>, T),
}

impl<T: Clone> LockResult<T> {
    pub fn into_changed(self) -> Self {
        match self {
            LockResult::Unchanged(inner) => LockResult::Changed(Some(inner.clone()), inner),
            LockResult::Changed(_, _) => self,
        }
    }
}

impl<T> LockResult<T> {
    pub fn map<S>(
        self,
        f_old: impl FnOnce(T) -> Option<S>,
        f_current: impl FnOnce(T) -> S,
    ) -> LockResult<S> {
        match self {
            LockResult::Unchanged(inner) => LockResult::Unchanged(f_current(inner)),
            LockResult::Changed(old, new) => LockResult::Changed(old.and_then(f_old), f_current(new)),
        }
    }
    pub fn current(&self) -> &T {
        match self {
            LockResult::Unchanged(inner) => inner,
            LockResult::Changed(_, new) => new,
        }
    }
    pub fn old(&self) -> Option<&T> {
        match self {
            LockResult::Unchanged(_) => None,
            LockResult::Changed(old, _) => old.as_ref(),
        }
    }
    pub fn into_current(self) -> T {
        match self {
            LockResult::Unchanged(inner) => inner,
            LockResult::Changed(_, new) => new,
        }
    }
    pub fn is_changed(&self) -> bool {
        matches!(self, LockResult::Changed(_, _))
    }
}

impl<T: Lock + 'static> LockResult<T> {
    fn into_dyn(self) -> LockResult<Box<dyn Lock>> {
        match self {
            LockResult::Unchanged(inner) => LockResult::Unchanged(Box::new(inner)),
            LockResult::Changed(old, new) => {
                LockResult::Changed(old.map(|old| Box::new(old) as Box<dyn Lock>), Box::new(new))
            }
        }
    }
}

pub async fn lock(
    spec: String,
    rev_spec: toml::Value,
    lock: Option<toml::Value>,
) -> anyhow::Result<LockResult<Box<dyn Lock>>> {
    let (scheme, _) = spec.split_once(':').context("invalid spec")?;
    try_sources!(scheme, spec, rev_spec, lock, github::GitHub, http::Http);
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
