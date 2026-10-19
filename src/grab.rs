use std::{fmt::Write, num::NonZeroUsize, sync::Arc};

use eyre::{OptionExt, Report};
use reqwest::Client;
use tokio::{process::Command, task::JoinSet};
use tracing::{Instrument, debug_span, error, info, info_span, instrument};
use ubuntu_merges::{deb_ftp_dir, retry::RequestRetry};

use crate::Args;

/// Download a merge's packaging files and report into a working directory.
#[derive(clap::Parser)]
pub struct Grab {
    /// The name of the target package.
    pub package_name: String,
    /// The directory to clone into.
    pub directory_name: Option<String>,
}

impl Grab {
    pub async fn run(&self, _args: &Args) -> eyre::Result<()> {
        let directory_name = self.directory_name.as_deref().unwrap_or(&self.package_name);
        let directory = Arc::new(std::env::current_dir()?.join(directory_name));

        Self::mkdir(&directory).await?;

        let report = self.fetch_report(&directory).await?;

        self.fetch_files(&directory, &report).await;

        Self::write_scripts(directory, report).await?;

        Ok(())
    }

    async fn mkdir(directory: &Arc<std::path::PathBuf>) -> Result<(), Report> {

        if directory.exists() {
            if tokio::fs::read_dir(&**directory)
                .await?
                .next_entry()
                .await?
                .is_some()
            {
                return Err(Report::msg(format!(
                    "Attempted to clone into {directory:?}, but it already exists and is not empty."
                )));
            }
        } else {
            info!("Creating {directory:?}");
            tokio::fs::create_dir_all(&**directory).await?;
        }
        Ok(())
    }

    #[instrument(name = "fetching files", skip(self, directory, report))]
    async fn fetch_files(&self, directory: &Arc<std::path::PathBuf>, report: &String) {
        let mut join_set: JoinSet<eyre::Result<String>> = JoinSet::new();

        for file in report.lines().filter_map(|line| line.strip_prefix("    ")) {
            join_set.spawn(Self::fetch_file(
                file.to_string(),
                self.package_name.clone(),
                Client::new(),
                directory.clone(),
            ).in_current_span());
        }

        while let Some(v) = join_set.join_next().await {
            let Ok(v) = v else {
                error!("Worker panicked");
                continue;
            };
            if let Err(e) = v {
                error!("{e}");
            }
        }
    }

    #[instrument(name = "reporting", skip(self, directory))]
    pub async fn fetch_report(&self, directory: &Arc<std::path::PathBuf>) -> Result<String, Report> {
        let url = format!(
            "https://merges.ubuntu.com/{group}/{package}/REPORT",
            group = deb_ftp_dir(&self.package_name),
            package = self.package_name
        );
        let client = Client::new();
        let report = client
            .get(&url)
            .retry_send(NonZeroUsize::new(5).unwrap(), 2.)
            .await?
            .text()
            .instrument(debug_span!("receiving"))
            .await?;
        tokio::fs::write(directory.join("REPORT"), &report)
            .instrument(debug_span!("writing", file = "REPORT"))
            .await?;
        Ok(report)
    }

    #[instrument(name = "fetching", skip(package_name, client, directory))]
    async fn fetch_file(
        file: String,
        package_name: String,
        client: Client,
        directory: Arc<std::path::PathBuf>,
    ) -> Result<String, Report> {
        let url = format!(
            "https://merges.ubuntu.com/{letter}/{package}/{file}",
            letter = package_name
                .chars()
                .next()
                .ok_or_eyre("Package name is empty")?,
            package = package_name
        );
        let res = client
            .get(&url)
            .retry_send(NonZeroUsize::new(5).unwrap(), 2.)
            .await?;
        if !res.status().is_success() {
            error!("{url} returned HTTP {status}", status = res.status());
            return Err(Report::msg(res.text().await?));
        }
        let data = res.bytes().instrument(debug_span!("receiving")).await?;
        let path = directory.join(&file);
        tokio::fs::write(&path, data).await?;
        if [".src", ".tar", ".gz"]
            .iter()
            .any(|ext| file.ends_with(ext))
            && let Ok(mut child) = Command::new("tar")
                .arg("xf")
                .arg(&file)
                .current_dir(&*directory)
                .spawn()
        {
            let status = child
                .wait()
                .instrument(debug_span!("extracting", file = file))
                .await;
            match status {
                Ok(s) if s.success() => {}
                Ok(s) => {
                    error!("Extracting {file} failed: {}", s.code().unwrap_or(1));
                }
                Err(e) => {
                    error!("Extracting {file} failed: {e}");
                }
            }
        }
        Ok(file)
    }

    #[instrument(name = "writing scripts", skip(directory, report))]
    async fn write_scripts(
        directory: Arc<std::path::PathBuf>,
        report: String,
    ) -> Result<(), Report> {
        let mut genchanges = "#!/bin/sh\n".to_string();
        let mut buildpackage = "#!/bin/sh\n".to_string();
        let mut debuild = "#!/bin/sh\n".to_string();
        for line in report.lines().filter_map(|line| line.strip_prefix("  $ ")) {
            writeln!(genchanges, r#"exec {line} "$@""#).unwrap();
            writeln!(
                buildpackage,
                r#"exec {line} "$@""#,
                line = line.replace("dpkg-genchanges", "dpkg-buildpackage")
            )
            .unwrap();
            writeln!(
                debuild,
                r#"exec {line} "$@""#,
                line = line.replace("dpkg-genchanges", "debuild")
            )
            .unwrap();
        }
        Ok(
            for (file, contents) in [
                ("merge-genchanges", genchanges),
                ("merge-buildpackage", buildpackage),
                ("merge-debuild", debuild),
            ] {
                tokio::fs::write(directory.join(file), contents).await?;
            },
        )
    }
}
