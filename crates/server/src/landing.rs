//! The site root shows the landing page (`/about.html`) to people new to
//! Honorum and the app to everyone else. `/app` records the choice in a
//! cookie; a session counts too, and each app response renews the cookie.
//! The Cloudflare Worker applies the same rule in
//! `deploy/cloudflare/src/landing.ts`.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderValue, Method, StatusCode, Uri, header};
use axum::response::Response;

use crate::Server;
use crate::assets::StaticSite;
use crate::session::{SESSION_COOKIE, cookie};

/// The cookie recording that this browser opened the app.
pub(crate) const APP_COOKIE: &str = "hn_app";
/// Its lifetime in seconds; browsers cap cookies at 400 days.
pub(crate) const APP_COOKIE_AGE: i64 = 400 * 24 * 60 * 60;

/// Reports whether a request gets the landing page: a GET or HEAD document
/// navigation to the bare root, without a query, by a browser with neither
/// the app cookie nor a session cookie. Without `Sec-Fetch-Mode`, a
/// navigation is a request that accepts `text/html`.
///
/// The app's own requests, such as the offline shell's, are not
/// navigations.
pub(crate) fn shows_landing(request: &Request) -> bool {
    let uri = request.uri();
    if uri.path() != "/" || uri.query().is_some_and(|q| !q.is_empty()) {
        return false;
    }
    if request.method() != Method::GET && request.method() != Method::HEAD {
        return false;
    }
    let headers = request.headers();
    let mode = headers.get("sec-fetch-mode").and_then(|v| v.to_str().ok()).unwrap_or_default();
    if !mode.is_empty() {
        if mode != "navigate" {
            return false;
        }
    } else if !headers.get(header::ACCEPT).and_then(|v| v.to_str().ok()).unwrap_or_default().contains("text/html") {
        return false;
    }
    [APP_COOKIE, SESSION_COOKIE].iter().all(|name| cookie(headers, name).is_none())
}

impl Server {
    /// The `Set-Cookie` value marking this browser as an app user.
    pub(crate) fn app_cookie(&self) -> HeaderValue {
        let secure = if self.cfg.base_url.starts_with("https://") { "; Secure" } else { "" };
        HeaderValue::from_str(&format!(
            "{APP_COOKIE}=1; Path=/; Max-Age={APP_COOKIE_AGE}; HttpOnly{secure}; SameSite=Lax"
        ))
        .expect("the app cookie is a valid header")
    }
}

