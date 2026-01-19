use crate::{hyper, probe};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Duration;

struct Fixture {
    _temp: tempfile::TempDir,
    context: probe::Context,
    target: super::Target,
    status: super::Status,
    liveness: PathBuf,
    readiness: PathBuf,
    startup: PathBuf,
}

impl Fixture {
    fn new(with_liveness: bool, with_readiness: bool, with_startup: bool) -> Self {
        fn probe(path: &Path) -> probe::Probe {
            probe::Probe {
                method: probe::Method::Exec {
                    command: (
                        "test".to_string(),
                        vec!["-f".to_string(), path.display().to_string()],
                    ),
                },
                initial_delay: Duration::default(),
                period: Duration::from_millis(100),
                timeout: Duration::from_millis(10),
                success_threshold: 1,
                failure_threshold: 1,
            }
        }

        let tls_config = hyper::tls_config().unwrap();
        let context = probe::Context {
            client: hyper::client(tls_config),
        };

        let temp = tempfile::tempdir().unwrap();
        let liveness = temp.path().join("liveness");
        let readiness = temp.path().join("readiness");
        let startup = temp.path().join("startup");

        let target = super::Target {
            name: "test".to_string(),
            liveness_probe: with_liveness.then(|| probe(&liveness)),
            readiness_probe: with_readiness.then(|| probe(&readiness)),
            startup_probe: with_startup.then(|| probe(&startup)),
        };

        Self {
            _temp: temp,
            context,
            target,
            status: super::Status::default(),
            liveness,
            readiness,
            startup,
        }
    }

    fn update(&self) -> impl Future<Output = ()> + '_ {
        super::update(&self.context, &self.target, &self.status)
    }

    async fn liveness(&self, value: bool) {
        if value {
            tokio::fs::write(&self.liveness, b"").await.unwrap();
        } else {
            tokio::fs::remove_file(&self.liveness).await.unwrap();
        }
    }

    async fn readiness(&self, value: bool) {
        if value {
            tokio::fs::write(&self.readiness, b"").await.unwrap();
        } else {
            tokio::fs::remove_file(&self.readiness).await.unwrap();
        }
    }

    async fn startup(&self, value: bool) {
        if value {
            tokio::fs::write(&self.startup, b"").await.unwrap();
        } else {
            tokio::fs::remove_file(&self.startup).await.unwrap();
        }
    }
}

#[tokio::test]
async fn test_update_empty() {
    let fixture = Fixture::new(false, false, false);
    futures::future::join(fixture.update(), async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(fixture.status.live.load(Ordering::Relaxed));
        assert!(fixture.status.ready.load(Ordering::Relaxed));
    })
    .await;
}

#[tokio::test]
async fn test_update_liveness() {
    let fixture = Fixture::new(true, false, false);
    futures::future::join(fixture.update(), async {
        assert!(fixture.status.live.load(Ordering::Relaxed));
        assert!(fixture.status.ready.load(Ordering::Relaxed));

        fixture.liveness(true).await;
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert!(fixture.status.live.load(Ordering::Relaxed));
        assert!(fixture.status.ready.load(Ordering::Relaxed));

        fixture.liveness(false).await;
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert!(!fixture.status.live.load(Ordering::Relaxed));
        assert!(fixture.status.ready.load(Ordering::Relaxed));

        fixture.liveness(true).await;
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert!(!fixture.status.live.load(Ordering::Relaxed));
        assert!(fixture.status.ready.load(Ordering::Relaxed));
    })
    .await;
}

#[tokio::test]
async fn test_update_readiness() {
    let fixture = Fixture::new(false, true, false);
    let (update, abort) = futures::future::abortable(fixture.update());
    let _ = futures::future::join(update, async {
        assert!(fixture.status.live.load(Ordering::Relaxed));
        assert!(!fixture.status.ready.load(Ordering::Relaxed));

        tokio::time::sleep(Duration::from_millis(200)).await;

        assert!(fixture.status.live.load(Ordering::Relaxed));
        assert!(!fixture.status.ready.load(Ordering::Relaxed));

        fixture.readiness(true).await;
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert!(fixture.status.live.load(Ordering::Relaxed));
        assert!(fixture.status.ready.load(Ordering::Relaxed));

        fixture.readiness(false).await;
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert!(fixture.status.live.load(Ordering::Relaxed));
        assert!(!fixture.status.ready.load(Ordering::Relaxed));

        fixture.readiness(true).await;
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert!(fixture.status.live.load(Ordering::Relaxed));
        assert!(fixture.status.ready.load(Ordering::Relaxed));

        abort.abort();
    })
    .await;
}

#[tokio::test]
async fn test_update_startup() {
    let fixture = Fixture::new(false, false, true);
    futures::future::join(fixture.update(), async {
        assert!(fixture.status.live.load(Ordering::Relaxed));
        assert!(!fixture.status.ready.load(Ordering::Relaxed));

        tokio::time::sleep(Duration::from_millis(200)).await;

        assert!(fixture.status.live.load(Ordering::Relaxed));
        assert!(!fixture.status.ready.load(Ordering::Relaxed));

        fixture.startup(true).await;
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert!(fixture.status.live.load(Ordering::Relaxed));
        assert!(fixture.status.ready.load(Ordering::Relaxed));

        fixture.startup(false).await;
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert!(fixture.status.live.load(Ordering::Relaxed));
        assert!(fixture.status.ready.load(Ordering::Relaxed));
    })
    .await;
}

#[tokio::test]
async fn test_update_all() {
    let fixture = Fixture::new(true, true, true);
    let (update, abort) = futures::future::abortable(fixture.update());
    let _ = futures::future::join(update, async {
        assert!(fixture.status.live.load(Ordering::Relaxed));
        assert!(!fixture.status.ready.load(Ordering::Relaxed));

        tokio::time::sleep(Duration::from_millis(200)).await;

        assert!(fixture.status.live.load(Ordering::Relaxed));
        assert!(!fixture.status.ready.load(Ordering::Relaxed));

        fixture.liveness(true).await;
        fixture.readiness(true).await;
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert!(fixture.status.live.load(Ordering::Relaxed));
        assert!(!fixture.status.ready.load(Ordering::Relaxed));

        fixture.liveness(false).await;
        fixture.readiness(false).await;
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert!(fixture.status.live.load(Ordering::Relaxed));
        assert!(!fixture.status.ready.load(Ordering::Relaxed));

        fixture.liveness(true).await;
        fixture.readiness(true).await;
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert!(fixture.status.live.load(Ordering::Relaxed));
        assert!(!fixture.status.ready.load(Ordering::Relaxed));

        fixture.startup(true).await;
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert!(fixture.status.live.load(Ordering::Relaxed));
        assert!(fixture.status.ready.load(Ordering::Relaxed));

        fixture.readiness(false).await;
        fixture.startup(false).await;
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert!(fixture.status.live.load(Ordering::Relaxed));
        assert!(!fixture.status.ready.load(Ordering::Relaxed));

        fixture.readiness(true).await;
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert!(fixture.status.live.load(Ordering::Relaxed));
        assert!(fixture.status.ready.load(Ordering::Relaxed));

        fixture.liveness(false).await;
        fixture.readiness(false).await;
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert!(!fixture.status.live.load(Ordering::Relaxed));
        assert!(!fixture.status.ready.load(Ordering::Relaxed));

        fixture.liveness(true).await;
        fixture.readiness(true).await;
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert!(!fixture.status.live.load(Ordering::Relaxed));
        assert!(fixture.status.ready.load(Ordering::Relaxed));

        abort.abort();
    })
    .await;
}
