use super::*;
use crate::testing::Scratch;

const THREAD: &str = "00000000-0000-4000-8000-000000000007";

#[test]
fn a_codex_inbox_stores_its_thread_without_a_socket_or_secret() {
    let inbox = crate::inbox::Messaging::restore("codex", THREAD, None).unwrap();
    assert_eq!(inbox.stored(), ("codex", THREAD, None));
    assert!(crate::inbox::Messaging::restore("codex", THREAD, Some("secret")).is_err());
    assert!(crate::inbox::Messaging::restore("codex", "/tmp/socket", None).is_err());
}

#[test]
fn a_codex_queue_distinguishes_acceptance_absence_and_transport_failure() {
    let scratch = Scratch::new("codex-queue");
    let program = scratch.script("codex", &format!(r#"
case "$5" in
accepted) [ "$1 $2 $3 $4 $6" = 'queue --thread {THREAD} --message --resume' ] ;;
missing) echo 'Error: thread/queue/add failed: no rollout found for thread id {THREAD}' >&2; exit 1 ;;
*) echo 'daemon unavailable' >&2; exit 1 ;;
esac
"#));
    let thread = ThreadId::parse(THREAD).unwrap();
    assert_eq!(
        queue(&thread, &program, "accepted").unwrap(),
        Sent::Delivered
    );
    assert_eq!(queue(&thread, &program, "missing").unwrap(), Sent::Gone);
    assert!(
        queue(&thread, &program, "error")
            .unwrap_err()
            .text()
            .contains("daemon unavailable")
    );
}

#[test]
fn a_codex_judges_thread_can_be_idle_but_must_exist() {
    let scratch = Scratch::new("codex-read");
    let fake = |name, response: Value| {
        scratch.script(
            name,
            &format!(
                r#"
read request
printf '%s\n' '{{"id":1,"result":{{}}}}'
read request
printf '%s\n' '{}'
"#,
                response
            ),
        )
    };
    let saved = fake(
        "saved",
        json!({"id":2,"result":{"thread":{"id":THREAD,"status":{"type":"notLoaded"}}}}),
    );
    let missing = fake(
        "missing",
        json!({"id":2,"error":{"code": -32600,"message":format!("thread not loaded: {THREAD}")}}),
    );
    let thread = ThreadId::parse(THREAD).unwrap();
    assert!(exists(&thread, &saved).unwrap());
    assert!(!exists(&thread, &missing).unwrap());
}

#[test]
fn codex_startup_failure_returns_the_standard_error_diagnostic() {
    let scratch = Scratch::new("codex-rpc-diagnostic");
    let failing = scratch.script(
        "codex",
        "echo 'app-server configuration failed' >&2; exit 1",
    );
    let thread = ThreadId::parse(THREAD).unwrap();
    let reason = exists(&thread, &failing).unwrap_err().text();
    assert!(
        reason.contains("app-server configuration failed"),
        "{reason}"
    );
}
