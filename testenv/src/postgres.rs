//! An embedded PostgreSQL server, replacing the `postgres` docker service.
//!
//! The server binaries are downloaded on first use and cached under
//! `~/.theseus/postgresql`; subsequent runs start from that cache.

use postgresql_embedded::{PostgreSQL, Settings, VersionReq};

/// The PostgreSQL release the tests run against.
///
/// Pinned so every run uses the same server. Releases from 18.6.1 on bundle
/// their shared libraries and timezone data, so they run on hosts without a
/// matching system `libxml2`.
///
/// CI keys its binary cache on this file, so changing the version refreshes it.
const POSTGRES_VERSION: &str = "=18.7.0";

/// A running PostgreSQL server with one database created.
///
/// Stops the server and removes its data directory when dropped, so tests must
/// hold it for as long as they need it.
pub struct TestPostgres {
    _server: PostgreSQL,
    url: String,
}

impl TestPostgres {
    /// Starts a server on an ephemeral port and creates `database`.
    pub async fn start(database: &str) -> TestPostgres {
        let settings = Settings {
            version: VersionReq::parse(POSTGRES_VERSION).expect("valid version requirement"),
            temporary: true,
            ..Default::default()
        };
        let mut server = PostgreSQL::new(settings);
        server
            .setup()
            .await
            .expect("failed to install the embedded postgres");
        server
            .start()
            .await
            .expect("failed to start the embedded postgres");
        server
            .create_database(database)
            .await
            .expect("failed to create the test database");

        let url = server.settings().url(database);
        TestPostgres {
            _server: server,
            url,
        }
    }

    /// The `postgresql://` connection URL for the created database.
    pub fn url(&self) -> &str {
        &self.url
    }
}
