//! `paddock doctor --json`: the prerequisites screen of the Mac launcher
//! (`paddock::doctor::macvm::json`). On another system the checks of the
//! Mac fail with their lines, so the output keeps its shape.

use anyhow::Result;
use paddock::doctor::macvm::json;
use paddock::env::HostEnv;

pub(crate) fn run(env: &HostEnv) -> Result<i32> {
    let (view, facts) = json::collect(env);
    let j = json::build(&view, &facts);
    println!("{j}");
    Ok(if j["ok"] == true { 0 } else { 1 })
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_output_of_this_machine_parses() {
        let t = tempfile::tempdir().unwrap();
        let env = paddock::env::HostEnv::from_parts(
            t.path().join("home"),
            "demo".into(),
            501,
            20,
            [(
                "XDG_DATA_HOME".to_string(),
                t.path().join("data").display().to_string(),
            )]
            .into_iter()
            .collect(),
        );
        let (view, facts) = paddock::doctor::macvm::json::collect(&env);
        let text = paddock::doctor::macvm::json::build(&view, &facts).to_string();
        let j: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(j["v"], 1);
        assert_eq!(j["checks"].as_array().unwrap().len(), 6);
        if !cfg!(target_os = "macos") {
            assert_eq!(j["ok"], false, "off macOS the Mac checks fail");
            assert_eq!(j["chip"]["name"], serde_json::Value::Null);
        }
    }
}
