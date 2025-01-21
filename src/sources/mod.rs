use std::{future::Future, pin::Pin};

use anyhow::Context as _;
use itertools::Itertools as _;
use serde::{de::DeserializeOwned, Serialize};

mod github;
use github::GitHub;
use http::Http;
pub trait Lock: erased_serde::Serialize + std::fmt::Debug {
    fn url(&self, spec: &str) -> url::Url;
    /// Whether the content pointed to by the `url` is immutable.
    /// For example, GitHub commit tarball URLs are immutable, an ordinary URL probably isn't.
    fn is_immutable(&self) -> bool;
    fn as_dyn_serialize(&self) -> &dyn erased_serde::Serialize;
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
            Box::pin(async {
                Ok(super::LockResult {
                    is_changed: false,
                    inner: Lock,
                })
            })
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

#[derive(Debug)]
pub struct LockResult<T> {
    is_changed: bool,
    inner: T,
}

impl<T: Lock + 'static> LockResult<T> {
    fn into_dyn(self) -> LockResult<Box<dyn Lock>> {
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
    let (scheme, _) = spec.split_once(':').context("invalid spec")?;
    try_sources!(scheme, spec, rev_spec, lock, GitHub, Http);
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
