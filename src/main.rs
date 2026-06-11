use axum::{Json, Router, extract::Query, routing::get};
use serde::{Deserialize, Serialize};
use tower_http::cors::{Any, CorsLayer};

#[derive(Deserialize)]
struct InvoiceReq {
    lnurl: String, // LNURL bech32 or lightning address
    amount_msat: u64,
    comment: Option<String>,
}

#[derive(Serialize)]
struct InvoiceRes {
    pr: String,
}

async fn lnurl_invoice(Query(q): Query<InvoiceReq>) -> Result<Json<InvoiceRes>, String> {
    use lnurl::Builder;
    use lnurl::LnUrlResponse::LnUrlPayResponse;
    use lnurl::lightning_address::LightningAddress;
    use std::str::FromStr;

    let client = Builder::default()
        .build_async()
        .map_err(|e| e.to_string())?;

    let response = if q.lnurl.contains('@') {
        let addr = LightningAddress::from_str(&q.lnurl).map_err(|e| e.to_string())?;
        client
            .make_request(addr.lnurl().url.as_str())
            .await
            .map_err(|e| e.to_string())?
    } else {
        client
            .make_request(&q.lnurl)
            .await
            .map_err(|e| e.to_string())?
    };

    let LnUrlPayResponse(pay) = response else {
        return Err("not an LNURL-pay response".into());
    };

    let invoice = client
        .get_invoice(&pay, q.amount_msat, None, q.comment.as_deref())
        .await
        .map_err(|e| e.to_string())?;

    Ok(Json(InvoiceRes {
        pr: invoice.invoice().to_string(),
    }))
}

fn app() -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        .route("/api/lnurl/invoice", get(lnurl_invoice))
        .layer(cors)
}

#[tokio::main]
async fn main() {
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app()).await.unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use serde_json::{Value, json};
    use tokio::task::JoinHandle;
    use tower::ServiceExt;

    #[tokio::test]
    async fn invoice_endpoint_fetches_lnurl_pay_invoice() {
        let (pay_url, mock_server) = spawn_mock_lnurl_pay_server().await;
        let encoded_pay_url = pay_url.replace(':', "%3A").replace('/', "%2F");

        let response = app()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/lnurl/invoice?lnurl={encoded_pay_url}&amount_msat=123000&comment=hello"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        mock_server.abort();

        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json, json!({ "pr": "lnbc123test" }));
    }

    async fn spawn_mock_lnurl_pay_server() -> (String, JoinHandle<()>) {
        #[derive(Deserialize)]
        struct CallbackReq {
            amount: u64,
            comment: Option<String>,
            nostr: Option<String>,
        }

        async fn callback(Query(q): Query<CallbackReq>) -> Result<Json<Value>, StatusCode> {
            if q.amount != 123000
                || q.comment.as_deref() != Some("hello")
                || q.nostr.as_deref().is_some()
            {
                return Err(StatusCode::BAD_REQUEST);
            }

            Ok(Json(json!({ "pr": "lnbc123test" })))
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let callback_url = format!("{base_url}/callback");

        let lnurl_pay_response = move || {
            let callback_url = callback_url.clone();
            async move {
                Json(json!({
                    "tag": "payRequest",
                    "callback": callback_url,
                    "minSendable": 1000,
                    "maxSendable": 1000000,
                    "metadata": "[[\"text/plain\",\"test lnurl proxy\"]]",
                    "commentAllowed": 20
                }))
            }
        };

        let app = Router::new()
            .route("/lnurl-pay", get(lnurl_pay_response))
            .route("/callback", get(callback));

        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        (format!("{base_url}/lnurl-pay"), server)
    }
}
