//! OPAQUE password login for Return of the Legends.
//!
//! The player proves it knows a password without ever sending it, and the server stores a record from which the
//! password cannot be recovered, so neither the server, the edge in front of it, nor any log ever sees the password.
//! Both sides come from this one crate: the server module is this crate built to WebAssembly for Node, and the player
//! uses it as a normal Rust dependency, so the two can't drift apart.
//!
//! Everything crosses the boundary as plain bytes. A login is two messages each way; the registration of a password
//! is two messages each way too. The account name is part of the proof, so a stored record only works for the account
//! it was made for.
//!
//! **Never change [`CONTEXT`], [`SERVER_ID`] or the key-stretching settings once records exist:** every stored record
//! and every player build would stop agreeing.

use opaque_ke::argon2::Argon2;
use opaque_ke::ciphersuite::CipherSuite;
use rand_core::OsRng;
use opaque_ke::{
    ClientLogin, ClientLoginFinishParameters, ClientRegistration, ClientRegistrationFinishParameters,
    CredentialFinalization, CredentialRequest, CredentialResponse, Identifiers, RegistrationRequest,
    RegistrationResponse, RegistrationUpload, ServerLogin, ServerLoginParameters, ServerRegistration, ServerSetup,
};

#[cfg(feature = "js")]
pub mod wasm;

/// The choices OPAQUE is built from: Ristretto255 for the group, Triple-DH for the key exchange, Argon2 to slow down
/// guessing on the player's side.
pub struct Suite;

impl CipherSuite for Suite {
    type OprfCs = opaque_ke::Ristretto255;
    type KeyExchange = opaque_ke::TripleDh<opaque_ke::Ristretto255, sha2::Sha512>;
    type Ksf = Argon2<'static>;
}

/// Mixed into every login so a login for some other service can never be replayed here.
pub const CONTEXT: &[u8] = b"rotl-opaque-v1";
/// The server's name inside the proof.
pub const SERVER_ID: &[u8] = b"rotl";

#[derive(Debug)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Error {}

type Result<T> = std::result::Result<T, Error>;

fn err<E: std::fmt::Debug>(what: &str) -> impl FnOnce(E) -> Error + '_ {
    move |e| Error(format!("{what}: {e:?}"))
}

/// Account names are not case sensitive in the game, so the proof uses one spelling of them.
pub fn account_id(username: &str) -> Vec<u8> {
    username.trim().to_lowercase().into_bytes()
}

// ── Server ──────────────────────────────────────────────────────────────────

/// The server's long-term secret setup. Make it once and keep it with the other secrets: losing it makes every stored
/// record useless.
pub fn server_setup_new() -> Vec<u8> {
    ServerSetup::<Suite>::new(&mut OsRng).serialize().to_vec()
}

fn load_setup(setup: &[u8]) -> Result<ServerSetup<Suite>> {
    ServerSetup::<Suite>::deserialize(setup).map_err(err("server setup"))
}

/// Registration, step 1 (server): answers the player's first message.
pub fn server_registration_start(setup: &[u8], request: &[u8], username: &str) -> Result<Vec<u8>> {
    let setup = load_setup(setup)?;
    let request = RegistrationRequest::deserialize(request).map_err(err("registration request"))?;
    let started = ServerRegistration::<Suite>::start(&setup, request, &account_id(username)).map_err(err("registration start"))?;
    Ok(started.message.serialize().to_vec())
}

/// Registration, step 2 (server): turns the player's second message into the record to store for the account.
pub fn server_registration_finish(upload: &[u8]) -> Result<Vec<u8>> {
    let upload = RegistrationUpload::<Suite>::deserialize(upload).map_err(err("registration upload"))?;
    Ok(ServerRegistration::<Suite>::finish(upload).serialize().to_vec())
}

pub struct ServerLoginStart {
    /// Sent back to the player.
    pub response: Vec<u8>,
    /// Kept by the server, for this connection only, until the player's last message arrives.
    pub state: Vec<u8>,
}

