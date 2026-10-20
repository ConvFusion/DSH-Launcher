//! Exercise production lifecycle code with a mock Tauri runtime, real local
//! HTTP processes and isolated state files. Never touch a user's DSH service.

use super::*;
use crate::config::{Config, ServerConfig};
use crate::runtime::{DshInfo, NodeInfo};
use crate::state::AppState;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};
use tauri::Listener;
use tempfile::TempDir;

fn node() -> NodeInfo {
    static NODE: std::sync::OnceLock<NodeInfo> = std::sync::OnceLock::new();
    NODE.get_or_init(|| detector::detect_node().expect("lifecycle tests require Node.js"))
        .clone()
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

struct Fixture {
    dir: TempDir,
    port: u16,
    child: Option<Child>,
    managed: Option<(u32, ProcessIdentity)>,
}

impl Fixture {
    fn new(dsh: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let package = dir.path().join("node_modules/@deepseek-ai/dsh");
        std::fs::create_dir_all(package.join("lib")).unwrap();
        std::fs::write(
            package.join("package.json"),
            serde_json::json!({
                "name": if dsh { "@deepseek-ai/dsh" } else { "unrelated-http-service" },
                "version": "0.1.6", "type": "commonjs"
            })
            .to_string(),
        )
        .unwrap();
        let port = free_port();
        std::fs::write(package.join("lib/bin.js"), format!(r#"
if (process.argv.includes('--version')) {{ console.log('0.1.6'); }}
else {{
  const http = require('node:http');
  const i = process.argv.indexOf('--port');
  const port = i >= 0 ? Number(process.argv[i + 1]) : {port};
  http.createServer((req, res) => {{ res.writeHead(404); res.end('ordinary HTTP response'); }})
    .listen(port, '127.0.0.1', () => console.log('dsh web: http://127.0.0.1:' + port + '/?token=test-only'));
}}
"#)).unwrap();
        Self {
            dir,
            port,
            child: None,
            managed: None,
        }
    }

    fn script(&self) -> std::path::PathBuf {
        self.dir
            .path()
            .join("node_modules/@deepseek-ai/dsh/lib/bin.js")
    }

    fn state_path(&self) -> std::path::PathBuf {
        self.dir.path().join("state.json")
    }

    fn app(&self) -> tauri::App<MockRuntime> {
        let cfg = Config {
            server: ServerConfig {
                host: "127.0.0.1".into(),
                port: self.port,
            },
            dsh_dir: Some(self.dir.path().to_string_lossy().into()),
            open_browser_on_start: false,
            ..Config::default()
        };
        let mut state = AppState::new(cfg, false);
        state.proc = ProcessManager::with_state_path("127.0.0.1", self.port, self.state_path());
        mock_builder()
            .manage(state)
            .build(mock_context(noop_assets()))
            .unwrap()
    }

    async fn spawn(&mut self) -> u32 {
        let child = Command::new(node().path)
            .arg(self.script())
            .arg("web")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        self.child = Some(child);
        health::wait_ready("127.0.0.1", self.port, Duration::from_secs(5))
            .await
            .unwrap();
        pid
    }

    fn remember_managed(&mut self, manager: &ProcessManager) {
        let g = manager.inner.lock().unwrap();
        self.managed = g.pid.zip(g.identity.clone());
    }

    fn save_record(&self, pid: u32, start: Option<u64>) {
        config::write_process_state(
            &self.state_path(),
            &ProcessStateFile {
                pid,
                host: "127.0.0.1".into(),
                port: self.port,
                started_at: "test".into(),
                process_start_time: start,
                token_url: Some("http://127.0.0.1/?token=saved-test-token".into()),
            },
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some((pid, process)) = &self.managed {
            if identity::matches(*pid, process) {
                health::signal_kill(*pid);
            }
        }
    }
}

#[test]
fn stopping_without_a_pid_does_not_deadlock_status_emission() {
    // Use an OS thread and an external deadline: a blocking Mutex deadlock
    // cannot be interrupted by a timeout running on the same async thread.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let fixture = Fixture::new(true);
        let app = fixture.app();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let state = app.state::<AppState>();
            state.proc.stop(app.handle()).await.unwrap();
            state.proc.stop(app.handle()).await.unwrap();
            assert_eq!(state.proc.snapshot().state, ProcessState::Stopped);
        });
        tx.send(()).unwrap();
    });
    rx.recv_timeout(Duration::from_secs(15))
        .expect("stopping an already stopped service deadlocked");
}

