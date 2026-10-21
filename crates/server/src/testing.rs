//! A disposable PostgreSQL database per test, shared by the unit tests and
//! the development labs (the `testing` feature).
//!
//! `HONORUM_TEST_POSTGRES` must name a disposable cluster and a role allowed
//! to create databases; `scripts/postgres.sh test cargo test` provides one.

/// A database created for one test and dropped when this value is.
pub(crate) struct TestDatabase {
    pub(crate) url: String,
    admin: String,
    name: String,
}

impl TestDatabase {
    pub(crate) async fn new() -> Self {
        let admin = std::env::var("HONORUM_TEST_POSTGRES").expect(
            "run backend tests with scripts/postgres.sh test cargo test, or set HONORUM_TEST_POSTGRES to a disposable cluster",
        );
        let name = format!("honorum_test_{}", crate::session::new_id());
        let (client, connection) =
            tokio_postgres::connect(&admin, tokio_postgres::NoTls).await.expect("connect to the test cluster");
        tokio::spawn(connection);
        client.batch_execute(&format!("CREATE DATABASE {name}")).await.expect("create the test database");
        let mut url = url::Url::parse(&admin).expect("a URL");
        url.set_path(&format!("/{name}"));
        Self { url: url.to_string(), admin, name }
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        let (admin, name) = (self.admin.clone(), self.name.clone());
        // Drop may run inside or outside a runtime; use a thread of its own.
        let _ = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a runtime");
            runtime.block_on(async move {
                if let Ok((client, connection)) = tokio_postgres::connect(&admin, tokio_postgres::NoTls).await {
                    tokio::spawn(connection);
                    let _ = client.batch_execute(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)")).await;
                }
            });
        })
        .join();
    }
}
