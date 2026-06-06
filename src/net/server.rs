use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::TcpListener;
use tracing::{error, info};

use crate::net::connection::run_connection;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolKind {
    Status,
    Login,
    Game,
}

#[derive(Debug, Clone)]
pub struct ServiceInfo {
    pub protocol_id: u8,
    pub checksummed: bool,
    pub server_sends_first: bool,
    pub kind: ProtocolKind,
}

pub struct ServicePort {
    pub port: u16,
    pub services: Arc<Vec<ServiceInfo>>,
}

pub struct ServiceManager {
    ports: Vec<ServicePort>,
}

impl ServiceManager {
    pub fn new() -> Self {
        Self { ports: Vec::new() }
    }

    pub fn add_service(&mut self, port: u16, info: ServiceInfo) {
        if let Some(sp) = self.ports.iter_mut().find(|sp| sp.port == port) {
            Arc::get_mut(&mut sp.services)
                .expect("services not yet shared")
                .push(info);
        } else {
            self.ports.push(ServicePort {
                port,
                services: Arc::new(vec![info]),
            });
        }
    }

    pub fn start(self) {
        for sp in self.ports {
            tokio::spawn(accept_loop(sp.port, sp.services));
        }
    }
}

impl Default for ServiceManager {
    fn default() -> Self {
        Self::new()
    }
}

async fn accept_loop(port: u16, services: Arc<Vec<ServiceInfo>>) {
    let addr: SocketAddr = ([0u8, 0, 0, 0], port).into();
    let listener = match TcpListener::bind(addr).await {
        Ok(l) => {
            info!(port, "listening");
            l
        }
        Err(e) => {
            error!(port, %e, "failed to bind");
            return;
        }
    };

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let services = Arc::clone(&services);
                tokio::spawn(run_connection(stream, services));
            }
            Err(e) => {
                error!(%e, "accept error");
            }
        }
    }
}
