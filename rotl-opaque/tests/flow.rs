use rotl_opaque::*;

/// Registers `password` for `username` the way a player and the server would, and returns the stored record.
fn register(setup: &[u8], username: &str, password: &str) -> Vec<u8> {
    let c1 = client_registration_start(password.as_bytes()).unwrap();
    let s1 = server_registration_start(setup, &c1.request, username).unwrap();
    let c2 = client_registration_finish(&c1.state, password.as_bytes(), &s1, username).unwrap();
    server_registration_finish(&c2).unwrap()
}

/// One full login. Returns what the player and the server ended up with (`None` where that side said no).
fn login(setup: &[u8], record: Option<&[u8]>, as_user: &str, server_user: &str, password: &str) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    let c1 = client_login_start(password.as_bytes()).unwrap();
    let s1 = server_login_start(setup, record, &c1.request, server_user).unwrap();
    match client_login_finish(&c1.state, password.as_bytes(), &s1.response, as_user) {
        Err(_) => (None, None),
        Ok(done) => {
            let server_key = server_login_finish(&s1.state, &done.finalization, server_user).ok();
            (Some(done.session_key), server_key)
        }
    }
}

#[test]
fn the_right_password_logs_in_and_both_sides_share_a_key() {
    let setup = server_setup_new();
    let record = register(&setup, "Alice", "correct horse battery staple");
    let (player, server) = login(&setup, Some(&record), "Alice", "Alice", "correct horse battery staple");
    assert!(player.is_some() && server.is_some());
    assert_eq!(player, server, "both ends hold the same session key");
    assert_eq!(player.unwrap().len(), 64);
}

#[test]
fn each_login_makes_a_new_key() {
    let setup = server_setup_new();
    let record = register(&setup, "Alice", "pw");
    let a = login(&setup, Some(&record), "Alice", "Alice", "pw").0.unwrap();
    let b = login(&setup, Some(&record), "Alice", "Alice", "pw").0.unwrap();
    assert_ne!(a, b);
}

#[test]
fn a_wrong_password_gets_nowhere() {
    let setup = server_setup_new();
    let record = register(&setup, "Alice", "right");
    assert_eq!(login(&setup, Some(&record), "Alice", "Alice", "wrong"), (None, None));
}

#[test]
fn an_account_that_does_not_exist_looks_like_a_wrong_password() {
    let setup = server_setup_new();
    // The server still answers, so the shape of the reply gives nothing away.
    let c1 = client_login_start(b"anything").unwrap();
    let ghost = server_login_start(&setup, None, &c1.request, "Nobody").unwrap();
    let record = register(&setup, "Alice", "pw");
    let real = server_login_start(&setup, Some(&record), &c1.request, "Alice").unwrap();
    assert_eq!(ghost.response.len(), real.response.len());
    assert_eq!(login(&setup, None, "Nobody", "Nobody", "anything"), (None, None));
}

#[test]
fn a_record_cannot_be_moved_to_another_account() {
    let setup = server_setup_new();
    let alice = register(&setup, "Alice", "shared-password");
    // Same password, but the record belongs to Alice: logging in as Bob with it fails.
    assert_eq!(login(&setup, Some(&alice), "Bob", "Bob", "shared-password"), (None, None));
}

#[test]
fn the_name_is_not_case_sensitive() {
    let setup = server_setup_new();
    let record = register(&setup, "Alice", "pw");
    let (player, server) = login(&setup, Some(&record), "  aLiCe ", "ALICE", "pw");
    assert!(player.is_some() && player == server);
}

#[test]
fn a_different_server_setup_cannot_log_anyone_in() {
    let setup = server_setup_new();
    let record = register(&setup, "Alice", "pw");
    let other = server_setup_new();
    assert_eq!(login(&other, Some(&record), "Alice", "Alice", "pw"), (None, None), "a stolen record is useless without the server's secret");
}

#[test]
fn a_tampered_final_message_is_refused() {
    let setup = server_setup_new();
    let record = register(&setup, "Alice", "pw");
    let c1 = client_login_start(b"pw").unwrap();
    let s1 = server_login_start(&setup, Some(&record), &c1.request, "Alice").unwrap();
    let mut done = client_login_finish(&c1.state, b"pw", &s1.response, "Alice").unwrap();
    let last = done.finalization.len() - 1;
    done.finalization[last] ^= 1;
    assert!(server_login_finish(&s1.state, &done.finalization, "Alice").is_err());
}

#[test]
fn garbage_is_an_error_not_a_crash() {
    let setup = server_setup_new();
    assert!(server_registration_start(&setup, &[1, 2, 3], "x").is_err());
    assert!(server_login_start(&setup, None, &[], "x").is_err());
    assert!(server_login_start(&[0u8; 5], None, &[0u8; 32], "x").is_err());
    assert!(client_login_finish(&[9, 9], b"pw", &[1], "x").is_err());
    assert!(server_login_finish(&[], &[], "x").is_err());
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn the_password_is_never_in_anything_sent_or_stored() {
    let password = "tr0ub4dor&3-unique-marker";
    let setup = server_setup_new();
    let c1 = client_registration_start(password.as_bytes()).unwrap();
    let s1 = server_registration_start(&setup, &c1.request, "Alice").unwrap();
    let c2 = client_registration_finish(&c1.state, password.as_bytes(), &s1, "Alice").unwrap();
    let record = server_registration_finish(&c2).unwrap();
    let l1 = client_login_start(password.as_bytes()).unwrap();
    let ls = server_login_start(&setup, Some(&record), &l1.request, "Alice").unwrap();
    let done = client_login_finish(&l1.state, password.as_bytes(), &ls.response, "Alice").unwrap();
    for (name, bytes) in [("registration request", &c1.request), ("registration response", &s1), ("registration upload", &c2), ("stored record", &record),
        ("login request", &l1.request), ("login response", &ls.response), ("login final", &done.finalization), ("server state", &ls.state)] {
        assert!(!contains(bytes, password.as_bytes()), "{name} must not contain the password");
    }
}

#[test]
fn the_same_password_makes_a_different_record_each_time() {
    let setup = server_setup_new();
    assert_ne!(register(&setup, "Alice", "pw"), register(&setup, "Alice", "pw"));
}
