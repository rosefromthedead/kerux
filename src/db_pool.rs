use crossbeam::queue::ArrayQueue;
use futures::compat::Future01CompatExt;
use pg::{Client, NoTls};
use std::sync::Arc;

pub struct DbPool {
    db_address: String,
    queue: Arc<ArrayQueue<Client>>,
}

pub struct ClientGuard {
    queue: Arc<ArrayQueue<Client>>,
    inner: Option<Client>,
}

impl DbPool {
    pub fn new(db_address: String, cap: usize) -> Self {
        DbPool {
            db_address,
            queue: Arc::new(ArrayQueue::new(cap)),
        }
    }

    pub async fn get_client(&self) -> Result<ClientGuard, pg::Error> {
        if let Ok(client) = self.queue.pop() {
            return Ok(ClientGuard {
                queue: Arc::clone(&self.queue),
                inner: Some(client),
            });
        } else {
            let (client, conn) = pg::connect(&*self.db_address, NoTls).compat().await.map_err(|e| {
                tracing::warn!("{}", e); e
            })?;
            runtime::spawn(conn.compat());
            return Ok(ClientGuard {
                queue: Arc::clone(&self.queue),
                inner: Some(client),
            });
        }
    }
}

impl std::fmt::Debug for DbPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DbPool {{ db_address: \"{}\", ... }}", self.db_address)?;
        Ok(())
    }
}

impl std::ops::Deref for ClientGuard {
    type Target = Client;

    fn deref(&self) -> &Client {
        self.inner.as_ref().unwrap()
    }
}

impl std::ops::DerefMut for ClientGuard {
    fn deref_mut(&mut self) -> &mut Client {
        self.inner.as_mut().unwrap()
    }
}

impl Drop for ClientGuard {
    fn drop(&mut self) {
        let _ = self.queue.push(self.inner.take().unwrap());
    }
}
