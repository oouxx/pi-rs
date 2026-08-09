//! Integration test: `steer` / `follow_up` RPC commands must queue messages
//! and report them via `queue_update` events + `get_state.pendingMessageCount`
//! (matching TS `_queueSteer` which pushes to `_steeringMessages`, emits a
//! queue update, and enqueues on the agent).
//!
//! Regression guard for: `AgentSession::steer`/`follow_up` previously only
//! enqueued on the agent — the mirror (`steering_messages` / `follow_up_messages`)
//! was never updated, so `pendingMessageCount` was always 0 and no
//! `queue_update` event was emitted.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

fn spawn_rpc() -> (std::process::Child, std::io::BufWriter<std::process::ChildStdin>, BufReader<std::process::ChildStdout>) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_pi-rs"))
        .arg("--mode")
        .arg("rpc")
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn pi-rs --mode rpc");
    let stdin = std::io::BufWriter::new(child.stdin.take().expect("stdin"));
    let stdout = BufReader::new(child.stdout.take().expect("stdout"));
    (child, stdin, stdout)
}

/// Send one JSON line and read lines until we find the response for `id`.
fn send_and_read(
    stdin: &mut std::io::BufWriter<std::process::ChildStdin>,
    stdout: &mut BufReader<std::process::ChildStdout>,
    line: &str,
    want_id: &str,
) -> Vec<serde_json::Value> {
    writeln!(stdin, "{line}").expect("write command");
    stdin.flush().expect("flush");
    let mut out = Vec::new();
    loop {
        let mut buf = String::new();
        let n = stdout.read_line(&mut buf).expect("read line");
        assert!(n > 0, "RPC process closed stdout while waiting for {want_id}");
        let value: serde_json::Value = serde_json::from_str(buf.trim()).expect("valid JSON");
        let is_target = value.get("id").and_then(|v| v.as_str()) == Some(want_id);
        out.push(value);
        if is_target {
            return out;
        }
    }
}

#[test]
fn steer_queues_message_and_reports_pending_count() {
    let (mut child, mut stdin, mut stdout) = spawn_rpc();

    // steer → success response + a queue_update event with the message in the
    // steering queue.
    let lines = send_and_read(&mut stdin, &mut stdout, r#"{"type":"steer","id":"s1","message":"stop and wait"}"#, "s1");
    let response = lines.last().expect("response");
    assert_eq!(response["command"], "steer");
    assert_eq!(response["success"], true, "steer must succeed: {response}");
    let queue_update = lines
        .iter()
        .find(|l| l["type"] == "queue_update")
        .expect("queue_update event must be emitted after steer");
    assert_eq!(queue_update["steering"], serde_json::json!(["stop and wait"]));
    assert_eq!(queue_update["followUp"], serde_json::json!([]));

    // get_state → pendingMessageCount reflects the queued steer message.
    let lines = send_and_read(&mut stdin, &mut stdout, r#"{"type":"get_state","id":"s2"}"#, "s2");
    let state = lines.last().expect("response");
    assert_eq!(state["command"], "get_state");
    assert_eq!(state["data"]["pendingMessageCount"], 1);

    child.kill().expect("kill child");
    let _ = child.wait();
}

/// A `/`-prefixed message that is NOT an extension command must queue
/// normally (the extension-command rejection only fires for registered
/// extension commands, matching TS `_throwIfExtensionCommand`).
#[test]
fn steer_non_extension_slash_text_is_not_rejected() {
    let (mut child, mut stdin, mut stdout) = spawn_rpc();

    let lines = send_and_read(&mut stdin, &mut stdout, r#"{"type":"steer","id":"s1","message":"/nonexistent_cmd hello"}"#, "s1");
    let response = lines.last().expect("response");
    assert_eq!(response["command"], "steer");
    assert_eq!(response["success"], true, "unknown slash text must queue: {response}");
    let queue_update = lines
        .iter()
        .find(|l| l["type"] == "queue_update")
        .expect("queue_update event");
    assert_eq!(queue_update["steering"], serde_json::json!(["/nonexistent_cmd hello"]));

    child.kill().expect("kill child");
    let _ = child.wait();
}

#[test]
fn follow_up_queues_message_and_reports_pending_count() {
    let (mut child, mut stdin, mut stdout) = spawn_rpc();

    let lines = send_and_read(&mut stdin, &mut stdout, r#"{"type":"follow_up","id":"f1","message":"continue when done"}"#, "f1");
    let response = lines.last().expect("response");
    assert_eq!(response["command"], "follow_up");
    assert_eq!(response["success"], true);
    let queue_update = lines
        .iter()
        .find(|l| l["type"] == "queue_update")
        .expect("queue_update event must be emitted after follow_up");
    assert_eq!(queue_update["steering"], serde_json::json!([]));
    assert_eq!(queue_update["followUp"], serde_json::json!(["continue when done"]));

    let lines = send_and_read(&mut stdin, &mut stdout, r#"{"type":"get_state","id":"f2"}"#, "f2");
    let state = lines.last().expect("response");
    assert_eq!(state["data"]["pendingMessageCount"], 1);

    child.kill().expect("kill child");
    let _ = child.wait();
}
