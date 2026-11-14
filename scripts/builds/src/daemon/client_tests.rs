use super::*;
use crate::testing::Scratch;
use std::os::unix::net::UnixListener;

/// A daemon that reads the question and answers `Failed("late")` after
/// <delay>, one thread per connection as the real daemon's accept loop does.
fn serve_after(socket: &Path, delay: Duration) {
    let listener = UnixListener::bind(socket).unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            std::thread::spawn(move || {
                let mut line = String::new();
                BufReader::new(stream.try_clone().unwrap())
                    .read_line(&mut line)
                    .unwrap();
                std::thread::sleep(delay);
                let reply = serde_json::to_string(&Reply::Failed("late".into())).unwrap();
                let _ = writeln!(stream, "{reply}");
            });
        }
    });
}

/// An answer that takes longer than the client waits is reported as a wait
/// that ran out, naming the question and the wait. The operating system's
/// own text for it ("Resource temporarily unavailable", EAGAIN on macOS)
/// says nothing about what the command was doing.
#[test]
fn a_late_answer_names_the_wait_not_eagain() {
    let scratch = Scratch::new("late-answer");
    let socket = scratch.dir.join("socket");
    serve_after(&socket, Duration::from_millis(1500));
    let message = match ask_within(
        &socket,
        &scratch.dir.join("lock"),
        &Question::Health,
        Duration::from_millis(200),
    ) {
        Err(message) => message,
        Ok(_) => panic!("the answer was late, so the wait should have run out"),
    };
    assert!(
        message.contains("waited 200ms") && message.contains("health"),
        "{message}"
    );
    assert!(!message.contains("temporarily unavailable"), "{message}");
}

/// Many commands asking at once all get their answers from a daemon whose
/// accept loop hands each connection to its own thread.
#[test]
fn many_clients_at_once_all_get_answers() {
    let scratch = Scratch::new("many-clients");
    let socket = scratch.dir.join("socket");
    serve_after(&socket, Duration::from_millis(50));
    let lock = scratch.dir.join("lock");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(100));
    let clients: Vec<_> = (0..100)
        .map(|_| {
            let (socket, lock, barrier) = (socket.clone(), lock.clone(), barrier.clone());
            std::thread::spawn(move || {
                barrier.wait();
                ask_within(&socket, &lock, &Question::Health, Duration::from_secs(20))
            })
        })
        .collect();
    for client in clients {
        match client.join().unwrap() {
            Ok(Some(Reply::Failed(text))) => assert_eq!(text, "late"),
            Ok(_) => panic!("no daemon answer"),
            Err(message) => panic!("{message}"),
        }
    }
}

/// A refused connection to a socket whose daemon holds the lock is a busy
/// daemon, not a stale socket: the command says so and measures itself.
#[test]
fn a_refusal_from_a_live_daemon_is_not_a_stale_socket() {
    let scratch = Scratch::new("refused-live");
    let socket = scratch.dir.join("socket");
    let lock = scratch.dir.join("lock");
    crate::testing::stale_socket(&socket);
    let _live = super::super::live::claim(&lock).unwrap().unwrap();
    let message = match ask_within(&socket, &lock, &Question::Health, Duration::from_secs(1)) {
        Err(message) => message,
        Ok(_) => panic!("a live daemon's refusal was taken for a stale socket"),
    };
    assert!(message.contains("refused"), "{message}");
}

/// With no daemon holding the lock, a refused connection is a socket a daemon
/// left behind: no daemon answers.
#[test]
fn a_refusal_with_no_lock_held_is_a_stale_socket() {
    let scratch = Scratch::new("refused-stale");
    let socket = scratch.dir.join("socket");
    crate::testing::stale_socket(&socket);
    let reply = ask_within(
        &socket,
        &scratch.dir.join("lock"),
        &Question::Health,
        Duration::from_secs(1),
    );
    assert!(
        matches!(reply, Ok(None)),
        "{}",
        match &reply {
            Ok(None) => "no daemon".to_string(),
            Ok(Some(_)) => "a reply".to_string(),
            Err(message) => message.clone(),
        }
    );
}
