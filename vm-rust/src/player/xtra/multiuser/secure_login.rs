//! Return of the Legends: logging in without ever sending the password (build feature `rotl-secure-login`).
//!
//! The movie hands the password to `connectToNetServer`. With this feature, and when the page asks for it
//! (external param `secure_login=1`), the Xtra runs an OPAQUE login (crate `rotl-opaque`) instead of putting the
//! password in the `Logon` message: the `Logon` carries a public request, the server answers, and the password is
//! checked on this side, so the server, the edge in front of it and every log only ever see values that can't be
//! turned back into the password. The movie sees nothing of this: the messages below are consumed here.
//!
//! The wire protocol is written down in NodeJS-ROTL: `docs/secure-login-protocol.md`.
//!
//! This file is plain logic (no browser calls) so it can be tested natively.

use base64::{engine::general_purpose::STANDARD, Engine};

use crate::director::static_datum::StaticDatum;

/// What the third item of the `Logon` starts with when it carries a login request instead of a password.
pub const LOGON_PREFIX: &str = "OPAQUE1:";
/// Server → player: the server's answer to the login request. Consumed by the Xtra.
pub const SUBJECT_CHALLENGE: &str = "OpaqueLogin";
/// Player → server: the last step of the login (empty content when the password turned out wrong).
pub const SUBJECT_FINISH: &str = "OpaqueFinish";
/// Server → player: this account has no OPAQUE record yet, only a bcrypt hash; the content is the hash's salt part.
pub const SUBJECT_LEGACY: &str = "OpaqueLegacy";
/// Player → server: `hash|registrationRequest`, the bcrypt hash computed here, and the start of a registration.
pub const SUBJECT_MIGRATE: &str = "OpaqueMigrate";
/// Server → player: the server's answer to the registration request.
pub const SUBJECT_REGISTER_RESPONSE: &str = "OpaqueRegisterResponse";
/// Player → server: the last step of the registration.
pub const SUBJECT_REGISTER_FINISH: &str = "OpaqueRegisterFinish";
/// In place of the password in the movie's own messages once the connection is authenticated.
pub const OPAQUE_MARKER: &str = "*opaque*";
/// In place of the hash the movie sends in `LoginAuth`.
const TOKEN_PREFIX: &str = "~";

pub struct SecureLogin {
    username: String,
    password: String,
    state: Vec<u8>,
    session_key: Option<Vec<u8>>,
    /// The hash the movie sent in `LoginAuth` and the token that stands for it on the wire.
    hash_token: Option<(String, String)>,
    /// While an old account is being upgraded: where the registration stands.
    registration: Option<Vec<u8>>,
    upgraded: bool,
}

impl SecureLogin {
    /// Starts a login. Returns it and what to put in the `Logon` where the password would be.
    pub fn begin(username: &str, password: &str) -> Result<(SecureLogin, String), String> {
        let started = rotl_opaque::client_login_start(password.as_bytes()).map_err(|e| e.to_string())?;
        let field = format!("{}{}", LOGON_PREFIX, STANDARD.encode(&started.request));
        Ok((
            SecureLogin { username: username.to_string(), password: password.to_string(), state: started.state, session_key: None, hash_token: None, registration: None, upgraded: false },
            field,
        ))
    }

    /// The server's answer arrived. Returns the content for `OpaqueFinish`, or an error when the password is wrong
    /// (or the server isn't the one that registered this account).
    pub fn answer_challenge(&mut self, challenge_base64: &str) -> Result<String, String> {
        let response = STANDARD.decode(challenge_base64.trim()).map_err(|_| "the server's answer was not valid".to_string())?;
        let done = rotl_opaque::client_login_finish(&self.state, self.password.as_bytes(), &response, &self.username).map_err(|e| e.to_string())?;
        self.session_key = Some(done.session_key);
        self.state.clear();
        Ok(STANDARD.encode(&done.finalization))
    }

    pub fn is_established(&self) -> bool {
        self.session_key.is_some() || self.upgraded
    }

