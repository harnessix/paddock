use super::*;

impl<'a> HarnessFactory<'a> {
    pub fn new(env: &'a HostEnv) -> Result<Self> {
        let mut profiles = BTreeMap::new();
        for (name, text) in BUILTIN {
            let p: Profile = toml::from_str(text)
                .with_context(|| format!("built-in profile '{name}' is invalid"))?;
            profiles.insert(p.name.clone(), (p, "built-in".to_string()));
        }
        let user_dir = env.paddock_config().join("harness.d");
        if let Ok(entries) = std::fs::read_dir(&user_dir) {
            let mut files: Vec<PathBuf> = entries
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|x| x == "toml"))
                .collect();
            files.sort();
            for file in files {
                let text = std::fs::read_to_string(&file)?;
                let p: Profile = toml::from_str(&text)
                    .with_context(|| format!("profile {} is invalid", file.display()))?;
                check_user_rules(&p, &file)?;
                profiles.insert(p.name.clone(), (p, file.display().to_string()));
            }
        }
        Ok(Self {
            env,
            profiles,
            common: Default::default(),
            found: Default::default(),
        })
    }

    /// How `create`, `program` or `adopt` found the harness `name`.
    pub fn found(&self, name: &str) -> Option<Found> {
        self.found.lock().ok()?.get(name).cloned()
    }

    pub(super) fn common(&self) -> &[PathBuf] {
        self.common
            .get_or_init(|| locate::common_dirs(self.env))
            .as_slice()
    }

    /// The program of a harness and its closure. A harness found outside
    /// PATH searches the interpreters of its script header in its folder
    /// and the common folders too.
    pub(super) fn closure(&self, p: &Profile) -> Result<(PathBuf, Vec<PathBuf>, Vec<PathBuf>)> {
        let (exe, found) = self.locate(p)?;
        // A harness on PATH needs no common folders (no `npm prefix -g`).
        let common = if found.by == FoundBy::Path {
            &[]
        } else {
            self.common()
        };
        let env = locate::closure_env(self.env, &found, common);
        let (mounts, dirs) = harness_closure(&env, &exe, &p.locate.helpers)?;
        if let Ok(mut map) = self.found.lock() {
            map.insert(p.name.clone(), found);
        }
        Ok((exe, mounts, dirs))
    }

    pub fn names(&self) -> Vec<String> {
        self.profiles.keys().cloned().collect()
    }

    pub fn profile(&self, name: &str) -> Option<&Profile> {
        self.profiles.get(name).map(|(p, _)| p)
    }

    /// The file of a user profile, None for a built-in one.
    pub fn user_file(&self, name: &str) -> Option<PathBuf> {
        self.profiles
            .get(name)
            .map(|(_, s)| PathBuf::from(s))
            .filter(|s| s.is_absolute())
    }

    pub fn create(&self, name: &str) -> Result<Harness> {
        let (profile, source) = self.profiles.get(name).ok_or_else(|| {
            anyhow!(
                "unknown harness '{name}'. Known harnesses: {}. Add a profile in {}",
                self.names().join(", "),
                self.env.paddock_config().join("harness.d").display()
            )
        })?;
        let (exe, install_mounts, path_dirs) = self.closure(profile)?;
        Ok(Harness {
            profile: profile.clone(),
            exe,
            install_mounts,
            path_dirs,
            source: source.clone(),
        })
    }

    /// The program of a harness, without its install closure: the first
    /// that `[locate]` finds. The VM guest asks it before a run starts.
    pub fn program(&self, name: &str) -> Result<PathBuf> {
        let (profile, _) = self
            .profiles
            .get(name)
            .ok_or_else(|| anyhow!("unknown harness '{name}'"))?;
        let (exe, found) = self.locate(profile)?;
        if let Ok(mut map) = self.found.lock() {
            map.insert(profile.name.clone(), found);
        }
        Ok(exe)
    }

    /// A harness for a profile that paddock builds itself, such as the
    /// workbench of a pasture.
    pub fn adopt(&self, profile: Profile, source: &str) -> Result<Harness> {
        let (exe, install_mounts, path_dirs) = self.closure(&profile)?;
        Ok(Harness {
            profile,
            exe,
            install_mounts,
            path_dirs,
            source: source.into(),
        })
    }

    /// Find the program of a profile. The order: `mise which` when mise is
    /// on PATH, the `commands` on PATH, the profile's `paths` and `dirs`,
    /// the common folders, and last `mise which` with a mise from a common
    /// folder.
    pub(super) fn locate(&self, p: &Profile) -> Result<(PathBuf, Found)> {
        let mut tried = Vec::new();
        let found = |by: FoundBy, path: &Path| Found {
            by,
            dir: path.parent().map(Path::to_path_buf).unwrap_or_default(),
        };
        if let (Some(tool), Some(mise)) = (&p.locate.mise, self.env.which("mise")) {
            tried.push(format!("mise which {tool}"));
            if let Some(path) = mise_which(&mise, tool) {
                return Ok((canonical(&path)?, found(FoundBy::Path, &path)));
            }
        }
        for cmd in &p.locate.commands {
            tried.push(format!("PATH lookup of {cmd}"));
            // The first match that is not a version-manager shim: Omarchy
            // puts mise's wrapper folder first on PATH.
            let found_at = self
                .env
                .path
                .iter()
                .filter(|d| d.is_absolute())
                .map(|d| d.join(cmd))
                .find(|p| is_executable(p) && !is_shim(p));
            if let Some(path) = found_at {
                return Ok((canonical(&path)?, found(FoundBy::Path, &path)));
            }
        }
        for raw in &p.locate.paths {
            let path = self.env.expand(raw, &self.env.home);
            tried.push(path.display().to_string());
            if is_executable(&path) {
                return Ok((canonical(&path)?, found(FoundBy::Profile, &path)));
            }
        }
        let dirs: Vec<PathBuf> = p
            .locate
            .dirs
            .iter()
            .map(|raw| self.env.expand(raw, &self.env.home))
            .collect();
        if let Some(path) = in_dirs(&dirs, &p.locate.commands, &mut tried) {
            return Ok((canonical(&path)?, found(FoundBy::Profile, &path)));
        }
        let common = self.common();
        let plain: Vec<PathBuf> = common.iter().filter(|d| !is_shim(d)).cloned().collect();
        if let Some(path) = in_dirs(&plain, &p.locate.commands, &mut tried) {
            return Ok((canonical(&path)?, found(FoundBy::Common, &path)));
        }
        // mise outside PATH: its shims re-enter mise, so `mise which` gives
        // the real program. Its shims folder is the one to put on PATH.
        if self.env.which("mise").is_none() {
            let mise = common
                .iter()
                .map(|d| d.join("mise"))
                .find(|m| is_executable(m) && !is_shim(m));
            let shims = locate::mise_shims(self.env);
            let tools = p
                .locate
                .mise
                .iter()
                .chain(p.locate.commands.iter().filter(|c| shims.join(c).exists()));
            if let Some(mise) = mise {
                for tool in tools {
                    tried.push(format!("{} which {tool}", mise.display()));
                    if let Some(path) = mise_which(&mise, tool) {
                        let dir = Found {
                            by: FoundBy::Common,
                            dir: shims.clone(),
                        };
                        return Ok((canonical(&path)?, dir));
                    }
                }
            }
        }
        bail!(
            "could not find the '{}' executable. Tried: {}",
            p.name,
            tried.join("; ")
        )
    }
}
