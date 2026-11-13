use super::*;

/// Build a spec for a located harness.
pub fn build(env: &HostEnv, opts: &Options) -> Result<Built> {
    if let Some(name) = &opts.pasture {
        if opts.child.is_some() {
            bail!("a sandbox that another one starts cannot join a pasture");
        }
        crate::pasture::admit(env, name, &opts.harness)?;
    }
    let factory = HarnessFactory::new(env)?;
    let harness = factory.create(&opts.harness)?;
    build_with(env, opts, harness)
}
