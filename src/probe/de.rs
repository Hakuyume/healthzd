use serde::{Deserialize, Deserializer};
use std::fmt::Write;
use std::time::Duration;

impl<'de> Deserialize<'de> for super::Probe {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[serde_with::serde_as]
        #[derive(Deserialize)]
        struct Probe {
            #[serde(flatten)]
            method: super::Method,
            #[serde_as(as = "Option<serde_with::DurationSeconds<u64>>")]
            initial_delay_seconds: Option<Duration>,
            #[serde_as(as = "Option<serde_with::DurationSeconds<u64>>")]
            period_seconds: Option<Duration>,
            #[serde(rename = "timeout_seconds")]
            #[serde_as(as = "Option<serde_with::DurationSeconds<u64>>")]
            timeout_seconds: Option<Duration>,
            success_threshold: Option<usize>,
            failure_threshold: Option<usize>,
        }

        let value = Probe::deserialize(deserializer)?;
        // https://kubernetes.io/docs/tasks/configure-pod-container/configure-liveness-readiness-startup-probes/#configure-probes
        Ok(Self {
            method: value.method,
            initial_delay: value
                .initial_delay_seconds
                .unwrap_or(Duration::from_secs(0)),
            period: value.period_seconds.unwrap_or(Duration::from_secs(10)),
            timeout: value.timeout_seconds.unwrap_or(Duration::from_secs(1)),
            success_threshold: value.success_threshold.unwrap_or(1),
            failure_threshold: value.failure_threshold.unwrap_or(3),
        })
    }
}

impl<'de> Deserialize<'de> for super::Method {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[serde_with::serde_as]
        #[derive(Deserialize)]
        #[serde(rename_all = "snake_case")]
        enum Method {
            Exec {
                command: Vec<String>,
            },
            // https://kubernetes.io/docs/tasks/configure-pod-container/configure-liveness-readiness-startup-probes/#http-probes
            HttpGet {
                host: Option<String>,
                scheme: Option<String>,
                path: Option<String>,
                #[serde(with = "http_serde::option::header_map", default)]
                http_headers: Option<http::HeaderMap>,
                port: Option<u16>,
            },
        }

        let value = Method::deserialize(deserializer)?;
        match value {
            Method::Exec { mut command } => {
                if command.is_empty() {
                    Err(serde::de::Error::invalid_length(
                        command.len(),
                        &"one or more",
                    ))
                } else {
                    Ok(Self::Exec {
                        command: (command.remove(0), command),
                    })
                }
            }
            Method::HttpGet {
                host,
                scheme,
                path,
                http_headers,
                port,
            } => {
                let mut uri = String::new();
                if let Some(scheme) = scheme {
                    uri.push_str(&scheme.to_lowercase());
                } else {
                    uri.push_str("http");
                }
                uri.push_str("://");
                if let Some(host) = host {
                    uri.push_str(&host);
                } else {
                    uri.push_str("localhost");
                }
                if let Some(port) = port {
                    write!(&mut uri, ":{port}").unwrap();
                }
                if let Some(path) = path {
                    uri.push_str(&path);
                } else {
                    uri.push('/');
                }
                Ok(Self::HttpGet {
                    uri: uri.parse().map_err(serde::de::Error::custom)?,
                    headers: http_headers.unwrap_or_default(),
                })
            }
        }
    }
}
