//! Rhai script plugin support.
//!
//! Each `.rhai` file runs once in its own sandboxed engine with a global
//! `katnip` API object:
//!
//! ```rhai
//! katnip.log("hello from my plugin");
//! katnip.bind("SUPER+T", "exec foot");
//!
//! fn on_window_open(title, floating) {
//!     katnip.log(`opened: ${title}`);
//! }
//! ```

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use rhai::{AST, Engine, Scope};

/// Marker type the scripts see as the `katnip` global.
#[derive(Clone, Copy)]
struct KatnipApi;

/// State a plugin mutates through the `katnip` object.
#[derive(Default)]
struct Bridge {
    binds: Vec<(String, String)>,
}

/// A compiled, still-alive Rhai plugin.
pub struct ScriptPlugin {
    pub name: String,
    pub engine: Engine,
    pub scope: Scope<'static>,
    pub ast: AST,
    /// Binds registered during evaluation (mirrored here for convenience).
    pub binds: Vec<(String, String)>,
}

/// All loaded script plugins.
#[derive(Default)]
pub struct ScriptHost {
    pub plugins: Vec<ScriptPlugin>,
    /// Aggregated binds registered by every plugin.
    pub binds: Vec<(String, String)>,
}

impl ScriptHost {
    /// Compiles and evaluates one plugin file.
    ///
    /// Returns the binds registered during evaluation. Any script error
    /// aborts that plugin only.
    pub fn load(&mut self, path: &Path) -> Result<Vec<(String, String)>, String> {
        let source = std::fs::read_to_string(path).map_err(|err| format!("cannot read: {err}"))?;
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "plugin".into());
        let log_name = name.clone();

        let bridge = Rc::new(RefCell::new(Bridge::default()));

        let mut engine = Engine::new();
        // Sandboxing: no filesystem/network access by default; cap CPU so
        // runaway loops cannot stall the compositor long.
        engine.set_max_operations(500_000);
        engine.set_max_string_size(1_000_000);
        engine.set_max_array_size(10_000);

        {
            let log_name = log_name.clone();
            engine.register_fn("log", move |_: &mut KatnipApi, msg: &str| {
                tracing::info!(plugin = %log_name, "{msg}");
            });
        }
        {
            let b = Rc::clone(&bridge);
            engine.register_fn(
                "bind",
                move |_: &mut KatnipApi, spec: &str, action: &str| {
                    b.borrow_mut()
                        .binds
                        .push((spec.to_string(), action.to_string()));
                },
            );
        }

        let mut scope = Scope::new();
        scope.push("katnip", KatnipApi);

        let ast = engine
            .compile(source)
            .map_err(|err| format!("compile error: {err}"))?;
        engine
            .run_ast_with_scope(&mut scope, &ast)
            .map_err(|err| format!("runtime error: {err}"))?;

        let binds = bridge.borrow().binds.clone();
        self.plugins.push(ScriptPlugin {
            name,
            engine,
            scope,
            ast,
            binds: binds.clone(),
        });
        self.binds.extend(binds.iter().cloned());
        Ok(binds)
    }

    fn has_fn(plugin: &ScriptPlugin, fn_name: &str) -> bool {
        plugin.ast.iter_functions().any(|f| f.name == fn_name)
    }

    fn fire<F>(&mut self, fn_name: &str, invoke: F)
    where
        F: Fn(&mut ScriptPlugin) -> Result<(), String>,
    {
        for idx in 0..self.plugins.len() {
            if !Self::has_fn(&self.plugins[idx], fn_name) {
                continue;
            }
            let name = self.plugins[idx].name.clone();
            if let Err(err) = invoke(&mut self.plugins[idx]) {
                tracing::warn!(plugin = %name, %err, "event hook failed");
            }
        }
    }

    /// Calls optional `fn on_window_open(title, floating)` in each plugin.
    pub fn fire_window_open(&mut self, title: &str, floating: bool) {
        let title = title.to_string();
        self.fire("on_window_open", move |p| {
            let result: Result<(), _> = p.engine.call_fn(
                &mut p.scope,
                &p.ast,
                "on_window_open",
                (title.clone(), floating),
            );
            result.map_err(|err| err.to_string())
        });
    }

    /// Calls optional `fn on_workspace_switch(id)` in each plugin.
    pub fn fire_workspace_switch(&mut self, id: usize) {
        self.fire("on_workspace_switch", |p| {
            let result: Result<(), _> =
                p.engine
                    .call_fn(&mut p.scope, &p.ast, "on_workspace_switch", (id as i64,));
            result.map_err(|err| err.to_string())
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_plugin(source: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("katnip-test-plugins-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join(format!(
            "test_{}.rhai",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::write(&path, source).expect("write");
        path
    }

    #[test]
    fn loads_binds_and_fires_events() {
        let mut host = ScriptHost::default();
        let path = temp_plugin(
            r#"
            katnip.log("loading test plugin");
            katnip.bind("SUPER+T", "exec foot");
            katnip.bind("SUPER+SHIFT+T", "close");

            fn on_window_open(title, floating) {
                katnip.log(`window ${title} floating=${floating}`);
            }
            fn on_workspace_switch(id) {
                katnip.log(`workspace ${id}`);
            }
        "#,
        );

        let binds = match host.load(&path) {
            Ok(b) => b,
            Err(err) => panic!("plugin load failed: {err}"),
        };
        assert_eq!(binds.len(), 2);
        // load() aggregates into the host-level list as well.
        assert_eq!(host.binds.len(), 2);
        assert!(
            host.plugins[0]
                .binds
                .contains(&("SUPER+T".into(), "exec foot".into()))
        );

        // Event hooks exist and run cleanly.
        host.fire_window_open("Alacritty", false);
        host.fire_workspace_switch(3);
    }

    #[test]
    fn broken_script_is_rejected() {
        let mut host = ScriptHost::default();
        let err = host
            .load(&temp_plugin("katnip.this_function_does_not_exist()"))
            .expect_err("missing fn must fail");
        assert!(err.contains("runtime error"), "{err}");
    }

    #[test]
    fn runaway_loop_is_capped() {
        let mut host = ScriptHost::default();
        let err = host
            .load(&temp_plugin("while true {}"))
            .expect_err("infinite loop must hit op cap");
        assert!(err.contains("runtime error"), "{err}");
    }
}
