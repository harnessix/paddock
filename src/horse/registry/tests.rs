use super::*;
use crate::horse::store::Store;
use std::cmp::Ordering;
use std::path::Path;

fn artifact(trait_value: &str) -> Vec<u8> {
    let mut d = dna::default_for_run();
    d.origin.created_ms = 1;
    d.traits.insert(
        "tone".into(),
        dna::Trait::new(trait_value, dna::Provenance::Default),
    );
    d.encode().into_bytes()
}

fn manifest(name: &str, version: &str, bytes: &[u8]) -> Manifest {
    Manifest {
        name: name.into(),
        version: version.into(),
        hash: artifact_hash(bytes),
        parents: vec![Hash::of(b"parent")],
        created_ms: 1_700_000_000_000,
        author: None,
        signature: None,
    }
}

fn index(dir: &Path, name: &str) -> Box<dyn Transport> {
    Box::new(FileIndex::new(name, dir.join(name)))
}

fn trusting(org: &str, k: &Keypair, name: Option<&str>) -> Trust {
    Trust {
        require_signature: true,
        keys: vec![TrustedKey {
            org: org.into(),
            public: k.public_hex(),
            name: name.map(String::from),
        }],
    }
}

fn r(s: &str) -> Reference {
    s.parse().unwrap()
}

fn err<T>(r: Result<T>) -> String {
    match r {
        Ok(_) => panic!("expected an error"),
        Err(e) => format!("{e:#}"),
    }
}

#[test]
fn references_round_trip() {
    let h = Hash::of(b"x");
    for s in [
        "registry://acme/review@1.2.0",
        "registry://acme/review@latest",
        "registry://a1/b-2@1.0.0-rc.1",
        &format!("registry://acme/review@sha256:{h}"),
    ] {
        assert_eq!(r(s).to_string(), s);
    }
    assert_eq!(r("registry://acme/x@latest").version, Version::Latest);
    assert_eq!(
        r(&format!("registry://acme/x@sha256:{h}")).version,
        Version::Hash(h)
    );
    assert_eq!(r("registry://acme/x@2").version, Version::Exact("2".into()));
}

#[test]
fn bad_references_name_the_bad_part() {
    for (s, part) in [
        ("acme/x@1", "must start with registry://"),
        ("https://acme/x@1", "must start with registry://"),
        ("registry://acme/x", "no version"),
        ("registry://acme@1", "no name"),
        ("registry://Acme/x@1", "the org \"Acme\""),
        ("registry:///x@1", "the org \"\""),
        ("registry://-acme/x@1", "the org \"-acme\""),
        ("registry://acme/x_y@1", "the name \"x_y\""),
        ("registry://acme/x/y@1", "the name \"x/y\""),
        ("registry://acme/x@", "the version \"\""),
        ("registry://acme/x@1/2", "the version \"1/2\""),
        ("registry://acme/x@..", "the version \"..\""),
        ("registry://acme/x@1 0", "the version \"1 0\""),
        ("registry://acme/x@sha256:abc", "the hash \"abc\""),
        (
            &format!("registry://acme/x@sha256:{}", "A".repeat(64)),
            "the hash",
        ),
    ] {
        let e = err(s.parse::<Reference>());
        assert!(e.contains(part), "{s}: {e}");
    }
}

#[test]
fn versions_compare_by_number_then_text() {
    use reference::compare_versions as c;
    assert_eq!(c("1.10.0", "1.9.0"), Ordering::Greater);
    assert_eq!(c("2", "10"), Ordering::Less);
    assert_eq!(c("1.0", "1.0.1"), Ordering::Less);
    assert_eq!(c("1.0.0", "1.0.0"), Ordering::Equal);
    assert_eq!(c("1.0.beta", "1.0.alpha"), Ordering::Greater);
    assert_eq!(c("1.0.9", "1.0.alpha"), Ordering::Less);
    assert_eq!(
        c("1.99999999999999999999999999999999999999999", "1.2"),
        Ordering::Greater
    );
    assert_ne!(c("1.01", "1.1"), Ordering::Equal);
}

