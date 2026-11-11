use super::*;
use std::os::unix::net::UnixListener;

/// Serve requests on `socket` until the process exits.
pub fn serve(socket: &Path, broker: Arc<Broker>) -> Result<()> {
    let _ = std::fs::remove_file(socket);
    if let Some(d) = socket.parent() {
        crate::privfs::dir(d)?;
    }
    let listener = UnixListener::bind(socket)
        .with_context(|| format!("cannot listen on {}", socket.display()))?;
    log::debug!("serving host requests for sandbox {}", broker.ctx.id);
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let b = broker.clone();
            std::thread::spawn(move || {
                if let Err(e) = b.serve_conn(conn) {
                    log::debug!("connection failed: {e:#}");
                }
            });
        }
    });
    Ok(())
}
