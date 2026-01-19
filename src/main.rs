mod hyper;
mod probe;

use axum::{Router, routing};
use clap::Parser;
use futures::{FutureExt, StreamExt};
use serde::Deserialize;
use std::io;
use std::net::SocketAddr;
use std::pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing_futures::Instrument;

#[derive(Parser)]
struct Args {
    #[clap(long)]
    bind: SocketAddr,
    #[clap(long, value_parser = parse_target)]
    target: Vec<Target>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();

    let tls_config = hyper::tls_config()?;
    let context = probe::Context {
        client: hyper::client(tls_config),
    };

    let targets = args
        .target
        .into_iter()
        .map(|target| (target, Status::default()))
        .collect();

    futures::future::try_join(
        serve(args.bind, &targets),
        futures::future::join_all(
            targets
                .iter()
                .map(|(target, status)| update(&context, target, status)),
        )
        .map(Ok),
    )
    .await?;

    Ok(())
}

#[derive(Clone, Deserialize)]
struct Target {
    name: String,
    liveness_probe: Option<probe::Probe>,
    readiness_probe: Option<probe::Probe>,
    startup_probe: Option<probe::Probe>,
}

fn parse_target(s: &str) -> Result<Target, String> {
    serde_json::from_str(s).map_err(|e| e.to_string())
}

struct Status {
    live: AtomicBool,
    ready: AtomicBool,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            live: AtomicBool::new(true),
            ready: AtomicBool::new(false),
        }
    }
}

async fn serve(bind: SocketAddr, targets: &Arc<[(Target, Status)]>) -> io::Result<()> {
    let app = Router::new()
        .route(
            "/live",
            routing::get({
                let targets = targets.clone();
                async move || {
                    if targets
                        .iter()
                        .all(|(_, status)| status.live.load(Ordering::Relaxed))
                    {
                        http::StatusCode::OK
                    } else {
                        http::StatusCode::INTERNAL_SERVER_ERROR
                    }
                }
            }),
        )
        .route(
            "/ready",
            routing::get({
                let targets = targets.clone();
                async move || {
                    if targets
                        .iter()
                        .all(|(_, status)| status.ready.load(Ordering::Relaxed))
                    {
                        http::StatusCode::OK
                    } else {
                        http::StatusCode::SERVICE_UNAVAILABLE
                    }
                }
            }),
        )
        .layer(tower_http::trace::TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(listener, app).await
}

fn update<'a>(
    context: &'a probe::Context,
    target: &'a Target,
    status: &'a Status,
) -> impl Future<Output = ()> + 'a {
    async move {
        if let Some(probe) = &target.startup_probe {
            let mut stream = pin::pin!(
                probe
                    .watch(context)
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
                if let Some(probe) = &target.liveness_probe {
                    let mut stream = pin::pin!(
                        probe
                            .watch(context)
                            .instrument(tracing::info_span!("liveness"))
                    );
                    while let Some(s) = stream.next().await {
                        if s == probe::Status::Failure {
                            status.live.store(false, Ordering::Relaxed);
                            break;
                        }
                    }
                }
            },
            async {
                if let Some(probe) = &target.readiness_probe {
                    let mut stream = pin::pin!(
                        probe
                            .watch(context)
                            .instrument(tracing::info_span!("readiness"))
                    );
                    while let Some(s) = stream.next().await {
                        match s {
                            probe::Status::Success => status.ready.store(true, Ordering::Relaxed),
                            probe::Status::Failure => status.ready.store(false, Ordering::Relaxed),
                        }
                    }
                } else {
                    status.ready.store(true, Ordering::Relaxed)
                }
            },
        )
        .await;
    }
    .instrument(tracing::info_span!("target", name = target.name))
}

#[cfg(test)]
mod tests;
