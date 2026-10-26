//! Records made by the native build and by the WebAssembly build must both work with this build.
use rotl_opaque::*;

fn field(text: &str, key: &str) -> String {
    text.lines().find_map(|l| l.strip_prefix(&format!("{key}="))).unwrap_or_else(|| panic!("missing {key}")).to_string()
}
fn unhex(s: &str) -> Vec<u8> { (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect() }

fn log_in_with(fixture: &str) {
    let setup = unhex(&field(fixture, "setup"));
    let record = unhex(&field(fixture, "record"));
    let (user, pw) = (field(fixture, "username"), field(fixture, "password"));
    let c1 = client_login_start(pw.as_bytes()).unwrap();
    let s1 = server_login_start(&setup, Some(&record), &c1.request, &user).unwrap();
    let done = client_login_finish(&c1.state, pw.as_bytes(), &s1.response, &user).unwrap();
    assert_eq!(server_login_finish(&s1.state, &done.finalization, &user).unwrap(), done.session_key);
    let c1 = client_login_start(b"not-it").unwrap();
    let s1 = server_login_start(&setup, Some(&record), &c1.request, &user).unwrap();
    assert!(client_login_finish(&c1.state, b"not-it", &s1.response, &user).is_err());
}

#[test]
fn a_record_made_by_the_native_build_works() { log_in_with(include_str!("fixtures/native.txt")); }

#[test]
fn a_record_made_by_the_webassembly_build_works() { log_in_with(include_str!("fixtures/wasm.txt")); }
