//! HTTPS beside the plain http address, for the two things browsers only allow on a secure page:
//! the microphone (voice messages) and GPS (chat's "My location"). There is no internet to get a
//! real certificate from, so the server makes its own once and keeps it (DESIGN §19). Browsers
//! warn about it the first time; the chat page explains that before sending anyone there.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::{self, pki_types};

const CERT: &str = "https-cert.der";
const KEY: &str = "https-key.der";

/// The server's certificate from `dir`, made there on first use.
pub fn config(dir: &Path) -> anyhow::Result<Arc<rustls::ServerConfig>> {
    let (cp, kp) = (dir.join(CERT), dir.join(KEY));
    let (cert, key) = match (std::fs::read(&cp), std::fs::read(&kp)) {
        (Ok(c), Ok(k)) => (c, k),
        _ => {
            let ck = rcgen::generate_simple_self_signed(vec!["survivalpedia.local".into(), "localhost".into()])?;
            let (c, k) = (ck.cert.der().to_vec(), ck.key_pair.serialize_der());
            std::fs::create_dir_all(dir)?;
            write(&kp, &k)?;
            write(&cp, &c)?;
            (c, k)
        }
    };
    let key = pki_types::PrivateKeyDer::try_from(key).map_err(|e| anyhow::anyhow!("{KEY}: {e}"))?;
    let mut cfg = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(vec![pki_types::CertificateDer::from(cert)], key)?;
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(cfg))
}

fn write(p: &Path, b: &[u8]) -> std::io::Result<()> {
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, b)?;
    std::fs::rename(tmp, p)
}

/// A TLS listener for axum::serve. Handshakes run in their own tasks, so one slow or stalled
/// client (or a browser that gives up at the certificate warning) never holds up the others.
pub struct Listener {
    rx: tokio::sync::mpsc::Receiver<(tokio_rustls::server::TlsStream<TcpStream>, SocketAddr)>,
    addr: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl Listener {
    pub fn new(tcp: TcpListener, cfg: Arc<rustls::ServerConfig>) -> std::io::Result<Listener> {
        let addr = tcp.local_addr()?;
        let acceptor = TlsAcceptor::from(cfg);
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let task = tokio::spawn(async move {
            loop {
                let (s, a) = match tcp.accept().await {
                    Ok(x) => x,
                    Err(_) => {
                        // out of file handles and the like: wait, don't spin
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                };
                let (acceptor, tx) = (acceptor.clone(), tx.clone());
                tokio::spawn(async move {
                    if let Ok(Ok(t)) = tokio::time::timeout(Duration::from_secs(10), acceptor.accept(s)).await {
                        let _ = tx.send((t, a)).await;
                    }
                });
            }
        });
        Ok(Listener { rx, addr, task })
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        // stop accepting (and free the port) when the server stops
        self.task.abort();
    }
}

impl axum::serve::Listener for Listener {
    type Io = tokio_rustls::server::TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.rx.recv().await {
            Some(x) => x,
            // the accept task only ends when aborted, which happens on drop
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> std::io::Result<SocketAddr> {
        Ok(self.addr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn makes_a_certificate_once_and_keeps_it() {
        let d = std::env::temp_dir().join(format!("sp-tls-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        config(&d).unwrap();
        let first = std::fs::read(d.join(CERT)).unwrap();
        config(&d).unwrap();
        assert_eq!(std::fs::read(d.join(CERT)).unwrap(), first);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
