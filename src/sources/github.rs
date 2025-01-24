use std::{collections::HashMap, pin::Pin};

use super::{DynLockFuture, LockResult, ResponseExt as _};
use anyhow::{Context as _, Ok};
use chrono::FixedOffset;
use graphql_client::GraphQLQuery;
use serde::{Deserialize, Serialize};
#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "kebab-case", untagged)]
pub enum Spec {
    /// git commit hash
    Rev { rev: String },
    /// a tag name or a glob pattern to match tags
    Tag { tag: String },
    /// git branch name
    Branch {
        #[serde(default)]
        branch: Option<String>,
    },
    Release {
        /// Whether to include pre-release versions
        pre_release: bool,
    },
}

impl Default for Spec {
    fn default() -> Self {
        Self::Release { pre_release: false }
    }
}

#[serde_with::serde_as]
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Lock {
    rev: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    tag: Option<String>,
    commit_date: DateTime,
}

impl super::Lock for Lock {
    fn url(&self, spec: &str) -> url::Url {
        let spec = spec.strip_prefix("github:").unwrap();
        let (owner, repo) = spec.split_once('/').unwrap();
        url::Url::parse(&format!(
            "https://github.com/{}/{}/archive/{}.tar.gz",
            owner, repo, self.rev
        ))
        .unwrap()
    }
    fn is_immutable(&self) -> bool {
        true
    }
    fn as_dyn_serialize(&self) -> &dyn erased_serde::Serialize {
        self
    }
    fn version(&self) -> Option<String> {
        self.tag.clone()
    }
    fn last_modified(&self) -> Option<DateTime> {
        Some(self.commit_date)
    }
    fn as_debug(&self) -> &dyn std::fmt::Debug {
        self
    }
    fn dyn_clone(&self) -> Box<dyn super::Lock> {
        Box::new(self.clone())
    }
}

pub struct GitHub;
type GitObjectID = String;
type DateTime = chrono::DateTime<FixedOffset>;
#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "github.schema.graphql",
    query_path = "github.query.graphql",
    variables_derives = "Debug",
    response_derives = "Debug"
)]
struct ListTags;
#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "github.schema.graphql",
    query_path = "github.query.graphql",
    variables_derives = "Debug",
    response_derives = "Debug"
)]
struct RefInfo;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "github.schema.graphql",
    query_path = "github.query.graphql",
    variables_derives = "Debug",
    response_derives = "Debug"
)]
struct ListReleases;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "github.schema.graphql",
    query_path = "github.query.graphql",
    variables_derives = "Debug",
    response_derives = "Debug"
)]
struct DefaultBranch;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "github.schema.graphql",
    query_path = "github.query.graphql",
    variables_derives = "Debug",
    response_derives = "Debug"
)]
struct ObjInfo;

#[derive(Serialize, Deserialize, Debug)]
struct AccessTokens {
    value: HashMap<String, String>,
}

impl ref_info::RefInfoRepositoryRefTarget {
    fn oid(&self) -> Option<&GitObjectID> {
        match &self.on {
            ref_info::RefInfoRepositoryRefTargetOn::Tag(tag) => Some(&tag.target.oid),
            ref_info::RefInfoRepositoryRefTargetOn::Commit(_) => Some(&self.oid),
            _ => None,
        }
    }
    fn commit_date(&self) -> Option<DateTime> {
        match &self.on {
            ref_info::RefInfoRepositoryRefTargetOn::Tag(
                ref_info::RefInfoRepositoryRefTargetOnTag {
                    target:
                        ref_info::RefInfoRepositoryRefTargetOnTagTarget {
                            on: ref_info::RefInfoRepositoryRefTargetOnTagTargetOn::Commit(commit),
                            ..
                        },
                },
            ) => Some(commit.committed_date),
            ref_info::RefInfoRepositoryRefTargetOn::Commit(commit) => Some(commit.committed_date),
            _ => None,
        }
    }
}

