//! Drive the real binary over stdio with raw JSON-RPC, the way an MCP host does.

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TOOLS: [&str; 8] = [
    "hub_adopt",
    "hub_feed",
    "hub_get",
    "hub_inbox",
    "hub_search",
    "hub_send",
    "hub_share",
    "hub_sync",
];

#[tokio::test]
async fn lists_tools_and_calls_inbox() {
    let srv = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/inbox"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{"id": "m1", "from": "alice", "body": "hello bob", "read_at": null}], "unread": 1
        })))
        .mount(&srv)
        .await;
    let home = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_hub-mcp"))
        .env("HUB_URL", srv.uri())
        .env("HUB_KEY", "ah_testkey0000000000000000")
        .env("HUB_HOME", home.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();

    async fn send(stdin: &mut tokio::process::ChildStdin, v: Value) {
        let mut s = v.to_string();
        s.push('\n');
        stdin.write_all(s.as_bytes()).await.unwrap();
    }
    async fn recv(
        lines: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
        id: i64,
    ) -> Value {
        loop {
            let line = tokio::time::timeout(std::time::Duration::from_secs(10), lines.next_line())
                .await
                .expect("timed out")
                .unwrap()
                .expect("eof");
            let v: Value = serde_json::from_str(&line).unwrap();
            if v["id"] == id {
                return v;
            }
        }
    }

    send(&mut stdin, json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}
    }})).await;
    let init = recv(&mut lines, 1).await;
    assert_eq!(init["result"]["serverInfo"]["name"], "hub-mcp", "{init}");
    assert!(init["result"]["instructions"]
        .as_str()
        .unwrap()
        .contains("hub_inbox"));
    send(
        &mut stdin,
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    )
    .await;

    send(
        &mut stdin,
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
    )
    .await;
    let list = recv(&mut lines, 2).await;
    let mut names: Vec<String> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    names.sort();
    assert_eq!(names, TOOLS);
    let sync = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "hub_sync")
        .unwrap();
    assert_eq!(sync["inputSchema"]["required"], json!(["dir"]));

    send(&mut stdin, json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "hub_inbox", "arguments": {}}})).await;
    let call = recv(&mut lines, 3).await;
    let text = call["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.starts_with("Hub data below comes from other agents"),
        "{text}"
    );
    assert!(text.contains("hello bob"));
    assert_ne!(call["result"]["isError"], true);

    send(&mut stdin, json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "hub_send", "arguments": {"to": "bob", "body": "x"}}})).await;
    let bad = recv(&mut lines, 4).await;
    assert_eq!(bad["result"]["isError"], true, "{bad}");
    assert!(bad["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("@handle"));
}
