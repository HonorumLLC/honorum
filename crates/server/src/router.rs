//! The HTTP routes.
//!
//! Every response passes through [`crate::api::secure`] (security headers
//! and cache policy) and [`crate::api::admit`] (request counting, write
//! origin checks, rate limits, timing). Bodies are bounded per handler with
//! [`crate::api::read_json`], so axum's default body limit is off.

use std::sync::Arc;

use axum::Router;
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};

use crate::assets::StaticSite;
use crate::{
    Server, accounts, admin, api, assets, auth, canvas_account, canvas_classes, catalog_api, docs, events, feeds,
    google, landing, lms, public, reminders, shares, status, streaks, watches,
};

/// Builds the service: every API route, the static site when the server
/// carries web assets, and the shared middleware.
pub fn router(s: Arc<Server>) -> Router {
    let admin_routes = Router::new()
        .route("/admin", get(admin::handle_admin_html))
        .route("/api/admin/metrics", get(admin::handle_admin_json))
        .route_layer(middleware::from_fn_with_state(s.clone(), admin::require_admin));

    let mut app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        // Catalog files rebuild on demand, so readiness needs only the database.
        .route("/readyz", get(readyz))
        .route("/api/config", get(public::handle_config))
        .route("/api/status", get(status::handle_status_json))
        .route("/status", get(status::handle_status_html))
        .merge(admin_routes)
        .route("/api/watches", post(watches::handle_create_watch))
        .route("/api/watches/{id}", delete(watches::handle_delete_watch))
        .route("/api/push/retarget", post(watches::handle_retarget))
        .route("/api/sync/{id}", get(shares::handle_sync_get).put(shares::handle_sync_put))
        .route("/api/lms/feed", post(lms::handle_feed))
        .route("/api/lms/canvas", post(lms::handle_canvas))
        .route("/api/lms/canvas/complete", post(lms::handle_canvas_complete))
        .route("/api/lms/canvas/classes", post(canvas_classes::handle_canvas_classes))
        .route("/api/reminders/{id}", put(reminders::handle_put_reminder).delete(reminders::handle_delete_reminder))
        .route("/catalog/{inst}/{file}", get(assets::handle_catalog))
        .route("/api/seats", get(catalog_api::handle_seats))
        // Accounts (optional): sign-in, devices and per-document sync.
        .route("/api/auth/code", post(auth::handle_auth_code))
        .route("/api/auth/verify", post(auth::handle_auth_verify))
        .route("/api/auth/logout", post(auth::handle_logout))
        .route("/api/me/streak", get(streaks::handle_streak))
        .route("/api/me/activity", post(streaks::handle_activity))
        .route(
            "/api/me/canvas",
            get(canvas_account::handle_account_canvas)
                .put(canvas_account::handle_connect_canvas)
                .delete(canvas_account::handle_disconnect_canvas),
        )
        .route("/api/me/canvas/refresh", post(canvas_account::handle_refresh_canvas))
        .route("/api/me/canvas/complete", post(canvas_account::handle_account_canvas_complete))
        .route("/api/me/canvas/classes", post(canvas_classes::handle_account_canvas_classes))
        .route("/api/me", get(accounts::handle_me).patch(accounts::handle_me_patch).delete(accounts::handle_me_delete))
        .route("/api/me/devices/{id}", delete(accounts::handle_delete_device))
        .route("/api/me/push", put(accounts::handle_me_push))
        .route("/api/me/watches", get(accounts::handle_me_watches))
        .route("/api/me/feeds", get(feeds::handle_feeds).put(feeds::handle_add_feed))
        .route("/api/me/feeds/{id}", delete(feeds::handle_delete_feed))
        .route("/api/me/feeds/refresh", post(feeds::handle_refresh_feeds))
        .route("/api/me/sync", post(docs::handle_sync_docs))
        .route(
            "/api/me/google",
            get(google::handle_google).patch(google::handle_google_patch).delete(google::handle_google_disconnect),
        )
        .route("/api/me/google/start", post(google::handle_google_start))
        .route("/api/me/google/finish", post(google::handle_google_finish))
        .route("/api/me/google/events", get(google::events::handle_google_events))
        .route("/api/google/callback", get(google::handle_google_callback))
        .route("/api/google/notify", post(google::events::handle_google_notify))
        .route("/api/me/history/{id}", get(docs::handle_history))
        // EventSource cannot set headers; the account it expects travels as a query.
        .route("/api/me/events", get(events::handle_events).layer(middleware::from_fn(account_from_query)));

    app = match s.web.clone() {
        Some(files) => {
            let site = Arc::new(StaticSite::new(files));
            app.fallback(move |State(s): State<Arc<Server>>, request: Request| {
                let site = site.clone();
                async move { landing::site(s, site, request).await }
            })
        }
        None => app.fallback(not_found),
    };

    app.layer(DefaultBodyLimit::disable())
        .layer(middleware::from_fn_with_state(s.clone(), api::admit))
        .layer(middleware::from_fn(api::secure))
        .with_state(s)
}

/// `GET /readyz`: "ready" once the database answers, else 503 "starting".
async fn readyz(State(s): State<Arc<Server>>) -> Response {
    if s.db.ping().await.is_err() {
        return text(StatusCode::SERVICE_UNAVAILABLE, "starting\n");
    }
    "ready".into_response()
}

/// The plain-text 404 for unknown paths.
async fn not_found() -> Response {
    text(StatusCode::NOT_FOUND, "404 page not found\n")
}

/// A plain-text response: `text/plain; charset=utf-8` and
/// `X-Content-Type-Options: nosniff`.
fn text(status: StatusCode, body: &'static str) -> Response {
    let mut response = (status, body).into_response();
    let h = response.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
    h.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    response
}

/// Copies `?account=` into `X-Honorum-Account` when the header is absent.
async fn account_from_query(mut request: Request, next: Next) -> Response {
    if !request.headers().contains_key("x-honorum-account") {
        let account = request.uri().query().and_then(|q| {
            url::form_urlencoded::parse(q.as_bytes()).find(|(k, _)| k == "account").map(|(_, v)| v.into_owned())
        });
        if let Some(account) = account
            && let Ok(value) = HeaderValue::from_str(&account)
        {
            request.headers_mut().insert("x-honorum-account", value);
        }
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use crate::test_support::test_server;

    #[tokio::test]
    async fn health_and_readiness_answer() {
        let t = test_server().await;
        let client = t.client();
        let health = client.get("/healthz").await;
        assert_eq!((health.status.as_u16(), health.text().as_str()), (200, "ok"));
        let ready = client.get("/readyz").await;
        assert_eq!((ready.status.as_u16(), ready.text().as_str()), (200, "ready"));
        assert_eq!(ready.header("x-frame-options"), "DENY");
        let missing = client.get("/nothing").await;
        assert_eq!(missing.status.as_u16(), 404);
    }
}