#[tokio::test]
async fn arbitrary_http_is_not_adopted_or_stopped_even_with_a_stale_record() {
    let mut fixture = Fixture::new(false);
    let pid = fixture.spawn().await;
    fixture.save_record(pid, None);
    let app = fixture.app();
    let manager = &app.state::<AppState>().proc;
    assert!(!manager.try_adopt(app.handle()).await);
    assert_eq!(manager.snapshot().state, ProcessState::Stopped);
    manager.poll_status(app.handle()).await;
    assert_eq!(manager.snapshot().pid, None);
    manager.stop(app.handle()).await.unwrap();
    assert!(fixture
        .child
        .as_mut()
        .unwrap()
        .try_wait()
        .unwrap()
        .is_none());
    assert!(health::http_ok("127.0.0.1", fixture.port).await);
    let outcome = manager
        .start(
            app.handle(),
            &node(),
            &DshInfo {
                path: fixture.dir.path().into(),
                version: "0.1.6".into(),
            },
        )
        .await;
    assert_eq!(outcome.kind, "port_in_use");
    assert!(fixture
        .child
        .as_mut()
        .unwrap()
        .try_wait()
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn manually_started_dsh_can_still_be_adopted_and_stopped() {
    let mut fixture = Fixture::new(true);
    let pid = fixture.spawn().await;
    let app = fixture.app();
    let manager = &app.state::<AppState>().proc;
    assert!(manager.try_adopt(app.handle()).await);
    assert_eq!(manager.snapshot().pid, Some(pid));
    assert!(manager.snapshot().external);
    manager.stop(app.handle()).await.unwrap();
    fixture.child.as_mut().unwrap().wait().unwrap();
    assert_eq!(manager.snapshot().state, ProcessState::Stopped);
    manager.stop(app.handle()).await.unwrap();
}

#[tokio::test]
async fn restored_dsh_preserves_token_and_checks_process_creation_time() {
    let mut fixture = Fixture::new(true);
    let pid = fixture.spawn().await;
    let birth = identity::read(pid).unwrap().start_time;
    fixture.save_record(pid, Some(birth));
    let app = fixture.app();
    let manager = &app.state::<AppState>().proc;
    assert!(manager.try_adopt(app.handle()).await);
    assert!(!manager.snapshot().external);
    assert!(manager.snapshot().url.contains("saved-test-token"));
    // A recycled/stale PID record must not restore another instance's token.
    fixture.save_record(pid, Some(birth - 1));
    let other_app = fixture.app();
    let other = &other_app.state::<AppState>().proc;
    assert!(other.try_adopt(other_app.handle()).await); // genuine new external DSH
    assert!(other.snapshot().external);
    assert!(!other.snapshot().url.contains("saved-test-token"));
    other.stop(other_app.handle()).await.unwrap();
}

#[tokio::test]
async fn legacy_dsh_state_files_remain_compatible() {
    let mut fixture = Fixture::new(true);
    let pid = fixture.spawn().await;
    std::fs::write(
        fixture.state_path(),
        serde_json::json!({
            "pid": pid, "host": "127.0.0.1", "port": fixture.port, "started_at": "old",
            "token_url": "http://127.0.0.1/?token=legacy-test"
        })
        .to_string(),
    )
    .unwrap();
    let app = fixture.app();
    let manager = &app.state::<AppState>().proc;
    assert!(manager.try_adopt(app.handle()).await);
    assert!(!manager.snapshot().external);
    assert!(manager.snapshot().url.contains("legacy-test"));
    manager.stop(app.handle()).await.unwrap();
}

#[tokio::test]
async fn changed_process_identity_is_never_signaled() {
    let mut fixture = Fixture::new(true);
    let pid = fixture.spawn().await;
    let app = fixture.app();
    let manager = &app.state::<AppState>().proc;
    assert!(manager.try_adopt(app.handle()).await);
    manager
        .inner
        .lock()
        .unwrap()
        .identity
        .as_mut()
        .unwrap()
        .start_time -= 1;
    assert!(manager.stop(app.handle()).await.is_err());
    assert!(fixture
        .child
        .as_mut()
        .unwrap()
        .try_wait()
        .unwrap()
        .is_none());
    assert_eq!(manager.snapshot().pid, Some(pid));
}

#[tokio::test]
async fn managed_start_stop_double_stop_restart_and_tokens_still_work() {
    let mut fixture = Fixture::new(true);
    let app = fixture.app();
    let manager = &app.state::<AppState>().proc;
    let dsh = DshInfo {
        path: fixture.dir.path().into(),
        version: "0.1.6".into(),
    };
    for _ in 0..2 {
        let outcome = manager.start(app.handle(), &node(), &dsh).await;
        fixture.remember_managed(manager);
        assert!(outcome.ok, "start failed: {outcome:?}");
        assert!(!manager.snapshot().external);
        assert!(manager
            .wait_for_token_url()
            .await
            .unwrap()
            .contains("test-only"));
        assert!(config::read_process_state(&fixture.state_path())
            .unwrap()
            .process_start_time
            .is_some());
        assert_eq!(
            manager.start(app.handle(), &node(), &dsh).await.kind,
            "already_running"
        );
        manager.stop(app.handle()).await.unwrap();
        manager.stop(app.handle()).await.unwrap(); // the update → restart path
        assert_eq!(manager.snapshot().state, ProcessState::Stopped);
        assert!(!Path::new(&fixture.state_path()).exists());
    }
}

#[tokio::test]
async fn listener_pid_and_recorded_pid_must_belong_to_the_same_process() {
    let mut dsh = Fixture::new(true);
    dsh.spawn().await;
    let mut unrelated = Fixture::new(false);
    let wrong_pid = unrelated.spawn().await;
    dsh.save_record(wrong_pid, None);
    let app = dsh.app();
    let manager = &app.state::<AppState>().proc;
    assert!(manager.try_adopt(app.handle()).await);
    assert_ne!(manager.snapshot().pid, Some(wrong_pid));
    assert!(manager.snapshot().external);
    manager.stop(app.handle()).await.unwrap();
    assert!(unrelated
        .child
        .as_mut()
        .unwrap()
        .try_wait()
        .unwrap()
        .is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn stubborn_verified_dsh_is_still_force_stopped() {
    let mut fixture = Fixture::new(true);
    let script = std::fs::read_to_string(fixture.script()).unwrap();
    std::fs::write(
        fixture.script(),
        format!("process.on('SIGTERM', () => {{}});\n{script}"),
    )
    .unwrap();
    fixture.spawn().await;
    let app = fixture.app();
    let manager = &app.state::<AppState>().proc;
    assert!(manager.try_adopt(app.handle()).await);
    manager.stop(app.handle()).await.unwrap();
    fixture.child.as_mut().unwrap().wait().unwrap();
    assert_eq!(manager.snapshot().state, ProcessState::Stopped);
}

#[tokio::test]
async fn dsh_path_in_an_unrelated_scripts_arguments_is_not_a_dsh_entry() {
    let mut fixture = Fixture::new(true);
    let other_script = fixture.dir.path().join("ordinary-server.js");
    std::fs::copy(fixture.script(), &other_script).unwrap();
    fixture.child = Some(
        Command::new(node().path)
            .arg(other_script)
            .arg("web")
            .arg(fixture.script())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    health::wait_ready("127.0.0.1", fixture.port, Duration::from_secs(5))
        .await
        .unwrap();
    let app = fixture.app();
    let manager = &app.state::<AppState>().proc;
    assert!(!manager.try_adopt(app.handle()).await);
    manager.stop(app.handle()).await.unwrap();
    assert!(fixture
        .child
        .as_mut()
        .unwrap()
        .try_wait()
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn missing_identity_fails_closed_without_signaling_the_process() {
    let mut fixture = Fixture::new(true);
    fixture.spawn().await;
    let app = fixture.app();
    let manager = &app.state::<AppState>().proc;
    assert!(manager.try_adopt(app.handle()).await);
    manager.inner.lock().unwrap().identity = None;
    assert!(manager.stop(app.handle()).await.is_err());
    assert!(fixture
        .child
        .as_mut()
        .unwrap()
        .try_wait()
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn relative_entry_paths_and_explicit_web_profiles_remain_supported() {
    let mut fixture = Fixture::new(true);
    fixture.child = Some(
        Command::new(node().path)
            .current_dir(fixture.dir.path())
            .args([
                "node_modules/@deepseek-ai/dsh/lib/bin.js",
                "--profile",
                "web",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    health::wait_ready("127.0.0.1", fixture.port, Duration::from_secs(5))
        .await
        .unwrap();
    let app = fixture.app();
    let manager = &app.state::<AppState>().proc;
    assert!(manager.try_adopt(app.handle()).await);
    manager.stop(app.handle()).await.unwrap();
    fixture.child.as_mut().unwrap().wait().unwrap();
}

#[tokio::test]
async fn exit_during_status_emission_does_not_leave_a_stopping_state() {
    let mut fixture = Fixture::new(true);
    fixture.spawn().await;
    let app = fixture.app();
    let manager = &app.state::<AppState>().proc;
    assert!(manager.try_adopt(app.handle()).await);
    let child = Arc::new(Mutex::new(fixture.child.take().unwrap()));
    let listener_child = child.clone();
    let listener = app.listen("dsh://status", move |event| {
        let payload: serde_json::Value = serde_json::from_str(event.payload()).unwrap();
        if payload["process"]["state"] == "stopping" {
            let mut child = listener_child.lock().unwrap();
            let _ = child.kill();
            let _ = child.wait();
        }
    });
    assert!(manager.stop(app.handle()).await.is_err());
    assert_eq!(manager.snapshot().state, ProcessState::Error);
    assert!(!manager.inner.lock().unwrap().stop_requested);
    app.unlisten(listener);
    // A subsequent stop still completes cleanly after the child has exited.
    manager.stop(app.handle()).await.unwrap();
    assert_eq!(manager.snapshot().state, ProcessState::Stopped);
}

#[tokio::test]
async fn custom_node_executable_names_remain_supported() {
    let mut fixture = Fixture::new(true);
    let mut runtime = node();
    let runtime_dir = fixture.dir.path().join("custom-node");
    std::fs::create_dir_all(runtime_dir.join("bin")).unwrap();
    let custom = runtime_dir.join("bin").join(if cfg!(windows) {
        "custom-runtime.exe"
    } else {
        "custom-runtime"
    });
    #[cfg(target_os = "macos")]
    {
        // Homebrew's Node links libnode via @loader_path/../lib. Preserve
        // that runtime layout instead of testing a deliberately broken copy.
        let source = runtime.path.canonicalize().unwrap();
        let lib = source.parent().unwrap().parent().unwrap().join("lib");
        if lib.is_dir() {
            std::os::unix::fs::symlink(lib, runtime_dir.join("lib")).unwrap();
        }
    }
    std::fs::copy(&runtime.path, &custom).unwrap();
    runtime.path = custom;
    let app = fixture.app();
    let manager = &app.state::<AppState>().proc;
    let outcome = manager
        .start(
            app.handle(),
            &runtime,
            &DshInfo {
                path: fixture.dir.path().into(),
                version: "0.1.6".into(),
            },
        )
        .await;
    fixture.remember_managed(manager);
    assert!(outcome.ok, "custom Node failed: {outcome:?}");
    manager.stop(app.handle()).await.unwrap();
    assert_eq!(manager.snapshot().state, ProcessState::Stopped);
}