/// Serves the web client around the root and `/app`: `/app` sets the
/// cookie and redirects (303) to `/` with `Cache-Control: no-store`; `/`
/// adds `Vary: Cookie` and serves the landing page or, renewing the cookie,
/// the app; everything else is a static file.
pub(crate) async fn site(s: Arc<Server>, files: Arc<StaticSite>, mut request: Request) -> Response {
    match request.uri().path() {
        "/app" => {
            let head = request.method() == Method::HEAD;
            // A GET redirect carries a short HTML link; HEAD gets no body.
            let body = if head { Body::empty() } else { Body::from("<a href=\"/\">See Other</a>.\n\n") };
            let mut response = Response::new(body);
            *response.status_mut() = StatusCode::SEE_OTHER;
            let h = response.headers_mut();
            h.append(header::SET_COOKIE, s.app_cookie());
            h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            h.insert(header::LOCATION, HeaderValue::from_static("/"));
            if !head {
                h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
            }
            response
        }
        "/" => {
            let landing = shows_landing(&request);
            if landing {
                *request.uri_mut() = Uri::from_static("/about.html");
            }
            let mut response = files.serve(request).await;
            let h = response.headers_mut();
            // `Vary: Cookie` precedes the file server's own values.
            let vary: Vec<HeaderValue> = h.get_all(header::VARY).iter().cloned().collect();
            h.remove(header::VARY);
            h.append(header::VARY, HeaderValue::from_static("Cookie"));
            for value in vary {
                h.append(header::VARY, value);
            }
            if !landing {
                h.append(header::SET_COOKIE, s.app_cookie());
            }
            response
        }
        _ => files.serve(request).await,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::borrow::Cow;
    use std::collections::HashMap;

    use axum::http::StatusCode;

    use crate::assets::WebAssets;
    use crate::test_support::test_server_with;

    /// Web assets held in memory.
    pub(crate) struct MapFs(pub HashMap<String, Vec<u8>>);

    impl MapFs {
        pub(crate) fn new(files: &[(&str, &[u8])]) -> Self {
            Self(files.iter().map(|(k, v)| (k.to_string(), v.to_vec())).collect())
        }
    }

    impl WebAssets for MapFs {
        fn names(&self) -> Vec<String> {
            self.0.keys().cloned().collect()
        }
        fn get(&self, path: &str) -> Option<Cow<'static, [u8]>> {
            self.0.get(path).map(|v| Cow::Owned(v.clone()))
        }
        fn modified(&self, _: &str) -> Option<i64> {
            None
        }
    }

    #[tokio::test]
    async fn root_shows_landing_to_new_visitors() {
        let files = MapFs::new(&[("index.html", b"app"), ("about.html", b"landing")]);
        let t = test_server_with(|cfg| cfg.base_url = "https://honorum.example".into()).await;
        // The router builds the site from `s.web`, which is fixed once the
        // server is shared, so this serves the site handler directly.
        let s = t.server.clone();
        let site = std::sync::Arc::new(crate::assets::StaticSite::new(std::sync::Arc::new(files)));
        let get = |target: &'static str, headers: Vec<(&'static str, &'static str)>| {
            let (s, site) = (s.clone(), site.clone());
            async move {
                let mut request = axum::http::Request::builder().method("GET").uri(target);
                for (k, v) in headers {
                    request = request.header(k, v);
                }
                let response = super::site(s, site, request.body(axum::body::Body::empty()).unwrap()).await;
                let (parts, body) = response.into_parts();
                let body = http_body_util::BodyExt::collect(body).await.unwrap().to_bytes();
                (parts, String::from_utf8_lossy(&body).into_owned())
            }
        };
        let navigate = vec![("sec-fetch-mode", "navigate"), ("accept", "text/html,*/*")];
        let with = |extra: (&'static str, &'static str)| {
            let mut headers = navigate.clone();
            headers.push(extra);
            headers
        };
        let cases = [
            ("new visitor", "/", navigate.clone(), "landing"),
            ("browser without fetch metadata", "/", vec![("accept", "text/html")], "landing"),
            ("opened the app", "/", with(("cookie", "hn_app=1")), "app"),
            ("signed in", "/", with(("cookie", "other=1; hn_session=token")), "app"),
            ("offline shell", "/", vec![("sec-fetch-mode", "cors"), ("accept", "*/*")], "app"),
            ("older offline shell", "/", vec![("accept", "*/*")], "app"),
            ("returning from Google", "/?google_connected=1", navigate.clone(), "app"),
            ("landing by name", "/about.html", navigate.clone(), "landing"),
        ];
        for (name, target, headers, want) in cases {
            let (parts, body) = get(target, headers).await;
            assert!(parts.status == StatusCode::OK && body == want, "{name}: {} {body:?}", parts.status);
            let cookie = parts.headers.get("set-cookie").and_then(|v| v.to_str().ok()).unwrap_or_default();
            let marked = cookie.contains("hn_app=1");
            if target != "/about.html" {
                assert_eq!(marked, want == "app", "{name}: cookie {cookie:?}");
            }
            if target == "/" {
                let vary: Vec<_> = parts.headers.get_all("vary").iter().filter_map(|v| v.to_str().ok()).collect();
                assert!(vary.join(",").contains("Cookie"), "{name}: vary {vary:?}");
            }
        }
        let (opened, _) = get("/app", navigate.clone()).await;
        let cookie = opened.headers.get("set-cookie").and_then(|v| v.to_str().ok()).unwrap_or_default();
        let location = opened.headers.get("location").and_then(|v| v.to_str().ok()).unwrap_or_default();
        assert!(
            opened.status == StatusCode::SEE_OTHER
                && location == "/"
                && cookie.contains("hn_app=1")
                && cookie.contains("Max-Age=34560000")
                && cookie.contains("Secure"),
            "open: {} {:?}",
            opened.status,
            opened.headers
        );
    }
}