/// Login, step 1 (server). `record` is the stored record for the account, or `None` when there is no such account:
/// the answer then looks exactly like one for a real account, so it does not reveal which names exist.
pub fn server_login_start(setup: &[u8], record: Option<&[u8]>, request: &[u8], username: &str) -> Result<ServerLoginStart> {
    let setup = load_setup(setup)?;
    let record = match record {
        Some(bytes) => Some(ServerRegistration::<Suite>::deserialize(bytes).map_err(err("stored record"))?),
        None => None,
    };
    let request = CredentialRequest::deserialize(request).map_err(err("login request"))?;
    let id = account_id(username);
    let started = ServerLogin::start(
        &mut OsRng,
        &setup,
        record,
        request,
        &id,
        ServerLoginParameters { context: Some(CONTEXT), identifiers: Identifiers { client: Some(&id), server: Some(SERVER_ID) } },
    )
    .map_err(err("login start"))?;
    Ok(ServerLoginStart { response: started.message.serialize().to_vec(), state: started.state.serialize().to_vec() })
}

/// Login, step 2 (server): `Ok` with the shared session key only if the player really knew the password.
pub fn server_login_finish(state: &[u8], finalization: &[u8], username: &str) -> Result<Vec<u8>> {
    let state = ServerLogin::<Suite>::deserialize(state).map_err(err("login state"))?;
    let finalization = CredentialFinalization::<Suite>::deserialize(finalization).map_err(err("login finalization"))?;
    let id = account_id(username);
    let done = state
        .finish(
            finalization,
            ServerLoginParameters { context: Some(CONTEXT), identifiers: Identifiers { client: Some(&id), server: Some(SERVER_ID) } },
        )
        .map_err(|_| Error("wrong password".into()))?;
    Ok(done.session_key.to_vec())
}

// ── Player ──────────────────────────────────────────────────────────────────

pub struct ClientStart {
    /// Sent to the server.
    pub request: Vec<u8>,
    /// Kept by the player until the server's answer arrives.
    pub state: Vec<u8>,
}

/// Registration, step 1 (player).
pub fn client_registration_start(password: &[u8]) -> Result<ClientStart> {
    let started = ClientRegistration::<Suite>::start(&mut OsRng, password).map_err(err("registration start"))?;
    Ok(ClientStart { request: started.message.serialize().to_vec(), state: started.state.serialize().to_vec() })
}

/// Registration, step 2 (player): the message that lets the server store the record.
pub fn client_registration_finish(state: &[u8], password: &[u8], response: &[u8], username: &str) -> Result<Vec<u8>> {
    let state = ClientRegistration::<Suite>::deserialize(state).map_err(err("registration state"))?;
    let response = RegistrationResponse::<Suite>::deserialize(response).map_err(err("registration response"))?;
    let id = account_id(username);
    let done = state
        .finish(
            &mut OsRng,
            password,
            response,
            ClientRegistrationFinishParameters { identifiers: Identifiers { client: Some(&id), server: Some(SERVER_ID) }, ksf: None },
        )
        .map_err(err("registration finish"))?;
    Ok(done.message.serialize().to_vec())
}

/// Login, step 1 (player).
pub fn client_login_start(password: &[u8]) -> Result<ClientStart> {
    let started = ClientLogin::<Suite>::start(&mut OsRng, password).map_err(err("login start"))?;
    Ok(ClientStart { request: started.message.serialize().to_vec(), state: started.state.serialize().to_vec() })
}

pub struct ClientLoginDone {
    /// Sent to the server to finish the login.
    pub finalization: Vec<u8>,
    /// The key both sides now share.
    pub session_key: Vec<u8>,
}

/// Login, step 2 (player). Fails with "wrong password" if the server's answer does not match the password, which
/// also means a server that never saw this account's registration can't pass itself off as the real one.
pub fn client_login_finish(state: &[u8], password: &[u8], response: &[u8], username: &str) -> Result<ClientLoginDone> {
    let state = ClientLogin::<Suite>::deserialize(state).map_err(err("login state"))?;
    let response = CredentialResponse::<Suite>::deserialize(response).map_err(err("login response"))?;
    let id = account_id(username);
    let done = state
        .finish(
            &mut OsRng,
            password,
            response,
            ClientLoginFinishParameters::new(Some(CONTEXT), Identifiers { client: Some(&id), server: Some(SERVER_ID) }, None),
        )
        .map_err(|_| Error("wrong password".into()))?;
    Ok(ClientLoginDone { finalization: done.message.serialize().to_vec(), session_key: done.session_key.to_vec() })
}
