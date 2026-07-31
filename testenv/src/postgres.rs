//! An embedded PostgreSQL server, replacing the `postgres` docker service.
//!
//! The server binaries are downloaded on first use and cached under
//! `~/.theseus/postgresql`; subsequent runs start from that cache.

use postgresql_embedded::{PostgreSQL, Settings};

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
