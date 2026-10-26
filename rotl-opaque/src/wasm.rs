//! The same functions for JavaScript (the game server runs this crate as a WebAssembly module).

use wasm_bindgen::prelude::*;

fn js<T>(r: Result<T, crate::Error>) -> Result<T, JsError> {
    r.map_err(|e| JsError::new(&e.0))
}

/// A pair of byte strings: what to send, and what to keep until the answer comes back.
#[wasm_bindgen]
pub struct Step {
    message: Vec<u8>,
    state: Vec<u8>,
}

#[wasm_bindgen]
impl Step {
    #[wasm_bindgen(getter)]
    pub fn message(&self) -> Vec<u8> { self.message.clone() }
    #[wasm_bindgen(getter)]
    pub fn state(&self) -> Vec<u8> { self.state.clone() }
}

/// The last step of a player's login: the message for the server and the key both sides now share.
#[wasm_bindgen]
pub struct LoginDone {
    message: Vec<u8>,
    session_key: Vec<u8>,
}

#[wasm_bindgen]
impl LoginDone {
    #[wasm_bindgen(getter)]
    pub fn message(&self) -> Vec<u8> { self.message.clone() }
    #[wasm_bindgen(getter, js_name = sessionKey)]
    pub fn session_key(&self) -> Vec<u8> { self.session_key.clone() }
}

#[wasm_bindgen(js_name = serverSetupNew)]
pub fn server_setup_new() -> Vec<u8> { crate::server_setup_new() }

#[wasm_bindgen(js_name = serverRegistrationStart)]
pub fn server_registration_start(setup: &[u8], request: &[u8], username: &str) -> Result<Vec<u8>, JsError> {
    js(crate::server_registration_start(setup, request, username))
}

#[wasm_bindgen(js_name = serverRegistrationFinish)]
pub fn server_registration_finish(upload: &[u8]) -> Result<Vec<u8>, JsError> { js(crate::server_registration_finish(upload)) }

/// `record` is the stored record, or empty/undefined when the account does not exist.
#[wasm_bindgen(js_name = serverLoginStart)]
pub fn server_login_start(setup: &[u8], record: Option<Vec<u8>>, request: &[u8], username: &str) -> Result<Step, JsError> {
    let started = js(crate::server_login_start(setup, record.as_deref().filter(|r| !r.is_empty()), request, username))?;
    Ok(Step { message: started.response, state: started.state })
}

#[wasm_bindgen(js_name = serverLoginFinish)]
pub fn server_login_finish(state: &[u8], finalization: &[u8], username: &str) -> Result<Vec<u8>, JsError> {
    js(crate::server_login_finish(state, finalization, username))
}

#[wasm_bindgen(js_name = clientRegistrationStart)]
pub fn client_registration_start(password: &[u8]) -> Result<Step, JsError> {
    let s = js(crate::client_registration_start(password))?;
    Ok(Step { message: s.request, state: s.state })
}

#[wasm_bindgen(js_name = clientRegistrationFinish)]
pub fn client_registration_finish(state: &[u8], password: &[u8], response: &[u8], username: &str) -> Result<Vec<u8>, JsError> {
    js(crate::client_registration_finish(state, password, response, username))
}

#[wasm_bindgen(js_name = clientLoginStart)]
pub fn client_login_start(password: &[u8]) -> Result<Step, JsError> {
    let s = js(crate::client_login_start(password))?;
    Ok(Step { message: s.request, state: s.state })
}

#[wasm_bindgen(js_name = clientLoginFinish)]
pub fn client_login_finish(state: &[u8], password: &[u8], response: &[u8], username: &str) -> Result<LoginDone, JsError> {
    let d = js(crate::client_login_finish(state, password, response, username))?;
    Ok(LoginDone { message: d.finalization, session_key: d.session_key })
}