#[test]
fn canonical_is_sorted_stable_and_leaves_out_the_signature() {
    let b = artifact("calm");
    let mut m = manifest("acme/x", "1.0.0", &b);
    let c = String::from_utf8(m.canonical()).unwrap();
    assert!(!c.contains(' ') && !c.contains('\n'), "{c}");
    assert!(!c.contains("signature"), "{c}");
    let keys: Vec<&str> = ["author", "created_ms", "hash", "name", "parents", "version"].into();
    let pos: Vec<usize> = keys
        .iter()
        .map(|k| c.find(&format!("\"{k}\"")).unwrap())
        .collect();
    assert!(pos.windows(2).all(|w| w[0] < w[1]), "{c}");
    // The same manifest in another field order parses to the same bytes.
    let shuffled = format!(
        r#"{{"version":"1.0.0","parents":["{}"],"name":"acme/x","hash":"{}","created_ms":1700000000000}}"#,
        m.parents[0], m.hash
    );
    assert_eq!(
        Manifest::parse(shuffled.as_bytes()).unwrap().canonical(),
        m.canonical()
    );
    m.signature = Some("ab".repeat(64));
    assert_eq!(m.canonical().as_slice(), c.as_bytes());
    assert_eq!(Manifest::parse(&m.encode()).unwrap(), m);
}

#[test]
fn manifests_refuse_unknown_fields_and_bad_names() {
    let b = artifact("calm");
    let m = manifest("acme/x", "1.0.0", &b);
    let mut v: serde_json::Value = serde_json::from_slice(&m.encode()).unwrap();
    v["extra"] = 1.into();
    assert!(Manifest::parse(&serde_json::to_vec(&v).unwrap()).is_err());
    let bad = manifest("Acme/x", "1.0.0", &b);
    assert!(err(Manifest::parse(&bad.encode())).contains("ORG/NAME"));
}

#[test]
fn a_signature_verifies_until_one_byte_changes() {
    let k = trust::new_keypair();
    let b = artifact("calm");
    let mut m = manifest("acme/x", "1.0.0", &b);
    m.sign(&k);
    assert_eq!(m.author.as_deref(), Some(k.public_hex().as_str()));
    assert!(m.verify(&k.public_hex()));
    assert!(!m.verify(&trust::new_keypair().public_hex()));
    assert!(!m.verify("not hex"));
    // Change one byte of the encoded manifest: the version.
    let text = String::from_utf8(m.encode())
        .unwrap()
        .replace("1.0.0", "1.0.1");
    let changed = Manifest::parse(text.as_bytes()).unwrap();
    assert!(!changed.verify(&k.public_hex()));
    let mut changed = m.clone();
    changed.created_ms += 1;
    assert!(!changed.verify(&k.public_hex()));
}

#[test]
fn allows_gives_each_verdict() {
    let k = trust::new_keypair();
    let b = artifact("calm");
    let mut m = manifest("acme/x", "1.0.0", &b);
    let t = trusting("acme", &k, Some("release"));
    assert_eq!(t.allows(&m), Verdict::Unsigned);
    m.sign(&k);
    assert_eq!(t.allows(&m), Verdict::Trusted("release".into()));
    assert_eq!(
        trusting("acme", &k, None).allows(&m),
        Verdict::Trusted(k.public_hex())
    );
    // The key is trusted, but for another org.
    assert_eq!(
        trusting("other", &k, None).allows(&m),
        Verdict::UntrustedKey
    );
    assert_eq!(Trust::default().allows(&m), Verdict::UntrustedKey);
    let mut bad = m.clone();
    bad.hash = Hash::of(b"other");
    assert_eq!(t.allows(&bad), Verdict::BadSignature);
}

#[test]
fn trust_and_keys_load_and_save_private() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("trust.toml");
    let t = Trust::load(&p).unwrap();
    assert!(t.require_signature && t.keys.is_empty());
    let k = trust::new_keypair();
    let t = trusting("acme", &k, Some("release"));
    t.save(&p).unwrap();
    assert_eq!(crate::privfs::tests::mode(&p), 0o600);
    assert_eq!(Trust::load(&p).unwrap(), t);
    std::fs::write(
        &p,
        "require_signature = false\n[[key]]\norg = \"acme\"\npublic = \"zz\"\n",
    )
    .unwrap();
    assert!(err(Trust::load(&p)).contains("64 lower-case hex"));
    std::fs::write(&p, "require_signatures = false\n").unwrap();
    assert!(Trust::load(&p).is_err());

    let kp = d.path().join("key.toml");
    trust::save_keypair(&k, &kp).unwrap();
    assert_eq!(crate::privfs::tests::mode(&kp), 0o600);
    let again = trust::load_keypair(&kp).unwrap();
    assert_eq!(again.public_hex(), k.public_hex());
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(&kp, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(err(trust::load_keypair(&kp)).contains("chmod 600"));
    std::fs::set_permissions(&kp, std::fs::Permissions::from_mode(0o600)).unwrap();
    let text = std::fs::read_to_string(&kp).unwrap();
    let other = trust::new_keypair().public_hex();
    std::fs::write(&kp, text.replace(&k.public_hex(), &other)).unwrap();
    assert!(err(trust::load_keypair(&kp)).contains("does not belong"));
}

