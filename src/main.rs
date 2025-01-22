use std::{
    collections::{HashMap, HashSet, VecDeque},
    ops::Mul,
};

use chrono::{DateTime, FixedOffset};
use futures_util::{lock, FutureExt as _, StreamExt};

mod nix_prefetch;
mod sources;
use serde::{Deserialize, Serialize};

/// A full lock for an input, this includes the generic file lock (the nix hash and etag
/// we got from the HTTP request), and source specific lock.
#[derive(Serialize, Clone)]
struct FullLock {
    #[serde(flatten)]
    lock: Lock,
    #[serde(flatten)]
    inner: Box<dyn sources::Lock>,
}

impl FullLock {
    fn info(&self) -> impl std::fmt::Display + '_ {
        LockInfo(self)
    }
}

impl std::fmt::Debug for FullLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FullLock")
            .field("lock", &self.lock)
            .field("inner", self.inner.as_debug())
            .finish()
    }
}
struct LockInfo<'a>(&'a FullLock);
impl std::fmt::Display for LockInfo<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let last_modified = self.0.inner.last_modified().or(self.0.lock.last_modified);
        match (self.0.inner.version(), last_modified) {
            (Some(version), Some(last_modified)) => {
                write!(f, "{version} ({last_modified})")
            }
            (Some(version), None) => write!(f, "{version}"),
            (None, Some(last_modified)) => write!(f, "({last_modified})"),
            (None, None) => write!(f, "?"),
        }
    }
}

/// A generic lock for an input, this only includes the nix hash and etag.
#[derive(Deserialize, Serialize, Clone, Debug)]
struct Lock {
    hash: ssri2::Integrity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    etag: Option<String>,
    unpack: bool,

    // Below: unused for change detection, but used for displaying human-readable information about the change.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    last_modified: Option<DateTime<FixedOffset>>,
}
fn default_unpack() -> bool {
    true
}
#[derive(Debug, Serialize, Deserialize)]
struct Spec {
    spec: String,
    #[serde(default = "default_unpack")]
    unpack: bool,
}

async fn run_one(
    name: &str,
    pb: indicatif::ProgressBar,
    specv: toml::Value,
    lockv: Option<toml::Value>,
) -> anyhow::Result<sources::LockResult<FullLock>> {
    let spec = Spec::deserialize(specv.clone())?;
    let mut lock = sources::lock(spec.spec.clone(), specv, lockv.clone())
        .await
        .unwrap();
    let old_lock: Option<Lock> = lockv
        .as_ref()
        .and_then(|l| Lock::deserialize(l.clone()).ok());
    let url = lock.current().url(&spec.spec);

    log::debug!("{lock:?} {url}");
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
    let (hash, last_modified, etag) = if !lock.current().is_immutable() {
        let mut req = reqwest::Client::new().get(url.clone());
        if let Some(old_lock) = old_lock.as_ref() {
            if let Some(etag) = &old_lock.etag {
                req = req.header("if-none-match", etag);
            }
        }
        let res = req.send().await.unwrap();
        let mut etag = res
            .headers()
            .get("etag")
            .map(|v| v.to_str().unwrap().to_string());
        if res.status() == reqwest::StatusCode::NOT_MODIFIED {
            etag = old_lock.as_ref().and_then(|l| l.etag.clone());
        }
        let hash = hash.filter(|_| {
            old_lock
                .as_ref()
                .map(|l| l.etag == etag && etag.is_some())
                .unwrap_or_default()
        });
        if hash.is_none() {
            // etag changed, so if `lock` is `LockResult::Unchanged`, we need to update it.
            lock = lock.into_changed();
        }
        let last_modified = res
            .headers()
            .get("last-modified")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| DateTime::parse_from_rfc2822(v).ok());
        (hash, last_modified, etag)
    } else {
        // If the lock points to content that is immutable, we don't need etag to detect changes.
        (hash, None, None)
    };
    let hash = if let Some(hash) = hash {
        log::debug!("no changes");
        hash.to_owned()
    } else {
        let fetch = nix_prefetch::fetch(&url, spec.unpack, &pb).await?;
        fetch.hash
    };
    Ok(lock.map(
        |l_old| {
            old_lock.map(|l| FullLock {
                lock: l,
                inner: l_old,
            })
        },
        |l_curr| FullLock {
            lock: Lock {
                hash,
                etag,
                last_modified,
                unpack: spec.unpack,
            },
            inner: l_curr,
        },
    ))
}
struct MultiProgressWriter {
    mpb: indicatif::MultiProgress,
    buf: VecDeque<u8>,
}
impl std::io::Write for MultiProgressWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.buf.extend(buf.iter());
        while let Some(idx) = self.buf.iter().position(|&b| b == b'\n') {
            let line = self.buf.drain(..=idx).collect::<Vec<_>>();
            let line = std::str::from_utf8(&line)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            self.mpb.println(line)?;
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mpb = indicatif::MultiProgress::new();
    let pb = indicatif::ProgressBar::new_spinner();
    pb.enable_steady_tick(std::time::Duration::from_millis(100));
    mpb.add(pb.clone());
    pb.set_message("Checking...");
    pb.tick();
    mpb.println("TE$ST");
    mpb.println("TE$ST");
    env_logger::Builder::from_default_env()
        .write_style(env_logger::WriteStyle::Always)
        .target(env_logger::Target::Pipe(Box::new(MultiProgressWriter {
            mpb: mpb.clone(),
            buf: VecDeque::new(),
        })))
        .init();
    let specs: HashMap<String, toml::Value> = toml::from_str(&std::fs::read_to_string("F.toml")?)?;
    log::debug!("specs: {:?}", specs);
    let mut locks: HashMap<String, toml::Value> = std::fs::read_to_string("F.lock")
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
    let mut changed = HashMap::new();
    let mut unchanged = HashSet::new();
    for res in &results {
        match res {
            (name, Ok(lock)) => {
                log::debug!("fetched {name} {lock:?}");
                match &lock {
                    sources::LockResult::Changed(old, current) => {
                        changed.insert(name, (old.clone(), current.clone()));
                    }
                    sources::LockResult::Unchanged(_) => {
                        unchanged.insert(name);
                    }
                }
                locks.insert(
                    name.to_string(),
                    toml::Value::try_from(lock.clone().into_current()).unwrap(),
                );
            }
            (name, Err(e)) => {
                log::error!("failed to fetch {name} error: {:?}", e);
            }
        }
    }

    std::fs::write("F.lock", toml::to_string(&locks)?)?;
    pb.set_message("Done");
    pb.finish();
    log::info!("Lock file updated");
    if !unchanged.is_empty() {
        log::info!("Unchanged:");
        for name in unchanged {
            log::info!("  {name}");
        }
    }
    if !changed.is_empty() {
        log::info!("Changed:");
        for (name, (old, current)) in changed {
            if let Some(old) = old {
                log::info!("  {name} [{} -> {}]", old.info(), current.info());
            } else {
                log::info!("  {name} [∅ -> {}]", current.info());
            }
        }
    }

    Ok(())
}