    /// The server says this account has only a bcrypt hash. Works out that hash here from the salt the server sent and
    /// starts a registration, so the server can check the hash and store a record without ever seeing the password.
    /// Returns the content for `OpaqueMigrate`.
    pub fn answer_legacy(&mut self, salt_part: &str) -> Result<String, String> {
        let proof = bcrypt_proof(&self.password, salt_part)?;
        let started = rotl_opaque::client_registration_start(self.password.as_bytes()).map_err(|e| e.to_string())?;
        self.registration = Some(started.state);
        Ok(format!("{}|{}", proof, STANDARD.encode(&started.request)))
    }

    /// The server answered the registration request. Returns the content for `OpaqueRegisterFinish`.
    pub fn answer_register_response(&mut self, response_base64: &str) -> Result<String, String> {
        let state = self.registration.take().ok_or_else(|| "no registration in progress".to_string())?;
        let response = STANDARD.decode(response_base64.trim()).map_err(|_| "the server's answer was not valid".to_string())?;
        let upload = rotl_opaque::client_registration_finish(&state, self.password.as_bytes(), &response, &self.username).map_err(|e| e.to_string())?;
        self.upgraded = true;
        Ok(STANDARD.encode(&upload))
    }

    pub fn session_key(&self) -> Option<&[u8]> {
        self.session_key.as_deref()
    }

    /// Rewrites a message the movie is about to send so the password (and anything made from it) stays here.
    /// Only the messages the movie is known to put the password in are touched; everything else passes unchanged.
    pub fn redact_outgoing(&mut self, subject: &str, content: &mut StaticDatum) {
        let StaticDatum::String(text) = content else { return };
        match subject {
            // `name:password:version`
            "LoadChar2" | "LoadChar" => {
                let needle = format!(":{}:", self.password);
                if !self.password.is_empty() {
                    if let Some(at) = text.find(&needle) {
                        text.replace_range(at..at + needle.len(), &format!(":{}:", OPAQUE_MARKER));
                    }
                }
            }
            // `version:hash` where the hash is made from the password by the movie
            "LoginAuth" => {
                if let Some((version, hash)) = text.split_once(':') {
                    if !hash.is_empty() && !hash.starts_with(TOKEN_PREFIX) {
                        let token = match &self.hash_token {
                            Some((original, token)) if original == hash => token.clone(),
                            _ => {
                                let token = format!("{}{}", TOKEN_PREFIX, random_hex(12));
                                self.hash_token = Some((hash.to_string(), token.clone()));
                                token
                            }
                        };
                        *text = format!("{}:{}", version, token);
                    }
                }
            }
            _ => {}
        }
    }

    /// Puts back what `redact_outgoing` took out of a message the server echoes: the movie compares it with its own.
    pub fn restore_incoming(&self, subject: &str, content: &mut StaticDatum) {
        if subject != "ConnectionGameLogin" {
            return;
        }
        if let (StaticDatum::String(text), Some((original, token))) = (content, &self.hash_token) {
            if text == token {
                *text = original.clone();
            }
        }
    }
}

/// bcrypt's own base64 alphabet (not the usual one).
const BCRYPT_ALPHABET: &[u8; 64] = b"./ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

/// The 16 salt bytes from the 22 characters of a bcrypt salt.
fn decode_bcrypt_salt(chars: &str) -> Option<[u8; 16]> {
    let mut bits: u32 = 0;
    let mut have = 0u32;
    let mut out = [0u8; 16];
    let mut n = 0;
    for c in chars.bytes() {
        let value = BCRYPT_ALPHABET.iter().position(|&a| a == c)? as u32;
        bits = (bits << 6) | value;
        have += 6;
        if have >= 8 && n < 16 {
            have -= 8;
            out[n] = ((bits >> have) & 0xff) as u8;
            n += 1;
        }
    }
    (n == 16).then_some(out)
}