#[test]
fn a_keypair_never_shows_its_secret() {
    let d = tempfile::tempdir().unwrap();
    let k = trust::new_keypair();
    let kp = d.path().join("key.toml");
    trust::save_keypair(&k, &kp).unwrap();
    let text = std::fs::read_to_string(&kp).unwrap();
    let secret = text
        .lines()
        .find_map(|l| l.strip_prefix("secret = "))
        .unwrap()
        .trim_matches('"');
    assert_eq!(secret.len(), 64);
    assert!(!format!("{k:?}").contains(secret));
}

#[test]
fn the_registry_hash_is_the_store_hash() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("store")).unwrap();
    let b = artifact("calm");
    let parsed = dna::parse(std::str::from_utf8(&b).unwrap()).unwrap();
    assert_eq!(dna::put(&s, &parsed).unwrap(), artifact_hash(&b));
}

#[test]
fn file_index_put_get_versions_latest() {
    let d = tempfile::tempdir().unwrap();
    let t = index(d.path(), "team");
    assert_eq!(t.versions("acme", "x").unwrap(), Vec::<String>::new());
    assert!(t.manifest("acme", "x", "1.0.0").unwrap().is_none());
    for v in ["1.9.0", "1.10.0", "1.2.0"] {
        let b = artifact(v);
        let m = manifest("acme/x", v, &b);
        t.put(&m, &b).unwrap();
        assert_eq!(t.manifest("acme", "x", v).unwrap(), Some(m.clone()));
        assert_eq!(t.object(&m.hash).unwrap(), Some(b));
    }
    // A leftover temporary file is not a version.
    std::fs::write(d.path().join("team/manifests/acme/x/.tmp-1-ab"), "x").unwrap();
    assert_eq!(
        t.versions("acme", "x").unwrap(),
        ["1.2.0", "1.9.0", "1.10.0"]
    );
    let got = resolve(
        &[t],
        &r("registry://acme/x@latest"),
        &Trust {
            require_signature: false,
            keys: vec![],
        },
    )
    .unwrap();
    assert_eq!(got.manifest.version, "1.10.0");
    assert_eq!(got.bytes, artifact("1.10.0"));
    assert_eq!(got.signed_by, None);
}

#[test]
fn file_index_refuses_paths_out_of_its_folder() {
    let d = tempfile::tempdir().unwrap();
    let t = index(d.path(), "team");
    assert!(t.manifest("..", "x", "1").is_err());
    assert!(t.manifest("acme", "x", "..").is_err());
    assert!(t.versions("acme", "../x").is_err());
    let b = artifact("calm");
    assert!(t.put(&manifest("acme/x", "../../1", &b), &b).is_err());
}

#[test]
fn a_version_is_immutable() {
    let d = tempfile::tempdir().unwrap();
    let t = index(d.path(), "team");
    let k = trust::new_keypair();
    let b = artifact("calm");
    let mut m = manifest("acme/x", "1.0.0", &b);
    m.sign(&k);
    t.put(&m, &b).unwrap();
    let path = d.path().join("team/manifests/acme/x/1.0.0.json");
    let before = std::fs::read(&path).unwrap();
    // The same manifest again: no change.
    t.put(&m, &b).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), before);
    // Another artifact under the same version: refused, both hashes named.
    let b2 = artifact("loud");
    let m2 = manifest("acme/x", "1.0.0", &b2);
    let e = err(t.put(&m2, &b2));
    assert!(
        e.contains(m.hash.as_str()) && e.contains(m2.hash.as_str()),
        "{e}"
    );
    // The same artifact with other metadata: refused too.
    let unsigned = manifest("acme/x", "1.0.0", &b);
    assert!(err(t.put(&unsigned, &b)).contains("other metadata"));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    // The refused artifact may stay as an object, by its own hash; only
    // the manifest names a version.
    assert_eq!(t.manifest("acme", "x", "1.0.0").unwrap(), Some(m));
}

#[test]
fn put_refuses_an_artifact_that_does_not_match() {
    let d = tempfile::tempdir().unwrap();
    let t = index(d.path(), "team");
    let b = artifact("calm");
    let m = manifest("acme/x", "1.0.0", &b);
    let e = err(t.put(&m, &artifact("loud")));
    assert!(e.contains(m.hash.as_str()), "{e}");
    let junk = b"not = [ dna".to_vec();
    assert!(t.put(&manifest("acme/x", "1.0.0", &junk), &junk).is_err());
    assert!(!d.path().join("team/manifests").exists());
}

