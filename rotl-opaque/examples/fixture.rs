//! Writes a server secret and a stored record for one account, as "key=hex" lines, for the cross-build tests.
//!   cargo run --release --example fixture > tests/fixtures/native.txt
use rotl_opaque::*;

fn hex(b: &[u8]) -> String { b.iter().map(|x| format!("{x:02x}")).collect() }

fn main() {
    let setup = server_setup_new();
    let c1 = client_registration_start(b"fixture-password").unwrap();
    let s1 = server_registration_start(&setup, &c1.request, "Alice").unwrap();
    let c2 = client_registration_finish(&c1.state, b"fixture-password", &s1, "Alice").unwrap();
    let record = server_registration_finish(&c2).unwrap();
    println!("username=Alice\npassword=fixture-password\nsetup={}\nrecord={}", hex(&setup), hex(&record));
}
