//! Minimal Omaha/Nebraska test double for Harpoon.

use std::{convert::Infallible, net::SocketAddr, path::Path};

use anyhow::Context;
use http_body_util::{BodyExt, Full};
use hyper::{
    body::{Bytes, Incoming},
    http::StatusCode,
    server::conn::http1,
    service::service_fn,
    Method, Request, Response,
};
use hyper_util::rt::TokioIo;
use serde::Deserialize;
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle};

#[derive(Debug, Clone, Deserialize)]
pub struct NebraskaScenario {
    #[serde(default)]
    pub available: bool,
    pub version: Option<String>,
    pub url: Option<String>,
    pub sha384: Option<String>,
}

impl NebraskaScenario {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read Nebraska scenario {}", path.display()))?;
        serde_yaml::from_str(&contents).context("failed to parse Nebraska scenario yaml")
    }

    fn render_response(&self) -> String {
        if !self.available {
            return indoc::formatdoc! {r#"
                <?xml version="1.0" encoding="UTF-8"?>
                <response protocol="3.0" server="tester">
                  <daystart elapsed_seconds="0"/>
                  <app appid="tester" status="ok">
                    <updatecheck status="noupdate"><urls></urls></updatecheck>
                  </app>
                </response>
            "#};
        }

        let version = self.version.as_deref().unwrap_or("1.0.0");
        let url = self.url.as_deref().unwrap_or("https://example.invalid/");
        let hash = self.sha384.as_deref().unwrap_or("ignored");
        indoc::formatdoc! {r#"
            <?xml version="1.0" encoding="UTF-8"?>
            <response protocol="3.0" server="tester">
              <daystart elapsed_seconds="0"/>
              <app appid="tester" status="ok">
                <updatecheck status="ok">
                  <urls><url codebase="{url}"/></urls>
                  <manifest version="{version}">
                    <packages>
                      <package hash="{hash}" name="acl.cosi" size="1" required="true"/>
                    </packages>
                  </manifest>
                </updatecheck>
              </app>
            </response>
        "#}
    }
}

pub struct NebraskaHandle {
    pub addr: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl NebraskaHandle {
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        let _ = self.task.await;
    }
}

pub async fn spawn(
    listen: SocketAddr,
    scenario: NebraskaScenario,
) -> anyhow::Result<NebraskaHandle> {
    let listener = TcpListener::bind(listen).await?;
    let addr = listener.local_addr()?;
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
    let xml = scenario.render_response();

    let task = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => break,
                accept = listener.accept() => {
                    let Ok((stream, _)) = accept else { break; };
                    let io = TokioIo::new(stream);
                    let xml = xml.clone();
                    tokio::spawn(async move {
                        let service = service_fn(move |request| {
                            let xml = xml.clone();
                            async move { handle_request(xml, request).await }
                        });
                        let _ = http1::Builder::new().serve_connection(io, service).await;
                    });
                }
            }
        }
    });

    Ok(NebraskaHandle {
        addr,
        shutdown: Some(shutdown_tx),
        task,
    })
}

async fn handle_request(
    xml: String,
    request: Request<Incoming>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let response = match route_request(xml, request).await {
        Ok(response) => response,
        Err(err) => text_response(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
    };
    Ok(response)
}

async fn route_request(
    xml: String,
    request: Request<Incoming>,
) -> anyhow::Result<Response<Full<Bytes>>> {
    match *request.method() {
        Method::POST => {
            let _ = request.into_body().collect().await?;
            Ok(Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "application/xml")
                .body(Full::new(Bytes::from(xml)))?)
        }
        _ => Ok(text_response(
            StatusCode::METHOD_NOT_ALLOWED,
            "method not allowed",
        )),
    }
}

fn text_response(status: StatusCode, message: impl Into<String>) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header("content-type", "text/plain; charset=utf-8")
        .body(Full::new(Bytes::from(message.into())))
        .expect("valid response")
}
