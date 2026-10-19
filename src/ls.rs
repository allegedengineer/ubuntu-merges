use std::{
    collections::HashMap,
    io::{Write, stdout},
    num::NonZeroUsize,
};

use reqwest::Client;
use serde::{Deserialize, Serialize};
use supports_hyperlinks::supports_hyperlinks;
use tokio::{process::Command, task::JoinSet};
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
    /// Search for Launchpad bugs.
    #[arg(short = 'b', long)]
    pub find_bugs: bool,
    /// Write links out instead of using escape codes.
    #[arg(short = 'L', long)]
    pub no_fancy_links: bool,
    /// Cycle name to use for create-merge-bug links.
    #[arg(short, long, default_value = "tenacious")]
    pub cycle_name: String,
    /// Milestone to use for create-merge-bug links.
    #[arg(short = 'M', long, default_value = "ubuntu-26.11")]
    pub milestone: String,
    /// Search for bugs already assigned to you.
    #[arg(short, long)]
    pub mine: bool,
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
        pockets.sort_by(|(a, _), (b, _)| a.cmp(b));
        pockets
            .iter_mut()
            .for_each(|(_, p)| p.sort_by(|a, b| a.source_package.cmp(&b.source_package)));

        let bugs = if self.find_bugs {
            let assignee = if self.mine {
                let out = Command::new("git")
                    .args(["config", "--get", "gitubuntu.lpuser"])
                    .output()
                    .await?
                    .stdout;
                String::from_utf8(out).ok()
            } else {
                None
            };
            list_merge_bugs(self.filter.as_deref(), assignee.as_ref().map(|a| a.trim())).await?
        } else {
            vec![]
        };

        if let [(_, merges)] = &*pockets {
            for merge in merges {
                let bug = bugs.iter().find(|b| {
                    let Some((_, target)) = b.target_link.rsplit_once('/') else {
                        return false;
                    };
                    target == merge.source_package
                });
                if self.find_bugs && self.mine && bug.is_none() {
                    continue;
                }
                print!(
                    "{}\t{}",
                    merge.source_package,
                    merge.user,
                    
                );
                if let Some(uploader) = merge.uploader.as_deref() {
                    print!(" {uploader}");
                }
                if self.find_bugs {
                    if let Some(bug) = bug {
                        let (_, number) = bug.web_link.rsplit_once('/').unwrap_or_default();
                        self.print_link(&bug.web_link, &number)?;
                        if let Some(assignee_link) = bug.assignee_link.as_deref() && let Some((_, assignee)) = assignee_link.rsplit_once("~") {
                            self.print_link(assignee_link, assignee)?;
                        }
                    } else {
                        let mk_bug = merge_bug_create_url(
                            &merge.source_package,
                            &self.cycle_name,
                            &self.milestone,
                        );
                        self.print_link(&mk_bug, "Create bug")?;
                    }
                }
                println!();
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
                        merge.uploader.as_deref().unwrap_or_default()
                    );
                }
            }
        }
        Ok(())
    }

    fn print_link(&self, link: &str, title: &str) -> Result<(), eyre::Error> {
        if self.no_fancy_links || !supports_hyperlinks() {
            print!("\t{title}: {link}");
        } else {
            print!("\t{}{title}{}", osc8::Hyperlink::new(link), osc8::Hyperlink::END);
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct LpBugsResponse {
    pub next_collection_link: Option<String>,
    pub entries: Vec<MergeBug>,
}

#[derive(Serialize, Deserialize)]
struct MergeBug {
    pub self_link: String,
    pub web_link: String,
    pub bug_link: String,
    pub assignee_link: Option<String>,
    pub target_link: String,
    pub is_complete: bool,
    pub title: String,
}

/// Fetches the list of all merge bugs from Launchpad, with optional search string.
async fn list_merge_bugs(
    search_text: Option<&str>,
    assignee: Option<&str>,
) -> eyre::Result<Vec<MergeBug>> {
    let client = Client::new();
    let mut url = format!(
        "https://api.launchpad.net/devel/bugs?ws.op=searchTasks&tags=dcr-merge{search}{assignee}",
        search = search_text.map_or_default(|s| format!("&search_text={s}")),
        assignee =
            assignee.map_or_default(|a| format!("&assignee=https://api.launchpad.net/devel/~{a}")),
    );
    let mut entries = vec![];
    loop {
        let response = client
            .get(&url)
            .retry_send(NonZeroUsize::new(5).unwrap(), 2.0)
            .instrument(info_span!("collecting", url = url))
            .await?;
        if !response.status().is_success() {
            return Err(eyre::Report::msg(format!(
                "Request to {url} failed with {}",
                response.status().as_str()
            )));
        }
        let mut response: LpBugsResponse = response.json().await?;
        entries.extend(response.entries.drain(..));
        if let Some(next) = response.next_collection_link {
            url = next;
        } else {
            break;
        }
    }
    entries.retain(|b| !b.is_complete);
    Ok(entries)
}

/// Generates a URL to create a merge bug for a package.
fn merge_bug_create_url(package: &str, cycle: &str, milestone: &str) -> String {
    format!(
        "https://launchpad.net/ubuntu/+source/{package}/+filebug?field.title=Merge%20{package}%20for%20{cycle}%20cycle&field.milestone={milestone}"
    )
}
