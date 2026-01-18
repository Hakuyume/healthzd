mod hyper;
mod probe;

use axum::{Router, routing};
use clap::Parser;
use futures::{FutureExt, StreamExt};
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing_futures::Instrument;

#[derive(Parser)]
struct Args {
    #[clap(long)]
    bind: SocketAddr,
    #[clap(long)]
    probe: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();

    let probe = toml::from_slice::<
        serde_with::de::DeserializeAsWrap<
            Vec<(String, Probe)>,
            serde_with::Map<serde_with::Same, serde_with::Same>,
        >,
    >(&tokio::fs::read(&args.probe).await?)?;

    let tls_config = hyper::tls_config()?;
    let context = probe::Context {
        client: hyper::client(tls_config),
    };

    let probe = probe
        .into_inner()
        .into_iter()
        .map(|(name, probe)| (name, probe, Status::default()))
        .collect::<Arc<[_]>>();

    futures::future::try_join(
        async {
            let app = Router::new()
                .route(
                    "/liveness",
                    routing::get({
                        let probe = probe.clone();
                        async move || {
                            if probe
                                .iter()
                                .all(|(_, _, status)| status.liveness.load(Ordering::Relaxed))
                            {
                                http::StatusCode::OK
                            } else {
                                http::StatusCode::INTERNAL_SERVER_ERROR
                            }
                        }
                    }),
                )
                .route(
                    "/readiness",
                    routing::get({
                        let probe = probe.clone();
                        async move || {
                            if probe
                                .iter()
                                .all(|(_, _, status)| status.readiness.load(Ordering::Relaxed))
                            {
                                http::StatusCode::OK
                            } else {
                                http::StatusCode::SERVICE_UNAVAILABLE
                            }
                        }
                    }),
                );

            let listener = tokio::net::TcpListener::bind(args.bind).await?;
            axum::serve(listener, app).await
        },
        futures::future::join_all(probe.iter().map(|(name, probe, status)| {
            async {
                if let Some(probe) = &probe.startup {
                    let mut stream = pin::pin!(
                        probe
                            .watch(&context)
                            .instrument(tracing::info_span!("startup"))
                    );
                    while let Some(status) = stream.next().await {
                        if status == probe::Status::Success {
                            break;
                        }
                    }
                }
                futures::future::join(
                    async {
                        if let Some(probe) = &probe.liveness {
                            let mut stream = pin::pin!(
                                probe
                                    .watch(&context)
                                    .instrument(tracing::info_span!("liveness"))
                            );
                            while let Some(s) = stream.next().await {
                                if s == probe::Status::Failure {
                                    status.liveness.store(false, Ordering::Relaxed);
                                    break;
                                }
                            }
                        }
                    },
                    async {
                        if let Some(probe) = &probe.readiness {
                            let mut stream = pin::pin!(
                                probe
                                    .watch(&context)
                                    .instrument(tracing::info_span!("readiness"))
                            );
                            while let Some(s) = stream.next().await {
                                match s {
                                    probe::Status::Success => {
                                        status.readiness.store(true, Ordering::Relaxed)
                                    }
                                    probe::Status::Failure => {
                                        status.readiness.store(false, Ordering::Relaxed)
                                    }
                                }
                            }
                        } else {
                            status.readiness.store(true, Ordering::Relaxed)
                        }
                    },
                )
                .await;
            }
            .instrument(tracing::info_span!("probe", name))
        }))
        .map(Ok),
    )
    .await?;

    Ok(())
}

#[derive(Deserialize)]
struct Probe {
    #[serde(rename = "liveness_probe")]
    liveness: Option<probe::Probe>,
    #[serde(rename = "readiness_probe")]
    readiness: Option<probe::Probe>,
    #[serde(rename = "startup_probe")]
    startup: Option<probe::Probe>,
}

struct Status {
    liveness: AtomicBool,
    readiness: AtomicBool,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            liveness: AtomicBool::new(true),
            readiness: AtomicBool::new(false),
        }
    }
}
