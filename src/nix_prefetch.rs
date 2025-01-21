use std::process::Stdio;

use anyhow::Context;
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};
use tokio::{
    io::{AsyncBufReadExt as _, AsyncReadExt, BufReader},
    process::Command,
};
#[derive(Deserialize_repr, Serialize_repr, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
enum ActivityType {
    Unknown = 0,
    CopyPath = 100,
    FileTransfer = 101,
    Realise = 102,
    CopyPaths = 103,
    Builds = 104,
    Build = 105,
    OptimiseStore = 106,
    VerifyPaths = 107,
    Substitute = 108,
    QueryPathInfo = 109,
    PostBuildHook = 110,
    BuildWaiting = 111,
    FetchTree = 112,
}

#[derive(Deserialize_repr, Serialize_repr, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
enum ResultType {
    FileLinked = 100,
    BuildLogLine = 101,
    UntrustedPath = 102,
    CorruptedPath = 103,
    SetPhase = 104,
    Progress = 105,
    SetExpected = 106,
    PostBuildLogLine = 107,
    FetchStatus = 108,
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
enum Field {
    Int(i64),
    Str(String),
}

impl Field {
    fn as_int(&self) -> Option<i64> {
        match self {
            Field::Int(i) => Some(*i),
            _ => None,
        }
    }
    fn as_str(&self) -> Option<&str> {
        match self {
            Field::Str(s) => Some(s),
            _ => None,
        }
    }
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PrefetchResult {
    pub hash: ssri2::Integrity,
    pub store_path: String,
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "action", rename_all = "lowercase")]
enum Action {
    Start {
        #[serde(default)]
        fields: Vec<Field>,
        id: u64,
        level: u64,
        parent: u64,
        text: String,
        #[serde(rename = "type")]
        type_: ActivityType,
    },
    Stop {
        id: u64,
    },
    Result {
        fields: Vec<Field>,
        id: u64,
        #[serde(rename = "type")]
        type_: ResultType,
    },
    Msg {
        msg: String,
    },
}

pub async fn fetch(url: &url::Url, unpack: bool, pb: &ProgressBar) -> anyhow::Result<PrefetchResult> {
    let mut cmd = Command::new("nix");
    cmd.args([
        "store",
        "prefetch-file",
        "--json",
        "--log-format",
        "internal-json",
        "--name",
        "source",
    ]);
    if unpack {
        cmd.arg("--unpack");
    }
    if let Some(segs) = url.path_segments() {
        pb.set_message(segs.last().map(ToOwned::to_owned).unwrap_or_default());
    } else {
        pb.set_message(url.as_str().to_owned());
    }
    let mut child = cmd
        .arg(url.as_str())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to spawn")?;
    let mut stdout = child.stdout.take().context("no stdout")?;
    let stderr = child.stderr.take().context("no stderr")?;
    let mut lines = BufReader::new(stderr).lines();
    let act_id = loop {
        let Some(line) = lines.next_line().await? else {
            break None;
        };
        if let Some(msg) = line.strip_prefix("@nix ") {
            let act: Action = serde_json::from_str(msg)?;
            match act {
                Action::Start {
                    id,
                    type_: ActivityType::FileTransfer,
                    ..
                } => {
                    break Some(id);
                }
                Action::Msg { msg } => {
                    pb.set_message(msg);
                }
                _ => (),
            }
        }
    }
    .context("unexpected nix output")?;

    let mut saved_total = None;
    loop {
        let Some(line) = lines.next_line().await? else {
            break;
        };
        if let Some(msg) = line.strip_prefix("@nix ") {
            let act: Action = serde_json::from_str(msg)?;
            match act {
                Action::Msg { msg } => {
                    pb.set_message(msg);
                }
                Action::Result {
                    id,
                    type_: ResultType::Progress,
                    fields,
                } if id == act_id => {
                    let progress = fields[0].as_int().context("unexpected progress")?;
                    let total = fields[1].as_int().context("unexpected total")?;
                    if total != 0 {
                        if saved_total == Some(0) || saved_total.is_none() {
                            pb.set_style(ProgressStyle::with_template("{spinner} {msg} {bar} {bytes}/{total_bytes}").unwrap());
                        }
                        if saved_total != Some(total) {
                            pb.set_length(total as u64);
                        }
                    } else if saved_total.is_none() || saved_total != Some(0) {
                        pb.set_style(
                            ProgressStyle::with_template("{spinner} {msg} {bytes}").unwrap(),
                        );
                    }

                    pb.set_position(progress as u64);
                    saved_total = Some(total);
                }
                _ => (),
            }
        }
    }
    pb.finish();

    let mut output = String::new();
    stdout.read_to_string(&mut output).await?;

    serde_json::from_str(&output).map_err(Into::into)
}