#[test]
fn resolve_takes_the_first_index_that_has_it() {
    let d = tempfile::tempdir().unwrap();
    let k = trust::new_keypair();
    let first = index(d.path(), "first");
    let second = index(d.path(), "second");
    let (b1, b2) = (artifact("one"), artifact("two"));
    let mut m1 = manifest("acme/x", "1.0.0", &b1);
    let mut m2 = manifest("acme/x", "1.0.0", &b2);
    m1.sign(&k);
    m2.sign(&k);
    second.put(&m2, &b2).unwrap();
    let mut m3 = manifest("acme/y", "2.0.0", &b2);
    m3.sign(&k);
    second.put(&m3, &b2).unwrap();
    let indexes = vec![first, second];
    let pol = trusting("acme", &k, Some("release"));
    let got = resolve(&indexes, &r("registry://acme/x@1.0.0"), &pol).unwrap();
    assert_eq!(
        (got.index.as_str(), got.bytes.as_slice()),
        ("second", b2.as_slice())
    );
    indexes[0].put(&m1, &b1).unwrap();
    let got = resolve(&indexes, &r("registry://acme/x@1.0.0"), &pol).unwrap();
    assert_eq!(got.index, "first");
    assert_eq!(got.bytes, b1);
    assert_eq!(got.signed_by.as_deref(), Some("release"));
    // Only the second has acme/y.
    assert_eq!(
        resolve(&indexes, &r("registry://acme/y@latest"), &pol)
            .unwrap()
            .index,
        "second"
    );
    let e = err(resolve(&indexes, &r("registry://acme/z@1.0.0"), &pol));
    assert!(e.contains("not in any index (first, second)"), "{e}");
    assert!(err(resolve(&[], &r("registry://acme/z@1.0.0"), &pol)).contains("no registry index"));
}

#[test]
fn resolve_refuses_by_trust_policy() {
    let d = tempfile::tempdir().unwrap();
    let t = index(d.path(), "team");
    let k = trust::new_keypair();
    let b = artifact("calm");
    t.put(&manifest("acme/unsigned", "1.0.0", &b), &b).unwrap();
    let mut signed = manifest("acme/signed", "1.0.0", &b);
    signed.sign(&k);
    t.put(&signed, &b).unwrap();
    let ix = vec![t];
    let strict = trusting("acme", &k, None);
    let loose = Trust {
        require_signature: false,
        ..Trust::default()
    };

    let e = err(resolve(&ix, &r("registry://acme/unsigned@1.0.0"), &strict));
    assert!(e.contains("not signed"), "{e}");
    assert_eq!(
        resolve(&ix, &r("registry://acme/unsigned@1.0.0"), &loose)
            .unwrap()
            .signed_by,
        None
    );
    // Signed by a key the policy does not list: refused under either.
    for p in [&Trust::default(), &loose] {
        let e = err(resolve(&ix, &r("registry://acme/signed@1.0.0"), p));
        assert!(e.contains("not trusted for acme"), "{e}");
    }
    // A manifest changed after signing: refused under either.
    let path = d.path().join("team/manifests/acme/signed/1.0.0.json");
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, text.replace("1700000000000", "1700000000001")).unwrap();
    for p in [
        &strict,
        &Trust {
            require_signature: false,
            ..strict.clone()
        },
    ] {
        let e = err(resolve(&ix, &r("registry://acme/signed@1.0.0"), p));
        assert!(e.contains("does not verify"), "{e}");
    }
}

#[test]
fn resolve_by_hash_is_unsigned() {
    let d = tempfile::tempdir().unwrap();
    let ix = vec![index(d.path(), "a"), index(d.path(), "b")];
    let b = artifact("calm");
    ix[1].put(&manifest("acme/x", "1.0.0", &b), &b).unwrap();
    let h = artifact_hash(&b);
    let rr = r(&format!("registry://acme/x@sha256:{h}"));
    assert!(err(resolve(&ix, &rr, &Trust::default())).contains("not signed"));
    let got = resolve(
        &ix,
        &rr,
        &Trust {
            require_signature: false,
            keys: vec![],
        },
    )
    .unwrap();
    assert_eq!(got.index, "b");
    assert_eq!(got.manifest.name, "acme/x");
    assert_eq!(got.manifest.version, format!("sha256:{h}"));
    assert_eq!(got.bytes, b);
    let missing = r(&format!("registry://acme/x@sha256:{}", Hash::of(b"none")));
    assert!(resolve(&ix, &missing, &Trust::default()).is_err());
}

