use std::{num::NonZeroUsize, time::Duration};

use eyre::Report;
use reqwest::{RequestBuilder, Response};
use tracing::{Instrument, debug_span, info_span, warn};

pub trait RequestRetry {
    fn retry_send(
        self,
        tries: NonZeroUsize,
        backoff: f32,
    ) -> impl std::future::Future<Output = eyre::Result<Response>> + Send;
}

impl RequestRetry for RequestBuilder {
    async fn retry_send(self, tries: NonZeroUsize, backoff: f32) -> eyre::Result<Response> {
        let mut wait = Duration::from_secs(2);
        let mut last_err = None;
        for attempt in 1..=tries.get() {
            let mut fail_fast = false;
            match self
                .try_clone()
                .unwrap()
                .send()
                .instrument(debug_span!("connecting"))
                .await
            {
                Ok(v) if v.status().is_success() || attempt == tries.get() => return Ok(v),
                Ok(v) => {
                    last_err = Some(Report::msg(v.status().to_string()));
                    let code = v.status().as_u16();
                    if (400u16..=499).contains(&code) && code != 429 {
                        fail_fast = true;
                    }
                }
                Err(e) => last_err = Some(e.into()),
            }
            if attempt < tries.get() || fail_fast {
                warn!(
                    "Request failed with: {}\n({attempt}/{tries}{})",
                    last_err.as_ref().unwrap(),
                    fail_fast.then_some(", failing fast").unwrap_or_default()
                );
            }
            tokio::time::sleep(wait)
                .instrument(debug_span!("backoff", seconds = wait.as_secs_f32()))
                .await;
            wait = wait.mul_f32(backoff);
        }
        Err(last_err.unwrap())
    }
}
