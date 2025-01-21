use std::collections::HashMap;

use anyhow::Context as _;
use futures_util::{FutureExt as _, StreamExt};

mod nix_prefetch;
mod sources;
use serde::{Deserialize, Serialize};

/// A full lock for an input, this includes the generic file lock (the nix hash and etag
/// we got from the HTTP request), and source specific lock.
#[derive(Serialize)]
struct FullLock<'a> {
    hash: ssri2::Integrity,
    #[serde(skip_serializing_if = "Option::is_none")]
    etag: Option<String>,
    #[serde(flatten)]
    inner: &'a dyn erased_serde::Serialize,
    unpack: bool,
}

/// A generic lock for an input, this only includes the nix hash and etag.
#[derive(Deserialize)]
struct Lock {
    hash: ssri2::Integrity,
    #[serde(default)]
    etag: Option<String>,
    unpack: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct Spec {
    spec: String,
    #[serde(default)]
    unpack: bool,
}

async fn run_one(
    name: &str,
    pb: indicatif::ProgressBar,
    specv: toml::Value,
    lockv: Option<toml::Value>,
) -> anyhow::Result<toml::Value> {
    let spec = Spec::deserialize(specv.clone())?;
    let lock = sources::lock(spec.spec.clone(), specv, lockv.clone())
        .await
        .unwrap();
    let old_lock: Option<Lock> = lockv
        .as_ref()
        .and_then(|l| Lock::deserialize(l.clone()).ok());
    let (_, inner_spec) = spec.spec.split_once(':').context("invalid spec")?;
    let url = lock.url(inner_spec);

    eprintln!("{lock:?} {url}");
    let old_hash = old_lock.as_ref().map(|l| &l.hash);
    // We can keep using the old hash in current lock if these conditions are met:
    //   1. The `unpack` flag is unchanged in the spec.
    //   2. Lock returned by sources is unchanged.
    //   3. If the lock is mutable, then the etag returned by the server is also unchanged.
    let hash = old_hash.filter(|_| {
        old_lock
            .as_ref()
            .map(|l| l.unpack == spec.unpack)
            .unwrap_or_default()
            && !lock.is_changed()
    });
    let (hash, etag) = if !lock.is_immutable() {
        let res = reqwest::Client::new()
            .get(url.clone())
            .send()
            .await
            .unwrap();
        let etag = res
            .headers()
            .get("etag")
            .map(|v| v.to_str().unwrap().to_string());
        let hash = hash.filter(|_| {
            old_lock
                .as_ref()
                .map(|l| l.etag == etag && etag.is_some())
                .unwrap_or_default()
        });
        (hash, etag)
    } else {
        // If the lock points to content that is immutable, we don't need etag to detect changes.
        (hash, None)
    };
    let hash = if let Some(hash) = hash {
        eprintln!("no changes");
        hash.to_owned()
    } else {
        let fetch = nix_prefetch::fetch(&url, spec.unpack, &pb).await?;
        fetch.hash
    };
    Ok(toml::Value::try_from(FullLock {
        hash,
        etag,
        unpack: spec.unpack,
        inner: lock.as_dyn_serialize(),
    })
    .unwrap())
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();
    let mpb = indicatif::MultiProgress::new();
    let specs: HashMap<String, toml::Value> =
        toml::from_str(&std::fs::read_to_string("specs.toml")?)?;
    let mut locks: HashMap<String, toml::Value> = std::fs::read_to_string("locks.toml")
        .map_err(anyhow::Error::from)
        .and_then(|s| toml::from_str(&s).map_err(anyhow::Error::from))
        .unwrap_or_default();
    let futs: futures_util::stream::FuturesUnordered<_> = specs
        .iter()
        .map(|(name, spec)| {
            let lock = locks.get(name).cloned();
            let pb = mpb.add(indicatif::ProgressBar::new_spinner());
            run_one(name, pb, spec.clone(), lock).map(move |res| (name.clone(), res))
        })
        .collect();
    let results = futs.collect::<Vec<_>>().await;
    for res in results {
        match res {
            (name, Ok(prefetch)) => {
                println!("fetched {name} prefetch: {:?}", prefetch);
                locks.insert(name, prefetch);
            }
            (name, Err(e)) => {
                eprintln!("failed to fetch {name} error: {:?}", e);
            }
        }
    }

    std::fs::write("locks.toml", toml::to_string(&locks)?)?;

    Ok(())
}
