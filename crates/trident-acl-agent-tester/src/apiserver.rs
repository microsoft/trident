//! Minimal fake Kubernetes apiserver for the label-protocol tester.
//!
//! The real agent currently uses the poll-loop fallback rather than a watch
//! stream, so this test double focuses on GET/PATCH for a single Node object.

use std::{collections::BTreeMap, convert::Infallible, net::SocketAddr, sync::Arc};

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
use k8s_openapi::{api::core::v1::Node, apimachinery::pkg::apis::meta::v1::ObjectMeta};
use serde_json::{json, Value};
use tokio::{
    net::TcpListener,
    sync::{oneshot, RwLock},
    task::JoinHandle,
};

#[derive(Clone)]
pub struct NodeStore {
    node: Arc<RwLock<Node>>,
}

impl NodeStore {
    pub fn new(node: Node) -> Self {
        Self {
            node: Arc::new(RwLock::new(node)),
        }
    }

    pub async fn get(&self) -> Node {
        self.node.read().await.clone()
    }

    pub async fn patch_merge(&self, patch: Value) -> anyhow::Result<Node> {
        let mut node = self.node.write().await;
        apply_metadata_patch(&mut node, &patch);
        Ok(node.clone())
    }

    pub async fn patch_labels(&self, labels: BTreeMap<String, String>) -> anyhow::Result<Node> {
        let patch = json!({ "metadata": { "labels": labels } });
        self.patch_merge(patch).await
    }

    pub async fn patch_annotations(
        &self,
        annotations: BTreeMap<String, String>,
    ) -> anyhow::Result<Node> {
        let patch = json!({ "metadata": { "annotations": annotations } });
        self.patch_merge(patch).await
    }
}

pub struct ApiServerHandle {
    addr: SocketAddr,
    store: NodeStore,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl ApiServerHandle {
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn store(&self) -> NodeStore {
        self.store.clone()
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
    node_name: impl Into<String>,
    seed_labels: BTreeMap<String, String>,
) -> anyhow::Result<ApiServerHandle> {
    let listener = TcpListener::bind(listen)
        .await
        .with_context(|| format!("failed to bind fake apiserver at {listen}"))?;
    let addr = listener.local_addr()?;
    let store = NodeStore::new(seed_node(node_name.into(), seed_labels));
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
    let service_store = store.clone();

    let task = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => break,
                accept_result = listener.accept() => {
                    let Ok((stream, _)) = accept_result else { break; };
                    let io = TokioIo::new(stream);
                    let service_store = service_store.clone();
                    tokio::spawn(async move {
                        let service = service_fn(move |request| {
                            let service_store = service_store.clone();
                            async move { handle_request(service_store, request).await }
                        });
                        if let Err(err) = http1::Builder::new().serve_connection(io, service).await {
                            eprintln!("fake apiserver connection failed: {err}");
                        }
                    });
                }
            }
        }
    });

    Ok(ApiServerHandle {
        addr,
        store,
        shutdown: Some(shutdown_tx),
        task,
    })
}

pub fn seed_node(name: String, labels: BTreeMap<String, String>) -> Node {
    Node {
        metadata: ObjectMeta {
            name: Some(name),
            labels: Some(labels),
            annotations: Some(BTreeMap::new()),
            ..Default::default()
        },
        ..Default::default()
    }
}

pub fn node_from_seed_file(contents: &str) -> anyhow::Result<Node> {
    serde_json::from_str(contents).context("failed to parse node seed json")
}

async fn handle_request(
    store: NodeStore,
    request: Request<Incoming>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let response = match route_request(store, request).await {
        Ok(response) => response,
        Err(err) => text_response(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
    };
    Ok(response)
}

async fn route_request(
    store: NodeStore,
    request: Request<Incoming>,
) -> anyhow::Result<Response<Full<Bytes>>> {
    let path = request.uri().path().trim_matches('/');
    let parts: Vec<_> = path.split('/').collect();
    let watch_requested = request
        .uri()
        .query()
        .is_some_and(|query| query.contains("watch=true"));

    if parts.len() != 4 || parts[..3] != ["api", "v1", "nodes"] {
        return Ok(text_response(StatusCode::NOT_FOUND, "not found"));
    }

    match (request.method().clone(), watch_requested) {
        (Method::GET, true) => Ok(text_response(
            StatusCode::NOT_IMPLEMENTED,
            "watch=true is intentionally omitted; use the agent poll-loop fallback",
        )),
        (Method::GET, false) => Ok(json_response(StatusCode::OK, &store.get().await)?),
        (Method::PATCH, _) => {
            let bytes = request.into_body().collect().await?.to_bytes();
            let patch: Value =
                serde_json::from_slice(&bytes).context("invalid merge patch json")?;
            let updated = store.patch_merge(patch).await?;
            Ok(json_response(StatusCode::OK, &updated)?)
        }
        _ => Ok(text_response(
            StatusCode::METHOD_NOT_ALLOWED,
            "method not allowed",
        )),
    }
}

fn apply_metadata_patch(node: &mut Node, patch: &Value) {
    let metadata = patch.get("metadata").and_then(Value::as_object);
    let Some(metadata) = metadata else {
        return;
    };

    if let Some(labels) = metadata.get("labels") {
        merge_string_map(
            node.metadata.labels.get_or_insert_with(BTreeMap::new),
            labels,
        );
    }
    if let Some(annotations) = metadata.get("annotations") {
        merge_string_map(
            node.metadata.annotations.get_or_insert_with(BTreeMap::new),
            annotations,
        );
    }
}

fn merge_string_map(target: &mut BTreeMap<String, String>, patch: &Value) {
    let Some(entries) = patch.as_object() else {
        return;
    };

    for (key, value) in entries {
        if value.is_null() {
            target.remove(key);
        } else if let Some(value) = value.as_str() {
            target.insert(key.clone(), value.to_string());
        }
    }
}

fn json_response(
    status: StatusCode,
    value: &impl serde::Serialize,
) -> anyhow::Result<Response<Full<Bytes>>> {
    Ok(Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(serde_json::to_vec(value)?)))?)
}

fn text_response(status: StatusCode, message: impl Into<String>) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header("content-type", "text/plain; charset=utf-8")
        .body(Full::new(Bytes::from(message.into())))
        .expect("valid response")
}
