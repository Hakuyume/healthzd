mod de;

use crate::hyper;
use bytes::Bytes;
use futures::{FutureExt, Stream};
use std::fmt;
use std::time::Duration;
use tracing_futures::Instrument;

#[derive(Clone, Debug)]
pub struct Probe {
    pub method: Method,
    pub initial_delay: Duration,
    pub period: Duration,
    pub timeout: Duration,
    pub success_threshold: usize,
    pub failure_threshold: usize,
}

#[derive(Clone, Debug)]
pub enum Method {
    Exec {
        command: (String, Vec<String>),
    },
    HttpGet {
        uri: http::Uri,
        headers: http::HeaderMap,
    },
}

pub struct Context {
    pub client: hyper::Client<http_body_util::Empty<Bytes>>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Status {
    Success,
    Failure,
}

impl Probe {
    pub fn watch<'a>(&'a self, context: &'a Context) -> impl Stream<Item = Status> + 'a {
        struct State {
            deadline: tokio::time::Instant,
            success: usize,
            failure: usize,
        }

        let state = State {
            deadline: tokio::time::Instant::now() + self.initial_delay,
            success: 0,
            failure: 0,
        };
        futures::stream::unfold(state, |mut state| {
            async {
                loop {
                    tokio::time::sleep_until(state.deadline).await;
                    state.deadline += self.period;

                    match tokio::time::timeout(self.timeout, self.method.call(context))
                        .map(|output| output?)
                        .await
                    {
                        Ok(_) => {
                            tracing::info!("ok");
                            state.success += 1;
                            state.failure = 0;
                        }
                        Err(e) => {
                            tracing::warn!(error = e.to_string());
                            state.success = 0;
                            state.failure += 1;
                        }
                    }

                    if state.success == self.success_threshold {
                        break Some((Status::Success, state));
                    }
                    if state.failure == self.failure_threshold {
                        break Some((Status::Failure, state));
                    }
                }
            }
            .instrument(self.method.span())
        })
    }
}

impl Method {
    async fn call(&self, context: &Context) -> anyhow::Result<()> {
        match self {
            Self::Exec {
                command: (program, args),
            } => {
                let status = tokio::process::Command::new(program)
                    .args(args)
                    .kill_on_drop(true)
                    .status()
                    .await?;
                if !status.success() {
                    anyhow::bail!("{status}");
                }
            }
            Self::HttpGet { uri, headers } => {
                let mut request = http::Request::new(http_body_util::Empty::new());
                *request.method_mut() = http::Method::GET;
                request.uri_mut().clone_from(uri);
                request.headers_mut().clone_from(headers);
                let response = context.client.request(request).await?;
                if !response.status().is_success() {
                    anyhow::bail!("{}", response.status());
                }
            }
        }
        Ok(())
    }

    fn span(&self) -> tracing::Span {
        match self {
            Self::Exec {
                command: (program, args),
            } => {
                struct Command<'a> {
                    program: &'a String,
                    args: &'a Vec<String>,
                }

                impl fmt::Debug for Command<'_> {
                    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
                        fmt.debug_list()
                            .entry(self.program)
                            .entries(self.args)
                            .finish()
                    }
                }

                let command = Command { program, args };
                tracing::info_span!("exec", ?command)
            }
            Self::HttpGet { uri, .. } => {
                tracing::info_span!("http_get", ?uri)
            }
        }
    }
}
