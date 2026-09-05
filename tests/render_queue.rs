use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

struct Server(Child, std::path::PathBuf);
impl Drop for Server {
    fn drop(&mut self) {
        if let Ok(mut stream) = UnixStream::connect(&self.1) {
            let _ = writeln!(stream, "{}", json!({"command":"shutdown"}));
            let deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < deadline {
                if self.0.try_wait().ok().flatten().is_some() {
                    return;
                }
                thread::sleep(Duration::from_millis(20));
            }
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn request(socket: &Path, command: Value) -> Value {
    let mut stream = UnixStream::connect(socket).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    writeln!(stream, "{command}").unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}
fn call(socket: &Path, command: Value) -> Value {
    let reply = request(socket, command);
    assert_eq!(reply["ok"], true, "{reply}");
    reply["result"].clone()
}
fn until(mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "server operation timed out");
        thread::sleep(Duration::from_millis(20));
    }
}
#[test]
fn queued_bounces_capture_revision_cancel_and_recover_without_manual_resubmission() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("song.muz");
    let socket = dir.path().join("control.sock");
    let original = r#"song({tracks:[track("a",phrase("C4:w"),synth("bell"))],tail:0.2})"#;
    std::fs::write(&source, original).unwrap();
    let mut server = Server(
        Command::new(env!("CARGO_BIN_EXE_muz"))
            .args([
                "serve",
                source.to_str().unwrap(),
                "--headless",
                "--stopped",
                "--socket",
                socket.to_str().unwrap(),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
        socket.clone(),
    );
    until(|| socket.exists());
    let revision = call(&socket, json!({"command":"status"}))["applied_revision"].clone();
    let submit = |name: &str, seconds: f64| {
        call(
            &socket,
            json!({"command":"render","output":dir.path().join(name),"seconds":seconds}),
        )
    };
    let a = submit("block-a.wav", 3600.0);
    let b = submit("block-b.wav", 3600.0);
    assert_eq!(a["state"], "running");
    assert_eq!(b["state"], "running");
    let protected = dir.path().join("cancel.wav");
    std::fs::write(&protected, b"keep existing output").unwrap();
    let cancelled = submit("cancel.wav", 0.1);
    assert_eq!(cancelled["state"], "queued");
    assert_eq!(
        call(&socket, json!({"command":"cancel","id":cancelled["id"]}))["state"],
        "cancelled"
    );
    assert_eq!(std::fs::read(&protected).unwrap(), b"keep existing output");
    let failed = call(
        &socket,
        json!({"command":"render","output":dir.path().join("bad.wav"),"section":"missing"}),
    );
    let queued = submit("queued.wav", 0.1);
    assert_eq!(queued["state"], "queued");
    assert_eq!(queued["revision"], revision);
    assert_eq!(
        request(
            &socket,
            json!({"command":"render","output":dir.path().join("queued.wav")})
        )["ok"],
        false
    );
    // Fill the bounded waiting queue while two long workers hold the slots.
    let mut extras = vec![];
    for i in 0..30 {
        extras.push(submit(&format!("extra-{i}.wav"), 0.1));
    }
    assert_eq!(
        request(
            &socket,
            json!({"command":"render","output":dir.path().join("overflow.wav")})
        )["ok"],
        false
    );
    for j in extras {
        call(&socket, json!({"command":"cancel","id":j["id"]}));
    }
    std::fs::write(&source, original.replace("C4:w", "D4:w")).unwrap();
    until(|| {
        call(&socket, json!({"command":"status"}))["applied_revision"]
            .as_u64()
            .unwrap()
            > revision.as_u64().unwrap()
    });
    for j in [&a, &b] {
        call(&socket, json!({"command":"cancel","id":j["id"]}));
    }
    let job = |id: &Value| {
        call(&socket, json!({"command":"jobs"}))
            .as_array()
            .unwrap()
            .iter()
            .find(|j| j["id"] == *id)
            .unwrap()
            .clone()
    };
    until(|| job(&queued["id"])["state"] == "finished");
    until(|| job(&failed["id"])["state"] == "failed");
    assert_eq!(job(&queued["id"])["result"]["Ok"]["revision"], revision);
    assert!(!dir.path().join("bad.wav").exists());
    // Captured source is audible, not just a revision number copied onto a newer render.
    let reference_source = dir.path().join("old.muz");
    let reference = dir.path().join("reference.wav");
    std::fs::write(&reference_source, original).unwrap();
    muz::render::render(
        &reference_source,
        &reference,
        Some(0.1),
        None,
        &[],
        48000,
        256,
    )
    .unwrap();
    assert_eq!(
        std::fs::read(reference).unwrap(),
        std::fs::read(dir.path().join("queued.wav")).unwrap()
    );
    submit("shutdown-a.wav", 3600.0);
    submit("shutdown-b.wav", 3600.0);
    assert_eq!(submit("shutdown-queued.wav", 0.1)["state"], "queued");
    call(&socket, json!({"command":"shutdown"}));
    until(|| server.0.try_wait().unwrap().is_some());
    assert!(server.0.wait().unwrap().success());
    assert!(!dir.path().join("shutdown-queued.wav").exists());
}
