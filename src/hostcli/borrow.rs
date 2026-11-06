//! `paddock-host borrow` and `return`: the help, and the path of a copy in
//! this sandbox (borrow.rs does the work on the host).

use std::path::PathBuf;

pub const HELP: &str = r#"paddock-host borrow and return: change a host folder outside the project.

  paddock-host borrow HOST_FOLDER   copy the folder to /run/paddock/borrow/NAME
  paddock-host return NAME          write the changes of the copy back

HOST_FOLDER must be absolute, and its last part is no symlink. borrow prints
the path of the copy (on macOS in $PADDOCK_BORROW_DIR). Change the copy as you
like. The original does not change. return shows the user every added, changed
and deleted file, and writes them back on approval. It refuses when the
original changed after the borrow: borrow it again and redo the changes. A
return never writes git config, hooks or pointer files, gives new files no
execute bit, and refuses symlinks that point outside the folder. After a
return the copy is gone. Copies that are not returned are deleted when the
sandbox ends. Protected paths (~/.ssh, cloud credentials, browser profiles and
the others that fetch refuses) and folders that hold one are refused. Without
reflinks the limit is [borrow] max of the user configuration (default 1G)."#;

/// The path of the borrowed copy in this sandbox: below the folder that
/// `PADDOCK_BORROW_DIR` names (Seatbelt on macOS), else below
/// `/run/paddock/borrow`.
pub fn copy_path(result: &serde_json::Value) -> PathBuf {
    crate::borrow::copy_path(result, std::env::var_os(crate::borrow::ENV).as_deref())
}
