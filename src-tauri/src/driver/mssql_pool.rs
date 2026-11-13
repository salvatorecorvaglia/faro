//! Keep pool and query sessions on one Tiberius API version.
use crate::error::{FaroError, Result};
use tokio::net::TcpStream;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

type Client = tiberius::Client<Compat<TcpStream>>;
pub struct ConnectionManager {
    config: tiberius::Config,
}
impl ConnectionManager {
    pub fn new(config: tiberius::Config) -> Self {
        Self { config }
    }
    pub(super) async fn open(mut config: tiberius::Config) -> Result<Client> {
        for redirect in 0..2 {
            let tcp = TcpStream::connect(config.get_addr())
                .await
                .map_err(|error| {
                    FaroError::Connection(format!("could not reach the server: {error}"))
                })?;
            tcp.set_nodelay(true)
                .map_err(|error| FaroError::Connection(error.to_string()))?;
            match tiberius::Client::connect(config.clone(), tcp.compat_write()).await {
                Ok(client) => return Ok(client),
                Err(tiberius::error::Error::Routing { host, port }) if redirect == 0 => {
                    config.host(host);
                    config.port(port);
                }
                Err(error) => return Err(FaroError::Connection(error.to_string())),
            }
        }
        Err(FaroError::Connection(
            "SQL Server redirected more than once".into(),
        ))
    }
}
impl bb8::ManageConnection for ConnectionManager {
    type Connection = Client;
    type Error = FaroError;
    async fn connect(&self) -> Result<Client> {
        Self::open(self.config.clone()).await
    }
    async fn is_valid(&self, client: &mut Client) -> Result<()> {
        client
            .simple_query("SELECT 1")
            .await
            .map_err(|error| FaroError::Connection(error.to_string()))?
            .into_results()
            .await
            .map_err(|error| FaroError::Connection(error.to_string()))?;
        Ok(())
    }
    fn has_broken(&self, _client: &mut Client) -> bool {
        false
    }
}
