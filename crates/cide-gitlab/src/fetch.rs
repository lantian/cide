//! Use the same installed Git and proxy environment as Cide's ordinary network operations.
//! Keep account credentials in this child's environment, never arguments or repository config.
use crate::Result;
use base64::{engine::general_purpose::STANDARD, Engine};
use cide_core::{child_env, proxy::ProxyEnv};
use cide_ipc::ProxySettings;
use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

fn command(root: &Path, sha: &str, url: &str, authorization: &str, proxy: &ProxyEnv) -> Command {
    let mut command = Command::new("git");
    child_env::prepare_command(&mut command);
    child_env::arm(&mut command);
    proxy.apply(&mut command);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command
        .current_dir(root)
        .args([
            "-c",
            "credential.helper=",
            "-c",
            "core.askPass=",
            "-c",
            "http.followRedirects=false",
            "-c",
            "http.lowSpeedLimit=1",
            "-c",
            "http.lowSpeedTime=30",
            "fetch",
            "--no-tags",
            "--no-recurse-submodules",
            "--no-progress",
            "--no-auto-maintenance",
            "--depth=1",
            "--",
            url,
            sha,
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        // An exact URL scope prevents a configured URL rewrite from receiving this token.
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", format!("http.{url}.extraHeader"))
        .env("GIT_CONFIG_VALUE_0", "")
        .env("GIT_CONFIG_KEY_1", format!("http.{url}.extraHeader"))
        .env("GIT_CONFIG_VALUE_1", authorization)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    // Debug tracing can include HTTP headers. It must not copy the injected credential to
    // inherited trace files; normal failures still arrive through the captured stderr pipe.
    for name in std::env::vars_os().map(|(name, _)| name) {
        if name
            .to_str()
            .is_some_and(|s| s.starts_with("GIT_TRACE") || s == "GIT_CURL_VERBOSE")
        {
            command.env_remove(name);
        }
    }
    // A Cide launched from a Git hook must not redirect this disposable fetch into the
    // launching repository or its object database.
    for name in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_SHALLOW_FILE",
        "GIT_NAMESPACE",
        "GIT_CONFIG_PARAMETERS",
        "GIT_ASKPASS",
        "SSH_ASKPASS",
    ] {
        command.env_remove(name);
    }
    command
}

fn redact(text: &str, token: &str, proxy: &ProxyEnv) -> String {
    let mut text = proxy.scrub_output(text);
    if !token.is_empty() {
        for secret in [
            token.to_string(),
            crate::encode(token),
            STANDARD.encode(format!("oauth2:{token}")),
        ] {
            text = text.replace(&secret, "[redacted]");
        }
    }
    text
}

pub(super) fn fetch(
    root: &Path,
    sha: &str,
    url: &str,
    token: &str,
    proxy: &ProxySettings,
    stopped: &AtomicBool,
    cancelled: &AtomicBool,
) -> Result<()> {
    let proxy = ProxyEnv::for_target(proxy, proxy.scope.git);
    let authorization = format!(
        "Authorization: Basic {}",
        STANDARD.encode(format!("oauth2:{token}"))
    );
    let command = command(root, sha, url, &authorization, &proxy);
    run(
        command,
        token,
        &proxy,
        stopped,
        cancelled,
        Duration::from_secs(300),
    )
}

fn run(
    mut command: Command,
    token: &str,
    proxy: &ProxyEnv,
    stopped: &AtomicBool,
    cancelled: &AtomicBool,
    timeout: Duration,
) -> Result<()> {
    let is_cancelled = || stopped.load(Ordering::Acquire) || cancelled.load(Ordering::Acquire);
    if is_cancelled() {
        return Err("Review checkout cancelled".into());
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("Cannot start Git for review checkout: {e}"))?;
    let mut stderr = child.stderr.take().expect("fetch stderr is piped");
    let reader = std::thread::spawn(move || {
        let mut output = Vec::new();
        let mut chunk = [0; 4096];
        // Drain the entire pipe, keeping bounded diagnostics so a noisy remote cannot block
        // the fetch or consume unlimited memory. No captured output is written to a log.
        while let Ok(n) = stderr.read(&mut chunk) {
            if n == 0 {
                break;
            }
            let retain = n.min(64 * 1024 - output.len());
            output.extend_from_slice(&chunk[..retain]);
        }
        output
    });
    let started = Instant::now();
    let result = loop {
        if is_cancelled() {
            break Err("Review checkout cancelled".to_string());
        }
        if started.elapsed() >= timeout {
            break Err("Review fetch timed out after five minutes".to_string());
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => break Err(format!("Cannot wait for review fetch: {e}")),
        }
    };
    if result.is_err() {
        // Git's HTTPS and index-pack helpers must stop before removing the workspace.
        #[cfg(unix)]
        child_env::signal_group(child.id(), libc::SIGKILL);
        let _ = child.kill();
        let _ = child.wait();
    }
    let output = reader.join().unwrap_or_default();
    let status = result?;
    if status.success() {
        return Ok(());
    }
    let detail = redact(&String::from_utf8_lossy(&output), token, proxy);
    Err(format!("Review fetch failed ({status}): {}", detail.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, io::Write, net::TcpListener, path::PathBuf, sync::Arc};

    struct TestRepo(PathBuf);
    impl TestRepo {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("cide-review-fetch-{}", uuid::Uuid::new_v4()));
            git2::Repository::init_bare(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TestRepo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn settings() -> ProxySettings {
        let mut settings = ProxySettings::default();
        settings.scope.git = cide_ipc::ProxyTarget::Direct;
        settings
    }

    fn server(
        respond: impl FnOnce(&mut std::net::TcpStream) + Send + 'static,
    ) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/project.git", listener.local_addr().unwrap());
        let thread = std::thread::spawn(move || {
            let start = Instant::now();
            let mut stream = loop {
                if let Ok((stream, _)) = listener.accept() {
                    break stream;
                }
                assert!(
                    start.elapsed() < Duration::from_secs(5),
                    "Git did not connect"
                );
                std::thread::sleep(Duration::from_millis(10));
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") {
                assert_eq!(stream.read(&mut byte).unwrap(), 1);
                request.push(byte[0]);
            }
            respond(&mut stream);
            String::from_utf8(request).unwrap()
        });
        (url, thread)
    }

    #[test]
    fn authenticated_fetch_reports_actual_failure_and_refuses_redirects() {
        let root = TestRepo::new();
        for response in [
            "HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            "HTTP/1.1 302 Found\r\nLocation: https://other.invalid/repo.git\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        ] {
            let (url, thread) = server(move |stream| stream.write_all(response.as_bytes()).unwrap());
            let token = "test-review-token";
            let error = fetch(&root.0, &"a".repeat(40), &url, token, &settings(), &AtomicBool::new(false), &AtomicBool::new(false)).unwrap_err();
            let request = thread.join().unwrap();
            assert!(request.contains(&format!("Authorization: Basic {}", STANDARD.encode(format!("oauth2:{token}")))), "request must carry the account credential");
            assert!(error.contains(if response.contains("403") { "403" } else { "302" }), "{error}");
            assert!(!error.contains(token));
            assert!(!error.contains("Could not resolve host: other.invalid"), "must not follow the redirect");
        }
        // Fetch authentication is transient, including after a failure.
        let config = fs::read_to_string(root.0.join("config")).unwrap();
        assert!(!config.contains("extraHeader"));
        assert!(!config.contains("test-review-token"));
    }

    #[test]
    fn closing_review_cancels_blocked_network_fetch_and_its_helpers() {
        let root = TestRepo::new();
        let cancelled = Arc::new(AtomicBool::new(false));
        let signal = cancelled.clone();
        let (url, thread) = server(move |stream| {
            signal.store(true, Ordering::Release);
            // Killing only `git fetch` leaves git-remote-http alive holding this connection.
            assert_eq!(
                stream.read(&mut [0]).unwrap(),
                0,
                "HTTP helper must stop too"
            );
        });
        let started = Instant::now();
        let error = fetch(
            &root.0,
            &"a".repeat(40),
            &url,
            "test-token",
            &settings(),
            &AtomicBool::new(false),
            &cancelled,
        )
        .unwrap_err();
        assert!(error.contains("cancelled"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(5));
        thread.join().unwrap();
    }

    #[test]
    fn diagnostics_redact_credentials_and_arguments_never_contain_them() {
        let token = "test-token/+";
        let encoded = STANDARD.encode(format!("oauth2:{token}"));
        let header = format!("Authorization: Basic {encoded}");
        let proxy = ProxyEnv::default();
        let text = redact(
            &format!("403 {token} {} {encoded}", crate::encode(token)),
            token,
            &proxy,
        );
        assert_eq!(text, "403 [redacted] [redacted] [redacted]");
        let command = command(
            Path::new("/unused"),
            &"a".repeat(40),
            "https://git.example/repo.git",
            &header,
            &proxy,
        );
        for arg in command.get_args() {
            assert!(!arg.to_string_lossy().contains(token));
            assert!(!arg.to_string_lossy().contains(&encoded));
        }
        let env: std::collections::HashMap<_, _> = command.get_envs().collect();
        assert_eq!(
            env.get(std::ffi::OsStr::new("GIT_CONFIG_VALUE_1")),
            Some(&Some(std::ffi::OsStr::new(&header)))
        );
        assert_eq!(
            env.get(std::ffi::OsStr::new("GIT_OBJECT_DIRECTORY")),
            Some(&None)
        );
    }
}