#[test]
fn resolve_refuses_a_changed_or_unparsable_artifact() {
    let d = tempfile::tempdir().unwrap();
    let t = index(d.path(), "team");
    let k = trust::new_keypair();
    let pol = trusting("acme", &k, None);
    let b = artifact("calm");
    let mut m = manifest("acme/x", "1.0.0", &b);
    m.sign(&k);
    t.put(&m, &b).unwrap();
    let obj = d.path().join("team/objects").join(m.hash.as_str());
    let other = artifact("loud");
    std::fs::write(&obj, &other).unwrap();
    let ix = vec![t];
    let e = err(resolve(&ix, &r("registry://acme/x@1.0.0"), &pol));
    assert!(
        e.contains(m.hash.as_str()) && e.contains(artifact_hash(&other).as_str()),
        "{e}"
    );

    // Bytes that hash right but are not DNA: written behind put's back.
    let junk = b"not = [ dna".to_vec();
    let mut mj = manifest("acme/junk", "1.0.0", &junk);
    mj.sign(&k);
    std::fs::write(d.path().join("team/objects").join(mj.hash.as_str()), &junk).unwrap();
    let dir = d.path().join("team/manifests/acme/junk");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("1.0.0.json"), mj.encode()).unwrap();
    let e = err(resolve(&ix, &r("registry://acme/junk@1.0.0"), &pol));
    assert!(e.contains("not a DNA"), "{e}");

    // A manifest missing its object.
    std::fs::remove_file(&obj).unwrap();
    let e = err(resolve(&ix, &r("registry://acme/x@1.0.0"), &pol));
    assert!(e.contains("not the artifact"), "{e}");
}

#[test]
fn resolve_refuses_a_manifest_in_the_wrong_place() {
    let d = tempfile::tempdir().unwrap();
    let t = index(d.path(), "team");
    let k = trust::new_keypair();
    let b = artifact("calm");
    let mut m = manifest("evil/x", "1.0.0", &b);
    m.sign(&k);
    t.put(&m, &b).unwrap();
    // A signed manifest of evil/x copied to the place of acme/x.
    let dir = d.path().join("team/manifests/acme/x");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("2.0.0.json"), m.encode()).unwrap();
    let pol = Trust {
        require_signature: true,
        keys: vec![
            TrustedKey {
                org: "acme".into(),
                public: k.public_hex(),
                name: None,
            },
            TrustedKey {
                org: "evil".into(),
                public: k.public_hex(),
                name: None,
            },
        ],
    };
    let e = err(resolve(&[t], &r("registry://acme/x@2.0.0"), &pol));
    assert!(e.contains("in its place"), "{e}");
}

#[test]
fn open_indexes_reads_the_config() {
    let d = tempfile::tempdir().unwrap();
    let ix = open_indexes(d.path()).unwrap();
    assert_eq!(ix.len(), 1);
    assert_eq!(ix[0].name(), "local");
    let cfg = d.path().join("registry/registry.toml");
    std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();
    let team = d.path().join("team");
    std::fs::write(
        &cfg,
        format!(
            "[[index]]\nname = \"team\"\npath = \"{}\"\n\n[[index]]\nname = \"other\"\npath = \"/srv/dna\"\n",
            team.display()
        ),
    )
    .unwrap();
    let ix = open_indexes(d.path()).unwrap();
    let names: Vec<&str> = ix.iter().map(|t| t.name()).collect();
    assert_eq!(names, ["team", "other"]);
    let b = artifact("calm");
    ix[0].put(&manifest("acme/x", "1.0.0", &b), &b).unwrap();
    assert!(team.join("manifests/acme/x/1.0.0.json").exists());

    for (text, want) in [
        ("", "lists no index"),
        (
            "[[index]]\nname = \"a\"\npath = \"rel\"\n",
            "must be absolute",
        ),
        (
            "[[index]]\nname = \"a\"\npath = \"/a\"\n[[index]]\nname = \"a\"\npath = \"/b\"\n",
            "used twice",
        ),
        ("[[index]]\nname = \" \"\npath = \"/a\"\n", "empty name"),
        ("[[index]]\nname = \"a\"\n", "not valid"),
    ] {
        std::fs::write(&cfg, text).unwrap();
        let e = err(open_indexes(d.path()));
        assert!(e.contains(want), "{text:?}: {e}");
    }
}
