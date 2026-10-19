use std::{env::current_dir, io::PipeReader, num::NonZeroUsize, process::Stdio, sync::Arc};

use eyre::{Error, OptionExt};
use reqwest::Client;
use tokio::process::Command;
use tracing::{Instrument, debug_span, error, info, instrument};
use ubuntu_merges::{deb_ftp_dir, retry::RequestRetry};

use crate::{Args, grab::Grab};

/// Initialize a git-ubuntu working tree for merging.
#[derive(clap::Parser)]
pub struct Init {
    /// The package to prepare to merge.
    pub package: String,
    /// The directory to prepare it in.
    pub directory: Option<String>,
    /// The branch to start the merge from.
    #[arg(short, long, default_value = "pkg/ubuntu/devel")]
    pub branch: String,
}

impl Init {
    pub async fn run(&self, _args: &Args) -> eyre::Result<()> {
        let cwd = current_dir().unwrap();
        let directory = self.directory.clone().unwrap_or_else(|| {
            format!(
                "{1}/{0}-gu",
                self.package,
                if cwd.ends_with(&self.package) {
                    "."
                } else {
                    &self.package
                }
            )
        });
        let repo_path = cwd.join(&directory);
        if repo_path.exists() {
            info!("{repo_path:?} exists, opening it");
        } else {
            info!("Create dir {:?}", repo_path.parent().unwrap());
            tokio::fs::create_dir_all(repo_path.parent().unwrap()).await?;
            info!("Invoking `git ubuntu clone {} {repo_path:?}`", self.package);
            let result = Command::new("git")
                .arg("ubuntu")
                .arg("clone")
                .arg(&self.package)
                .arg(&repo_path)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?
                .wait()
                .await?;
            if !result.success() {
                error!(
                    "Nonzero exit code {} from git-ubuntu!",
                    result.code().unwrap_or(1)
                );
                std::process::exit(1);
            }
        }
        info!("Invoking `git fetch pkg` in worktree");
        let result = Command::new("git")
            .arg("fetch")
            .arg("pkg")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .current_dir(&repo_path)
            .spawn()?
            .wait()
            .await?;
        if !result.success() {
            error!("Nonzero exit code {} from git!", result.code().unwrap_or(1));
            std::process::exit(1);
        }

        info!("Invoking `git ubuntu merge start` in worktree");
        let result = Command::new("git")
            .arg("ubuntu")
            .arg("merge")
            .arg("start")
            .arg(&self.branch)
            .current_dir(&repo_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?
            .wait()
            .await?;
        if !result.success() {
            error!(
                "Nonzero exit code {} from git-ubuntu!",
                result.code().unwrap_or(1)
            );
            std::process::exit(1);
        }
        info!("Invoking `git ubuntu export-orig --for-merge` in worktree");
        let result = Command::new("git")
            .arg("ubuntu")
            .arg("export-orig")
            .arg("--for-merge")
            .current_dir(&repo_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?
            .wait()
            .await?;
        if !result.success() {
            error!(
                "Nonzero exit code {} from git-ubuntu!",
                result.code().unwrap_or(1)
            );
            std::process::exit(1);
        }

        Ok(())
    }

    #[instrument(name = "reporting", skip(self))]
    async fn fetch_report(&self) -> eyre::Result<String> {
        let url = format!(
            "https://merges.ubuntu.com/{group}/{package}/REPORT",
            group = deb_ftp_dir(&self.package),
            package = self.package
        );
        let client = Client::new();
        let report = client
            .get(&url)
            .retry_send(NonZeroUsize::new(5).unwrap(), 2.)
            .await?
            .text()
            .instrument(debug_span!("receiving"))
            .await?;
        Ok(report)
    }
}
