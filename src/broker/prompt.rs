//! Safe access to a sandbox-named prompt file.

use super::*;

/// Read a prompt file that the sandbox names. The project is
/// sandbox-writable: the path must stay inside it, no symlink is followed, a
/// FIFO or a device is never opened, and the size is limited.
pub fn read_prompt(project: &Path, raw: &str) -> Result<Vec<u8>> {
    let p = Path::new(raw);
    let rel = if p.is_absolute() {
        p.strip_prefix(project)
            .with_context(|| {
                format!(
                    "the prompt file must be inside the project {}",
                    project.display()
                )
            })?
            .to_path_buf()
    } else {
        p.to_path_buf()
    };
    let root = crate::safefs::Root::open(project)?;
    let bytes = root
        .read(&rel, crate::spawn::MAX_PROMPT)
        .with_context(|| format!("cannot read the prompt file {raw}"))?
        .with_context(|| format!("the prompt file {raw} does not exist"))?;
    if bytes.iter().all(u8::is_ascii_whitespace) {
        bail!("the prompt file {raw} is empty");
    }
    Ok(bytes)
}
