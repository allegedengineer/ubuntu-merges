use std::num::NonZeroUsize;

use reqwest::Client;
use tokio::task::JoinSet;
use tracing::{Instrument, info_span};
use ubuntu_merges::retry::RequestRetry;

use crate::Args;

/// List merges by pocket.
#[derive(clap::Parser)]
pub struct Ls {
    /// String to filter by.
    pub filter: Option<String>,
    /// Pockets to search.
    #[arg(short, long)]
    pub pocket: Vec<String>,
}

impl Ls {
    pub async fn run(&self, _args: &Args) -> eyre::Result<()> {
        let pockets: Vec<_> = if self.pocket.is_empty() {
            [
                "main",
                "main-manual",
                "restricted",
                "restricted-manual",
                "universe",
                "universe-manual",
                "multiverse",
                "multiverse-manual",
            ]
            .into_iter()
            .map(|s| s.to_string())
            .collect()
        } else {
            self.pocket.clone()
        };
        let mut join_set: JoinSet<eyre::Result<(String, Vec<ubuntu_merges::MergeListing>)>> =
            JoinSet::new();
        for pocket in pockets {
            let client = Client::new();
            let url = format!("https://merges.ubuntu.com/{pocket}.json");
            let url_ = url.clone();
            join_set.spawn(
                async move {
                    Ok((
                        pocket,
                        client
                            .get(url)
                            .retry_send(NonZeroUsize::new(5).unwrap(), 2.)
                            .await?
                            .json()
                            .await?,
                    ))
                }
                .instrument(info_span!("fetching", url = url_)),
            );
        }
        let mut pockets = join_set
            .join_all()
            .await
            .into_iter()
            .collect::<eyre::Result<Vec<_>>>()?;
        pockets.retain_mut(|(_, merges)| {
            if let Some(filter) = self.filter.as_deref() {
                merges.retain(|m| m.source_package.contains(filter));
            }
            !merges.is_empty()
        });

        if let [(_, merges)] = &*pockets {
            for merge in merges {
                println!(
                    "{}\t{} {}",
                    merge.source_package,
                    merge.user,
                    merge.uploader.as_deref().unwrap_or("")
                );
            }
        } else if pockets.is_empty() {
            println!("(No merges! Yippee!)")
        } else {
            for (pocket, merges) in &*pockets {
                if merges.is_empty() {
                    continue;
                }
                println!("{pocket}:");
                for merge in merges {
                    println!(
                        "\t{}\t{} {}",
                        merge.source_package,
                        merge.user,
                        merge.uploader.as_deref().unwrap_or("???")
                    );
                }
            }
        }
        Ok(())
    }
}