/// The bcrypt hash of `password` for the salt part the server sent (`$2b$10$` plus 22 characters). The result is exactly
/// the hash the server stored, which is what lets it check we know the password without being told it.
pub fn bcrypt_proof(password: &str, salt_part: &str) -> Result<String, String> {
    let mut fields = salt_part.trim().split('$');
    let (empty, version, cost, rest) = (fields.next(), fields.next(), fields.next(), fields.next());
    if empty != Some("") {
        return Err("the salt is not a bcrypt salt".into());
    }
    let version = match version {
        Some("2a") => bcrypt::Version::TwoA,
        Some("2b") => bcrypt::Version::TwoB,
        Some("2x") => bcrypt::Version::TwoX,
        Some("2y") => bcrypt::Version::TwoY,
        _ => return Err("the salt is not a bcrypt salt".into()),
    };
    // The server chooses the cost, so it is bounded: a hostile answer must not be able to keep this page busy for long.
    let cost: u32 = cost.and_then(|c| c.parse().ok()).filter(|c| (4..=14).contains(c)).ok_or_else(|| "the bcrypt cost is out of range".to_string())?;
    let salt = rest.and_then(|r| r.get(..22)).and_then(decode_bcrypt_salt).ok_or_else(|| "the bcrypt salt is damaged".to_string())?;
    let parts = bcrypt::hash_with_salt(password.as_bytes(), cost, salt).map_err(|e| e.to_string())?;
    Ok(parts.format_for_version(version))
}

fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).expect("the browser's random numbers are unavailable");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> StaticDatum { StaticDatum::String(s.to_string()) }

    /// Hashes made by the SERVER's bcrypt (Node's `bcrypt` package), so the player and the server are known to agree.
    const SERVER_HASHES: &[(&str, &str)] = &[
        ("old password", "$2b$04$hdDLG02//jr3zVg1aqif/.66Lw02hBnjLULwZSMsAXCvp0UtoH1Ju"),
        ("correct horse battery staple", "$2b$06$jOC3Ji7Ri01tRp8Kyl1ObeD41n5cIIOnyVXSuxNEXNKib7Srin4f6"),
        ("p@ss:word|with%odd chars", "$2b$04$pk4cv1IO/oTAEMS2RYRC3.XnlPi84lraYOjcvQBsFQYrBCMCe0Tm6"),
        ("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "$2b$04$nONg62ppIzBECafXTGAEXuQFNLUI7Xev0C156fzaMYtGYUU9c7TAu"),
        ("héllo wörld ✓", "$2b$04$nD1AR3Su9cohq723orO/..RNJ.5iQFEtXPigCDUK.8NVBv.BriNQu"),
        ("", "$2b$04$TF/ujMqfErJ9YXavYo/opeE41e3mrWiQlIJYszLxP/NThI1IPkKb2"),
    ];

    #[test]
    fn the_players_bcrypt_gives_exactly_the_hash_the_server_stored() {
        for (password, stored) in SERVER_HASHES {
            assert_eq!(&bcrypt_proof(password, &stored[..29]).unwrap(), stored, "password {:?}", password);
        }
    }

    #[test]
    fn a_wrong_password_gives_a_different_hash_and_odd_salts_are_errors() {
        let (password, stored) = SERVER_HASHES[0];
        assert_ne!(bcrypt_proof("not it", &stored[..29]).unwrap(), stored);
        assert_eq!(bcrypt_proof(password, stored).unwrap(), stored, "the whole hash works too: only the salt part is used");
        for bad in ["", "$2b$10$", "$2b$10$short", "2b$10$hdDLG02//jr3zVg1aqif/.", "$9z$10$hdDLG02//jr3zVg1aqif/.", "$2b$31$hdDLG02//jr3zVg1aqif/.", "$2b$03$hdDLG02//jr3zVg1aqif/.", "$2b$xx$hdDLG02//jr3zVg1aqif/.", "$2b$10$hdDLG02//jr3zVg1aqif!!"] {
            assert!(bcrypt_proof("pw", bad).is_err(), "{:?} should be refused", bad);
        }
    }

    #[test]
    fn an_old_account_upgrades_without_the_password_leaving_the_player() {
        let (password, stored) = SERVER_HASHES[1];
        let (mut login, field) = SecureLogin::begin("Alice", password).unwrap();
        assert!(field.starts_with(LOGON_PREFIX));
        // The server's half: no record yet, only the hash, so it sends the salt and later checks the proof.
        let migrate = login.answer_legacy(&stored[..29]).unwrap();
        let (proof, request) = migrate.split_once('|').unwrap();
        assert_eq!(proof, stored, "the server can compare this with what it stored");
        assert!(!migrate.contains(password));
        let setup = rotl_opaque::server_setup_new();
        let response = rotl_opaque::server_registration_start(&setup, &STANDARD.decode(request).unwrap(), "Alice").unwrap();
        assert!(!login.is_established());
        let upload = login.answer_register_response(&STANDARD.encode(&response)).unwrap();
        assert!(login.is_established());
        let record = rotl_opaque::server_registration_finish(&STANDARD.decode(upload).unwrap()).unwrap();
        // And the record works for a normal secure login afterwards.
        let (mut again, field) = SecureLogin::begin("Alice", password).unwrap();
        let (challenge, state) = server_answers(&setup, Some(&record), "Alice", &field);
        let finish = again.answer_challenge(&challenge).unwrap();
        assert!(rotl_opaque::server_login_finish(&state, &STANDARD.decode(finish).unwrap(), "Alice").is_ok());
    }

    #[test]
    fn upgrade_steps_out_of_order_or_damaged_are_errors() {
        let (mut login, _) = SecureLogin::begin("Alice", "pw").unwrap();
        assert!(login.answer_register_response("AAAA").is_err(), "no registration was started");
        assert!(login.answer_legacy("not a salt").is_err());
        login.answer_legacy(&SERVER_HASHES[0].1[..29]).unwrap();
        assert!(login.answer_register_response("not base64 !!").is_err());
        assert!(!login.is_established());
    }
    fn as_str(d: &StaticDatum) -> &str { match d { StaticDatum::String(s) => s, _ => panic!("not a string") } }

    /// Registers an account the way the server will, and returns (server secret, stored record).
    fn registered(user: &str, password: &str) -> (Vec<u8>, Vec<u8>) {
        let setup = rotl_opaque::server_setup_new();
        let c1 = rotl_opaque::client_registration_start(password.as_bytes()).unwrap();
        let s1 = rotl_opaque::server_registration_start(&setup, &c1.request, user).unwrap();
        let c2 = rotl_opaque::client_registration_finish(&c1.state, password.as_bytes(), &s1, user).unwrap();
        (setup.clone(), rotl_opaque::server_registration_finish(&c2).unwrap())
    }

    /// The server's half, as the NodeJS server will run it.
    fn server_answers(setup: &[u8], record: Option<&[u8]>, user: &str, logon_field: &str) -> (String, Vec<u8>) {
        let request = STANDARD.decode(logon_field.strip_prefix(LOGON_PREFIX).expect("a request, not a password")).unwrap();
        let started = rotl_opaque::server_login_start(setup, record, &request, user).unwrap();
        (STANDARD.encode(&started.response), started.state)
    }

    #[test]
    fn the_logon_carries_a_request_and_never_the_password() {
        let (_, field) = SecureLogin::begin("Alice", "hunter2-unique").unwrap();
        assert!(field.starts_with(LOGON_PREFIX));
        assert!(!field.contains("hunter2-unique"));
        assert!(field.len() > 40, "a real request");
    }

    #[test]
    fn the_right_password_logs_in_and_both_ends_share_a_key() {
        let (setup, record) = registered("Alice", "hunter2");
        let (mut login, field) = SecureLogin::begin("Alice", "hunter2").unwrap();
        let (challenge, server_state) = server_answers(&setup, Some(&record), "Alice", &field);
        let finish = login.answer_challenge(&challenge).unwrap();
        assert!(login.is_established());
        let server_key = rotl_opaque::server_login_finish(&server_state, &STANDARD.decode(finish).unwrap(), "Alice").unwrap();
        assert_eq!(login.session_key().unwrap(), &server_key[..]);
    }

    #[test]
    fn a_wrong_password_fails_on_the_player_and_never_establishes() {
        let (setup, record) = registered("Alice", "right");
        let (mut login, field) = SecureLogin::begin("Alice", "wrong").unwrap();
        let (challenge, _) = server_answers(&setup, Some(&record), "Alice", &field);
        assert!(login.answer_challenge(&challenge).is_err());
        assert!(!login.is_established());
        assert!(login.session_key().is_none());
    }

    #[test]
    fn an_account_the_server_does_not_know_looks_like_a_wrong_password() {
        let setup = rotl_opaque::server_setup_new();
        let (mut login, field) = SecureLogin::begin("Nobody", "anything").unwrap();
        let (challenge, _) = server_answers(&setup, None, "Nobody", &field);
        assert!(login.answer_challenge(&challenge).is_err());
    }

    #[test]
    fn a_damaged_answer_is_an_error_not_a_crash() {
        let (mut login, _) = SecureLogin::begin("Alice", "pw").unwrap();
        assert!(login.answer_challenge("not base64 !!!").is_err());
        assert!(login.answer_challenge("AAAA").is_err());
        assert!(login.answer_challenge("").is_err());
    }

    #[test]
    fn loadchar_loses_the_password_and_keeps_the_rest() {
        let (mut login, _) = SecureLogin::begin("Alice", "p@ss:word").unwrap();
        let mut content = text("Alice:p@ss:word:1.2.3");
        login.redact_outgoing("LoadChar2", &mut content);
        assert_eq!(as_str(&content), "Alice:*opaque*:1.2.3");
        let mut other = text("Alice:somethingelse:1.2.3");
        login.redact_outgoing("LoadChar2", &mut other);
        assert_eq!(as_str(&other), "Alice:somethingelse:1.2.3", "only the password is replaced");
    }

    #[test]
    fn loginauth_hash_is_replaced_by_a_token_and_the_echo_is_put_back() {
        let (mut login, _) = SecureLogin::begin("Alice", "pw").unwrap();
        let mut content = text("v7:5f4dcc3b5aa765d61d8327deb882cf99");
        login.redact_outgoing("LoginAuth", &mut content);
        let sent = as_str(&content).to_string();
        assert!(sent.starts_with("v7:~") && !sent.contains("5f4dcc3b"), "the hash did not leave: {sent}");
        let token = sent.strip_prefix("v7:").unwrap().to_string();
        // The server echoes the token back as ConnectionGameLogin; the movie gets its own hash again.
        let mut echo = text(&token);
        login.restore_incoming("ConnectionGameLogin", &mut echo);
        assert_eq!(as_str(&echo), "5f4dcc3b5aa765d61d8327deb882cf99");
        // Sending it again uses the same token, and unrelated messages are never touched.
        let mut again = text("v7:5f4dcc3b5aa765d61d8327deb882cf99");
        login.redact_outgoing("LoginAuth", &mut again);
        assert_eq!(as_str(&again), sent);
        let mut chat = text(&token);
        login.restore_incoming("Chat", &mut chat);
        assert_eq!(as_str(&chat), token);
    }

    #[test]
    fn other_messages_and_non_text_content_pass_through_untouched() {
        let (mut login, _) = SecureLogin::begin("Alice", "pw").unwrap();
        for subject in ["Chat", "Move", "QueryCreate", "CheckifRemake"] {
            let mut content = text("hello pw world :pw:");
            login.redact_outgoing(subject, &mut content);
            assert_eq!(as_str(&content), "hello pw world :pw:", "{subject}");
        }
        let mut list = StaticDatum::List(vec![text("Alice"), text("pw")]);
        login.redact_outgoing("LoadChar2", &mut list);
        assert!(matches!(&list, StaticDatum::List(l) if l.len() == 2));
        let mut empty_pw = SecureLogin::begin("Bob", "").unwrap().0;
        let mut content = text("Bob::1.0");
        empty_pw.redact_outgoing("LoadChar2", &mut content);
        assert_eq!(as_str(&content), "Bob::1.0", "an empty password is never 'found' everywhere");
    }
}