impl list_tags::ListTagsRepositoryRefsNodesTarget {
    fn oid(&self) -> Option<&GitObjectID> {
        match &self.on {
            list_tags::ListTagsRepositoryRefsNodesTargetOn::Tag(tag) => Some(&tag.target.oid),
            list_tags::ListTagsRepositoryRefsNodesTargetOn::Commit(_) => Some(&self.oid),
            _ => None,
        }
    }
    fn commit_date(&self) -> Option<DateTime> {
        match &self.on {
            list_tags::ListTagsRepositoryRefsNodesTargetOn::Tag(
                list_tags::ListTagsRepositoryRefsNodesTargetOnTag {
                    target:
                        list_tags::ListTagsRepositoryRefsNodesTargetOnTagTarget {
                            on:
                                list_tags::ListTagsRepositoryRefsNodesTargetOnTagTargetOn::Commit(
                                    commit,
                                ),
                            ..
                        },
                },
            ) => Some(commit.committed_date),
            list_tags::ListTagsRepositoryRefsNodesTargetOn::Commit(commit) => {
                Some(commit.committed_date)
            }
            _ => None,
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "kebab-case")]
struct NixConfig {
    access_tokens: AccessTokens,
}
impl super::Source for GitHub {
    type Lock = Lock;
    type Error = anyhow::Error;
    type RevisionSpec = Spec;
    fn schemes() -> &'static [&'static str] {
        &["github"]
    }
    fn lock<'a>(
        spec: &'a str,
        rev_spec: Self::RevisionSpec,
        pb: &'a indicatif::ProgressBar,
        lock: Option<&'a Self::Lock>,
    ) -> Pin<Box<DynLockFuture<'a, Lock, anyhow::Error>>> {
        let spec = spec.strip_prefix("github:").unwrap().to_owned();
        let lock = lock.cloned();
        Box::pin(async move {
            let nix_config: NixConfig = serde_json::from_slice(
                &std::process::Command::new("nix")
                    .args(["config", "show", "--json"])
                    .output()?
                    .stdout,
            )?;
            let github_token = nix_config.access_tokens.value.get("github.com");
            let octocrab = octocrab::Octocrab::builder();
            let octocrab = if let Some(github_token) = github_token {
                octocrab.personal_token(github_token.as_str())
            } else {
                octocrab
            };
            let octocrab = octocrab.build()?;
            let (owner, repo) = spec.split_once('/').unwrap();
            match rev_spec {
                Spec::Rev { rev } => {
                    let not_changed = lock.as_ref().map(|l| l.rev == *rev).unwrap_or(false);
                    let commit_date = if not_changed {
                        lock.as_ref().unwrap().commit_date
                    } else {
                        pb.set_message(format!("fetching information for commit {rev}"));
                        let query = ObjInfo::build_query(obj_info::Variables {
                            owner: owner.to_string(),
                            repo: repo.to_string(),
                            oid: rev.clone(),
                        });
                        let response: octocrab::Result<
                            graphql_client::Response<obj_info::ResponseData>,
                        > = octocrab.graphql(&query).await;
                        let response = response?.anyhow()?;
                        let obj = response
                            .repository
                            .context("not found")?
                            .object
                            .context("object not found")?;
                        let obj_info::ObjInfoRepositoryObject::Commit(commit) = obj else {
                            return Err(anyhow::anyhow!("specified rev is not a commit"));
                        };
                        commit.committed_date
                    };
                    Ok(if not_changed {
                        LockResult::Unchanged(lock.unwrap())
                    } else {
                        LockResult::Changed(
                            lock,
                            Lock {
                                rev,
                                tag: None,
                                commit_date,
                            },
                        )
                    })
                }
                Spec::Tag { tag } => {
                    pb.set_message(format!("checking if {tag} exists"));
                    let query = RefInfo::build_query(ref_info::Variables {
                        owner: owner.to_string(),
                        repo: repo.to_string(),
                        q_tag: format!("refs/tags/{tag}"),
                    });
                    let response: octocrab::Result<
                        graphql_client::Response<ref_info::ResponseData>,
                    > = octocrab.graphql(&query).await;
                    let data = response?.anyhow()?;
                    log::debug!("{:?}", data);
                    let new_lock =
                        if let Some(ref_) = data.repository.context("no repository")?.ref_ {
                            let target = ref_.target.context("no target")?;
                            let Some(oid) = target.oid() else {
                                return Err(anyhow::anyhow!("not a tag {target:?}"));
                            };
                            log::debug!("found direct match: {tag} => {oid}");
                            Lock {
                                rev: oid.to_owned(),
                                tag: Some(tag),
                                commit_date: target.commit_date().context("tag not a commit")?,
                            }
                        } else {
                            pb.set_message(format!("searching for latest tag matching \"{tag}\""));
                            let mut cursor = None;
                            let glob = glob::Pattern::new(&tag).context("invalid glob pattern")?;
                            'find_tag: loop {
                                let query = ListTags::build_query(list_tags::Variables {
                                    owner: owner.to_string(),
                                    repo: repo.to_string(),
                                    after: cursor,
                                });
                                let response: octocrab::Result<
                                    graphql_client::Response<list_tags::ResponseData>,
                                > = octocrab.graphql(&query).await;
                                let data = response?.anyhow()?;

                                let refs = data
                                    .repository
                                    .context("no repository")?
                                    .refs
                                    .context("no refs")?;
                                if !refs.page_info.has_next_page {
                                    return Err(anyhow::anyhow!("no matching tag found"));
                                }
                                cursor = refs.page_info.end_cursor.clone();
                                let Some(nodes) = refs.nodes else { continue };
                                for tag in nodes {
                                    let Some(tag) = tag else { continue };
                                    if !glob.matches(&tag.name) {
                                        continue;
                                    }
                                    log::debug!("found glob match: {}", tag.name);
                                    let target = tag.target.context("no target")?;
                                    let Some(oid) = target.oid() else {
                                        continue;
                                    };
                                    break 'find_tag Lock {
                                        rev: oid.to_owned(),
                                        tag: Some(tag.name),
                                        commit_date: target
                                            .commit_date()
                                            .context("tag not a commit")?,
                                    };
                                }
                            }
                        };
                    Ok(
                        if lock
                            .as_ref()
                            .map(|l| l.rev == new_lock.rev)
                            .unwrap_or(false)
                        {
                            LockResult::Unchanged(lock.unwrap())
                        } else {
                            LockResult::Changed(lock, new_lock)
                        },
                    )
                }
                Spec::Branch {
                    branch: Some(branch),
                } => {
                    pb.set_message(format!("getting information for branch {branch}"));
                    let query = RefInfo::build_query(ref_info::Variables {
                        owner: owner.to_string(),
                        repo: repo.to_string(),
                        q_tag: format!("refs/heads/{branch}"),
                    });
                    let response: octocrab::Result<
                        graphql_client::Response<ref_info::ResponseData>,
                    > = octocrab.graphql(&query).await;
                    let data = response?.anyhow()?;
                    log::debug!("{:?}", data);
                    let ref_ = data
                        .repository
                        .context("no repository")?
                        .ref_
                        .context("branch not found")?;
                    let target = ref_.target.context("no target")?;
                    let Some(oid) = target.oid() else {
                        return Err(anyhow::anyhow!("no target commit: {target:?}"));
                    };
                    log::debug!("found direct match: {branch} => {oid}");
                    Ok(if lock.as_ref().map(|l| l.rev == *oid).unwrap_or(false) {
                        LockResult::Unchanged(lock.unwrap())
                    } else {
                        LockResult::Changed(
                            lock,
                            Lock {
                                rev: oid.to_owned(),
                                tag: None,
                                commit_date: target.commit_date().context("tag not a commit")?,
                            },
                        )
                    })
                }
                Spec::Branch { branch: None } => {
                    pb.set_message(format!("getting information for default branch"));
                    let query = DefaultBranch::build_query(default_branch::Variables {
                        owner: owner.to_string(),
                        repo: repo.to_string(),
                    });
                    let response: octocrab::Result<
                        graphql_client::Response<default_branch::ResponseData>,
                    > = octocrab.graphql(&query).await;
                    let data = response?.anyhow()?;
                    let default_branch = data
                        .repository
                        .context("no repository")?
                        .default_branch_ref
                        .context("no default branch")?;
                    let target = default_branch.target.context("no target")?;
                    let default_branch::DefaultBranchRepositoryDefaultBranchRefTargetOn::Commit(
                        commit,
                    ) = target.on
                    else {
                        return Err(anyhow::anyhow!("default branch is not a commit"));
                    };
                    log::debug!(
                        "found direct match: {} => {}",
                        default_branch.name,
                        target.oid
                    );
                    Ok(
                        if lock.as_ref().map(|l| l.rev == target.oid).unwrap_or(false) {
                            LockResult::Unchanged(lock.unwrap())
                        } else {
                            LockResult::Changed(
                                lock,
                                Lock {
                                    rev: target.oid,
                                    tag: None,
                                    commit_date: commit.committed_date,
                                },
                            )
                        },
                    )
                }
                Spec::Release { pre_release } => {
                    let mut cursor = None;
                    Ok(loop {
                        let query = ListReleases::build_query(list_releases::Variables {
                            owner: owner.to_string(),
                            repo: repo.to_string(),
                            after: cursor,
                        });
                        let response: octocrab::Result<
                            graphql_client::Response<list_releases::ResponseData>,
                        > = octocrab.graphql(&query).await;
                        let data = response?.anyhow()?;
                        let releases = data.repository.context("no repository")?.releases;
                        let has_next_page = releases.page_info.has_next_page;
                        cursor = releases.page_info.end_cursor.clone();
                        let Some(releases) = releases.nodes else {
                            return Err(anyhow::anyhow!("no release found"));
                        };
                        let release = releases.into_iter().flatten().find(|release| {
                            if pre_release {
                                true
                            } else {
                                !release.is_prerelease
                            }
                        });
                        let Some(release) = release else {
                            if has_next_page {
                                continue;
                            } else {
                                return Err(anyhow::anyhow!("no release found"));
                            }
                        };

                        let tag = release.tag.context("release is not a tag??")?;
                        let tag_commit = release.tag_commit.context("tag has no commit??")?;
                        if lock
                            .as_ref()
                            .map(|l| l.rev == tag_commit.oid)
                            .unwrap_or(false)
                        {
                            break LockResult::Unchanged(lock.unwrap());
                        } else {
                            break LockResult::Changed(
                                lock,
                                Lock {
                                    rev: tag_commit.oid,
                                    tag: Some(tag.name),
                                    commit_date: tag_commit.committed_date,
                                },
                            );
                        }
                    })
                }
            }
        })
    }
}
