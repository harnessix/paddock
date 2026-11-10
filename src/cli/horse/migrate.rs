use anyhow::Result;
use paddock::env::HostEnv;

pub(super) fn run(env: &HostEnv, verify: bool, drop_plain: bool) -> Result<i32> {
    if verify {
        let nodes = paddock::horse::migrate::verify(env)?;
        println!("verified {nodes} nodes; the plaintext copy is retained as horse.plain");
    } else if drop_plain {
        eprintln!("Warning: removing the retained plaintext horse and registry copies after verification.");
        let nodes = paddock::horse::migrate::drop_plain(env)?;
        println!("removed the verified plaintext copy after checking {nodes} nodes");
    }
    Ok(0)
}
