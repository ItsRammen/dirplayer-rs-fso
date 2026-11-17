pub mod blowfish;
pub mod writer;
pub mod reader;
pub mod types;
pub mod name_colours;
pub mod vip_badge;
pub mod inv_overlay;
pub mod page_bridge;
#[cfg(feature = "rotl-secure-login")]
pub mod secure_login;

use std::{cell::Cell, collections::VecDeque, rc::Rc};
use async_std::{channel::Sender, task::spawn_local};
use binary_reader::BinaryReader;
use fxhash::FxHashMap;
use itertools::Itertools;
use num::FromPrimitive;
use num_derive::FromPrimitive;
use wasm_bindgen::{closure::Closure, JsCast};
use web_sys::{CloseEvent, Event, MessageEvent, WebSocket};

// Use console::warn_1 directly for debugging since log level is set to Error
macro_rules! multiuser_log {
    ($($arg:tt)*) => {
        web_sys::console::warn_1(&format!($($arg)*).into())
    };
}

use crate::{
    director::{lingo::datum::{Datum, DatumType}, static_datum::{StaticDatum, static_datum_to_runtime}},
    player::{
        DatumRef, ScriptError, events::player_dispatch_callback_event, reserve_player_mut, reserve_player_ref, symbols::{builtin::BuiltInSymbol, symbol::Symbol}, xtra::multiuser::blowfish::{DEFAULT_CIPHER_KEY, MUSBlowfish}
    },
};

#[derive(Debug, Clone, FromPrimitive)]
pub enum MultiuserConnectionMode {
    Binary = 0,
    Text = 1,
}

pub struct MultiuserMessage {
    pub error_code: i32,
    pub recipients: Vec<String>,
    pub sender_id: String,
    pub subject: String,
    pub content: StaticDatum,
    pub time_stamp: u32,
}

pub struct MultiuserXtraInstance {
    pub net_message_handler: Option<(DatumRef, Symbol)>,
    pub message_queue: Vec<MultiuserMessage>,
    pub socket_tx: Option<Sender<Vec<u8>>>,
    pub connection_mode: MultiuserConnectionMode,
    pub sender_id: String,
    pub recv_buffer: Vec<u8>,
    /// The connection's socket, kept so it can be closed when the movie lets go of this instance (see `hang_up`).
    pub socket: Option<WebSocket>,
    /// Shared with the connection's send loop: set once the movie has let go of the instance, so the loop closes the
    /// socket after sending what the movie had already queued.
    pub close_when_drained: Rc<Cell<bool>>,
    /// Set when this connection logs in with OPAQUE instead of sending the password (see `secure_login`).
    #[cfg(feature = "rotl-secure-login")]
    pub secure_login: Option<secure_login::SecureLogin>,
    /// A character-creation message held while its password is registered (sent as soon as the server has answered).
    #[cfg(feature = "rotl-secure-login")]
    pub held_create: Option<MultiuserMessage>,
}

/// The one-use game ticket the website's account page left for this character (`window.rotlLoginTicket = {name, ticket}`),
/// if any. Taken, so it cannot be used again, and only for the character it was made for.
#[cfg(feature = "rotl-secure-login")]
fn take_login_ticket(username: &str) -> Option<String> {
    let window = web_sys::window()?;
    let holder = js_sys::Reflect::get(&window, &"rotlLoginTicket".into()).ok()?;
    if holder.is_undefined() || holder.is_null() {
        return None;
    }
    let _ = js_sys::Reflect::set(&window, &"rotlLoginTicket".into(), &wasm_bindgen::JsValue::UNDEFINED);
    let name = js_sys::Reflect::get(&holder, &"name".into()).ok()?.as_string()?;
    let ticket = js_sys::Reflect::get(&holder, &"ticket".into()).ok()?.as_string()?;
    if name.eq_ignore_ascii_case(username) { Some(ticket) } else { None }
}

impl MultiuserXtraInstance {
    /// The movie has let go of this instance: end its connection, as the Director Xtra does when its last reference
    /// goes. Nothing more is delivered (the socket's callbacks are taken off first, so the movie is not told about a
    /// close it asked for either), and the socket is closed once anything the movie sent just before letting go has
    /// gone out — ROTL's logout does `sendNetMessage(...)` and `gConnect = 0` back to back.
    pub fn hang_up(&mut self) {
        self.net_message_handler = None;
        self.message_queue.clear();
        self.close_when_drained.set(true);
        // Dropping the sender ends the send loop, which closes the socket on its way out. With no send loop left
        // (it ended at `breakConnection`) the socket is closed here.
        let had_send_loop = self.socket_tx.take().is_some();
        if let Some(socket) = self.socket.take() {
            socket.set_onmessage(None);
            socket.set_onerror(None);
            socket.set_onclose(None);
            socket.set_onopen(None);
            if !had_send_loop {
                let _ = socket.close();
            }
        }
    }

    pub fn dispatch_message_handler(&self) {
        if let Some((handler_obj_ref, handler_symbol)) = &self.net_message_handler {
            let handler_symbol = handler_symbol.clone();
            let handler_obj_ref = handler_obj_ref.clone();
            player_dispatch_callback_event(handler_obj_ref, handler_symbol, &vec![]);
        }
    }

    /// The server's answer to an OPAQUE login is for this Xtra, not the movie: it is consumed here, and the last step
    /// goes straight back. Anything the movie will see has the hash it sent in `LoginAuth` put back first.
    #[cfg(feature = "rotl-secure-login")]
    fn send_to_server(&self, subject: &str, content: String) {
        if let Some(tx) = &self.socket_tx {
            let message = MultiuserMessage {
                error_code: 0,
                recipients: vec!["System".to_string()],
                sender_id: self.sender_id.clone(),
                subject: subject.to_string(),
                content: StaticDatum::String(content),
                time_stamp: 0,
            };
            let _ = tx.try_send(message.to_bytes(None));
        }
    }

    /// The server's steps of an OPAQUE login are for this Xtra, not the movie: they are consumed here and answered at
    /// once. Anything the movie will see has the hash it sent in `LoginAuth` put back first.
    #[cfg(feature = "rotl-secure-login")]
    fn handle_secure_login(&mut self, message: &mut MultiuserMessage) -> bool {
        use secure_login::*;
        let Some(secure) = self.secure_login.as_mut() else { return false };
        let content = match &message.content {
            StaticDatum::String(text) => text.clone(),
            _ => String::new(),
        };
        // A wrong password (or any step that cannot be completed) ends here, on this side. The server is sent an empty
        // answer, so it counts the failed attempt and answers the movie as it does for a wrong password today.
        let step = |outcome: Result<String, String>| match outcome {
            Ok(answer) => answer,
            Err(why) => {
                multiuser_log!("Multiuser: secure login did not complete: {}", why);
                String::new()
            }
        };
        let (reply_subject, answer) = match message.subject.as_str() {
            SUBJECT_CHALLENGE => (SUBJECT_FINISH, step(secure.answer_challenge(&content))),
            SUBJECT_LEGACY => (SUBJECT_MIGRATE, step(secure.answer_legacy(&content))),
            SUBJECT_REGISTER_RESPONSE => (SUBJECT_REGISTER_FINISH, step(secure.answer_register_response(&content))),
            SUBJECT_CREATE_RESPONSE => {
                let answer = step(secure.answer_create_response(&content));
                self.send_to_server(SUBJECT_CREATE_FINISH, answer);
                // The creation message goes straight after the registration, in that order.
                if let (Some(held), Some(tx)) = (self.held_create.take(), &self.socket_tx) {
                    let _ = tx.try_send(held.to_bytes(None));
                }
                return true;
            }
            _ => {
                secure.restore_incoming(&message.subject, &mut message.content);
                return false;
            }
        };
        self.send_to_server(reply_subject, answer);
        true
    }

    pub fn dispatch_message(&mut self, message: MultiuserMessage) {
        // The server's list of names with a colour of their own is for the player, not the movie (see `name_colours`).
        if message.subject == name_colours::SUBJECT {
            if let StaticDatum::String(list) = &message.content {
                name_colours::replace(list);
            }
            return;
        }
        // The inventory pages' reply is for the web page, not the movie (see `inv_overlay`).
        if message.subject == inv_overlay::SUBJECT {
            if let StaticDatum::String(json) = &message.content {
                inv_overlay::deliver(json);
            }
            return;
        }
        // What the Modern UI page draws (party, currencies, date, teleport list, ...) is offered to it first (see `page_bridge`).
        if let StaticDatum::String(content) = &message.content {
            if page_bridge::offer(&message.subject, content) {
                return;
            }
        }
        // The server's `[VIP]` tag in a chat line becomes a star (see `vip_badge`).
        let mut message = message;
        let swap: Option<fn(&str) -> Option<String>> = match message.subject.as_str() {
            vip_badge::SUBJECT => Some(vip_badge::swap_tag),
            vip_badge::GUILD_SUBJECT => Some(vip_badge::swap_guild_tag),
            _ => None,
        };
        if let (Some(swap), StaticDatum::String(line)) = (swap, &message.content) {
            if let Some(swapped) = swap(line) {
                message.content = StaticDatum::String(swapped);
            }
        }
        #[cfg(feature = "rotl-secure-login")]
        let message = {
            let mut message = message;
            if self.handle_secure_login(&mut message) {
                return;
            }
            message
        };
        self.message_queue.push(message);
        self.dispatch_message_handler();
    }

    pub fn next_message(&mut self) -> Option<MultiuserMessage> {
        if self.message_queue.is_empty() {
            return None;
        }
        let mut message = self.message_queue.remove(0);
        // A profile's badge rides on its last item: taken off here, kept for the profile window's name (see `vip_badge`).
        vip_badge::take_profile_badge(&message.subject, &mut message.content);
        Some(message)
    }

    pub fn receive_binary_data(&mut self, data: &[u8]) {
        use crate::player::xtra::multiuser::reader::{MUS_HEADER, MUS_FRAME_HEADER_SIZE, MusReader};

        self.recv_buffer.extend_from_slice(data);
        loop {
            if self.recv_buffer.len() < MUS_FRAME_HEADER_SIZE {
                break;
            }
            let header = u16::from_be_bytes([self.recv_buffer[0], self.recv_buffer[1]]);
            if header != MUS_HEADER {
                multiuser_log!("Multiuser: Invalid message header 0x{:04x}, discarding recv buffer", header);
                self.recv_buffer.clear();
                break;
            }
            let payload_size = u32::from_be_bytes([
                self.recv_buffer[2], self.recv_buffer[3],
                self.recv_buffer[4], self.recv_buffer[5],
            ]) as usize;
            let total_size = MUS_FRAME_HEADER_SIZE + payload_size;
            if self.recv_buffer.len() < total_size {
                break;
            }
            let msg_bytes: Vec<u8> = self.recv_buffer.drain(..total_size).collect();
            let payload = &msg_bytes[MUS_FRAME_HEADER_SIZE..];
            match BinaryReader::read_mus_message_payload(payload, None) {
                Ok(msg) => self.dispatch_message(msg),
                Err(e) => multiuser_log!("Multiuser: Failed to read binary message: {:?}", e),
            }
        }
    }
}

pub struct MultiuserXtraManager {
    pub instances: FxHashMap<u32, MultiuserXtraInstance>,
    pub instance_counter: u32,
}

impl MultiuserXtraManager {
    pub fn create_instance(&mut self, _: &Vec<DatumRef>) -> u32 {
        self.instance_counter += 1;
        self.instances.insert(
            self.instance_counter,
            MultiuserXtraInstance {
                net_message_handler: None,
                message_queue: vec![],
                socket_tx: None,
                connection_mode: MultiuserConnectionMode::Binary,
                sender_id: String::new(),
                recv_buffer: Vec::new(),
                socket: None,
                close_when_drained: Rc::new(Cell::new(false)),
                #[cfg(feature = "rotl-secure-login")]
                secure_login: None,
                #[cfg(feature = "rotl-secure-login")]
                held_create: None,
            },
        );
        self.instance_counter
    }

    pub fn has_instance_async_handler(_name: &str) -> bool {
        false
    }

    pub async fn call_instance_async_handler(
        handler_name: &str,
        instance_id: u32,
        _args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        Err(ScriptError::new(format!(
            "No async handler {} found for Multiuser xtra instance #{}",
            handler_name, instance_id
        )))
    }

    pub fn call_instance_handler(
        handler_name: &str,
        instance_id: u32,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        match handler_name.to_lowercase().as_str() {
            "setnetbufferlimits" => Ok(DatumRef::Void),
            "setnetmessagehandler" => {
                let mut multiusr_manager = unsafe { MULTIUSER_XTRA_MANAGER_OPT.as_mut().unwrap() };
                let instance = multiusr_manager.instances.get_mut(&instance_id).unwrap();
                reserve_player_mut(|player| {
                    let handler_symbol = player.get_datum(args.get(0).unwrap());
                    let handler_obj_ref = args.get(1).unwrap().clone();
                    // TODO subject and sender
                    if handler_symbol.is_void() {
                        instance.net_message_handler = None;
                    } else {
                        let handler_symbol = handler_symbol.symbol_value()?;
                        instance.net_message_handler = Some((handler_obj_ref, handler_symbol));
                    }

                    // TODO return error code?
                    Ok(player.alloc_datum(Datum::Int(0)))
                })
            }
            "connecttonetserver" => {
                let mut multiusr_manager = unsafe { MULTIUSER_XTRA_MANAGER_OPT.as_mut().unwrap() };
                let instance = multiusr_manager.instances.get_mut(&instance_id).unwrap();
                if let Some((handler_obj_ref, handler_symbol)) = &instance.net_message_handler {
                    let _handler_symbol = handler_symbol.clone();
                    let _handler_obj_ref = handler_obj_ref.clone();
                }
                // Director's connectToNetServer has two overloads:
                //   (A) (userName, password, serverID, port, movieID, [mode, encKey])
                //   (B) (serverID, port, propList[#userID, #password, #movieid],
                //        modeSymbol, [encKey])
                // Form B is what ClubMarian / MaidMarian's Connection
                // Script uses. Detect it by sniffing args[2]: a PropList
                // means form B; otherwise assume form A.
                let (username, password, host, port, movie_id, mode, encryption_key) = reserve_player_ref(|player| {
                    let arg2 = args.get(2).map(|d| player.get_datum(d));
                    let is_form_b = matches!(arg2, Some(Datum::PropList(..)));
                    if is_form_b {
                        // Form B: (host, port, plist, mode, [encKey])
                        let host = player.get_datum(args.get(0).unwrap()).string_value()?;
                        let port = player.get_datum(args.get(1).unwrap()).int_value()?;
                        let plist = player.get_datum(args.get(2).unwrap());
                        let lookup_str = |key: BuiltInSymbol| -> String {
                            if let Datum::PropList(pairs, _) = plist {
                                for (k, v) in pairs.iter() {
                                    let kd = player.get_datum(k);
                                    let kd_symbol = kd.symbol_value().and_then(|x| x.into_builtin_or_error());
                                    let key_matches = kd_symbol.is_ok_and(|x| x == key);
                                    if key_matches {
                                        return player.get_datum(v).string_value().unwrap_or_default();
                                    }
                                }
                            }
                            String::default()
                        };
                        let username = lookup_str(BuiltInSymbol::UserID);
                        let password = lookup_str(BuiltInSymbol::Password);
                        let movie_id = lookup_str(BuiltInSymbol::Movieid);
                        let mode = args.get(3).and_then(|d| player.get_datum(d).int_value().ok()).unwrap_or(0);
                        let encryption_key = args.get(4).and_then(|d| player.get_datum(d).string_value().ok()).unwrap_or_default();
                        Ok((username, password, host, port, movie_id, mode, encryption_key))
                    } else {
                        // Form A: (userName, password, serverID, port, movieID, ...)
                        let username = player.get_datum(args.get(0).unwrap()).string_value().unwrap_or_default();
                        let password = player.get_datum(args.get(1).unwrap()).string_value().unwrap_or_default();
                        let host = player.get_datum(args.get(2).unwrap()).string_value()?;
                        let port = player.get_datum(args.get(3).unwrap()).int_value()?;
                        let movie_id = player.get_datum(args.get(4).unwrap()).string_value().unwrap_or_default();
                        let mode = args.get(5).and_then(|d| player.get_datum(d).int_value().ok()).unwrap_or(0);
                        let encryption_key = args.get(6).and_then(|d| player.get_datum(d).string_value().ok()).unwrap_or_default();
                        Ok((username, password, host, port, movie_id, mode, encryption_key))
                    }
                })?;

                let window_secure = web_sys::window()
                    .and_then(|w| w.location().protocol().ok())
                    .map_or(false, |p| p == "https:");
                // Log in without sending the password, when this build has it and the page asks for it.
                #[cfg(feature = "rotl-secure-login")]
                let secure_requested = reserve_player_ref(|player| {
                    player.external_params.get("secure_login").is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                });
                let (ws_path, ws_secure) = reserve_player_ref(|player| {
                    (
                        player.external_params.get("multiuser_websocket_path").cloned(),
                        player.external_params.get("multiuser_websocket_ssl")
                            .and_then(|v|{
                                if v.is_empty() { 
                                    return None;
                                } else {
                                    Some(v)
                                }
                            })
                            .map(|v| {
                                // parse bool
                                v == "true" || v == "1"
                            }),
                    )
                });

                // If ws_secure param is set, it overrides the page protocol check for ws_scheme
                let ws_secure = ws_secure.unwrap_or(window_secure);
                let ws_scheme = if ws_secure {
                    "wss"
                } else {
                    "ws"
                };
                let ws_url = format!("{}://{}:{}{}", ws_scheme, host, port, ws_path.unwrap_or_default());

                // Check if the page provides a socket proxy mapping via JS
                let ws_url = {
                    let default_url = ws_url.clone();
                    let window = web_sys::window().unwrap();
                    if let Ok(resolver) = js_sys::Reflect::get(&window, &"dirplayerResolveSocketUrl".into()) {
                        if let Some(func) = resolver.dyn_ref::<js_sys::Function>() {
                            if let Ok(result) = func.call2(&wasm_bindgen::JsValue::NULL, &host.as_str().into(), &wasm_bindgen::JsValue::from_f64(port as f64)) {
                                if let Some(url) = result.as_string() {
                                    if !url.is_empty() {
                                        url
                                    } else {
                                        default_url
                                    }
                                } else {
                                    default_url
                                }
                            } else {
                                default_url
                            }
                        } else {
                            default_url
                        }
                    } else {
                        default_url
                    }
                };
                multiuser_log!("Multiuser: Connecting to WebSocket URL: {} (original={}:{}, user={}, movie={})", ws_url, host, port, username, movie_id);

                instance.sender_id = username.clone();
                instance.connection_mode = if let Some(mode) = MultiuserConnectionMode::from_i32(mode) {
                    mode
                } else {
                    return Err(ScriptError::new(format!("Invalid connection mode: {}", mode)));
                };

                let socket = match WebSocket::new(&ws_url) {
                    Ok(s) => s,
                    Err(e) => {
                        multiuser_log!("Multiuser: Failed to create WebSocket: {:?}", e);
                        // Dispatch error message to handler
                        instance.dispatch_message(MultiuserMessage {
                            error_code: -1,
                            recipients: vec![],
                            sender_id: "System".to_string(),
                            subject: "ConnectToNetServer".to_string(),
                            content: StaticDatum::String(format!("Failed to create WebSocket: {:?}", e)),
                            time_stamp: 0,
                        });
                        return Ok(DatumRef::Void);
                    }
                };
                socket.set_binary_type(web_sys::BinaryType::Arraybuffer);

                // Log initial socket state
                multiuser_log!("Multiuser: WebSocket created, readyState={}", socket.ready_state());

                let socket_clone = socket.clone();
                let ws_url_clone = ws_url.clone();
                // Snapshot the player generation at connection time. All four
                // callbacks check this before touching global state so that
                // forgotten (leaked) closures from a previous test cannot fire
                // on the new test's manager/player after a reset.
                let conn_generation = unsafe { crate::player::PLAYER_GENERATION };
                let onmessage_callback = Closure::<dyn FnMut(_)>::new(move |e: MessageEvent| {
                    if unsafe { crate::player::PLAYER_GENERATION } != conn_generation { return; }
                    // A movie that has just let go of the connection gets nothing more from it, even if this
                    // message beat the next frame's check.
                    release_unreferenced_instances();
                    // Handle both ArrayBuffer and String messages
                    let data = e.data();
                    let message_bytes = if let Ok(array_buffer) = data.clone().dyn_into::<js_sys::ArrayBuffer>() {
                        let array = js_sys::Uint8Array::new(&array_buffer);
                        array.to_vec()
                    } else if let Ok(js_string) = data.dyn_into::<js_sys::JsString>() {
                        // js_string.as_string().unwrap().into_bytes()
                        js_string.as_string().unwrap_or_default().chars().map(|c| c as u8).collect()
                    } else {
                        multiuser_log!("Multiuser: Received unknown message type");
                        return;
                    };
                    // let message_str = if let Ok(array_buffer) = data.clone().dyn_into::<js_sys::ArrayBuffer>() {
                    //     let array = js_sys::Uint8Array::new(&array_buffer);
                    //     let vec = array.to_vec();
                    //     let char_vec: Vec<char> = vec.into_iter().map(|byte| byte as char).collect();
                    //     char_vec.into_iter().collect()
                    // } else if let Ok(js_string) = data.dyn_into::<js_sys::JsString>() {
                    //     js_string.as_string().unwrap_or_default()
                    // } else {
                    //     multiuser_log!("Multiuser: Received unknown message type");
                    //     return;
                    // };
                    // multiuser_log!("Multiuser: WebSocket message received: {:?}", message_str);

                    let multiusr_manager =
                        unsafe { MULTIUSER_XTRA_MANAGER_OPT.as_mut() };
                    if let Some(manager) = multiusr_manager {
                        if let Some(instance) = manager.instances.get_mut(&instance_id) {
                            match instance.connection_mode {
                                MultiuserConnectionMode::Text => {
                                    // A Unicode Director (11.5) Multiuser Xtra hands
                                    // the movie UNICODE text — it decodes the wire
                                    // itself. Habbo relies on that: `String Services
                                    // Class.decodeUTF8` opens with
                                    //   `if pUnicodeDirector and not tForceDecode then return tStr`
                                    // and `pUnicodeDirector` is 1 whenever
                                    // `_player.productVersion >= 11`, so the client
                                    // deliberately does NOT decode incoming chat.
                                    //
                                    // Mapping each byte to a char (Latin-1) therefore
                                    // surfaced the server's UTF-8 verbatim: `ö` came
                                    // back as `C3 B6` and rendered as two characters,
                                    // which is the `ÄÖÄÖ` garbling in v26/v31 chat.
                                    // v7 predates all of this (`pUnicodeDirector = 0`,
                                    // no `encodeUTF8`) and sends plain Latin-1, so it
                                    // never showed the fault.
                                    //
                                    // `decode_text_auto` decodes valid UTF-8 as UTF-8
                                    // and falls back to per-byte for anything else, so
                                    // a Latin-1 server (v7) is untouched.
                                    let message_str =
                                        crate::io::encoding::decode_text_auto(&message_bytes);
                                    instance.dispatch_message(MultiuserMessage {
                                        error_code: 0,
                                        recipients: vec!["*".to_string()],
                                        sender_id: "System".to_string(),
                                        subject: "String".to_string(),
                                        content: StaticDatum::String(message_str),
                                        time_stamp: 0,
                                    });
                                }
                                MultiuserConnectionMode::Binary => {
                                    instance.receive_binary_data(&message_bytes);
                                }
                            }
                        }
                    }
                });

                let onerror_callback = Closure::<dyn FnMut(_)>::new(move |_e: Event| {
                    if unsafe { crate::player::PLAYER_GENERATION } != conn_generation { return; }
                    // Note: WebSocket error events typically don't provide detailed error info.
                    // The ErrorEvent's message/filename/etc. fields are often undefined for WebSocket errors.
                    // We just log a generic error message.
                    multiuser_log!("Multiuser: WebSocket error occurred (connection failed or aborted)");

                    let mut multiusr_manager =
                        unsafe { MULTIUSER_XTRA_MANAGER_OPT.as_mut().unwrap() };
                    if let Some(instance) = multiusr_manager.instances.get_mut(&instance_id) {
                        instance.dispatch_message(MultiuserMessage {
                            error_code: -1,
                            recipients: vec![],
                            sender_id: "System".to_string(),
                            subject: "ConnectToNetServer".to_string(),
                            content: StaticDatum::String("WebSocket connection error".to_string()),
                            time_stamp: 0,
                        });
                    }
                });

                let onclose_callback = Closure::<dyn FnMut(_)>::new(move |e: CloseEvent| {
                    if unsafe { crate::player::PLAYER_GENERATION } != conn_generation { return; }
                    multiuser_log!("Multiuser: WebSocket closed - code: {}, reason: '{}', wasClean: {}",
                        e.code(), e.reason(), e.was_clean());

                    let mut multiusr_manager =
                        unsafe { MULTIUSER_XTRA_MANAGER_OPT.as_mut().unwrap() };
                    if let Some(instance) = multiusr_manager.instances.get_mut(&instance_id) {
                        // Report a dropped connection the way the Director Xtra
                        // does: an error that getNetErrorString describes as
                        // "There is no current connection". The WebSocket close
                        // code (1000/1006) meant nothing to movies — ROTL keys its
                        // "You have lost your connection" handling on that text.
                        instance.dispatch_message(MultiuserMessage {
                            error_code: -5,
                            recipients: vec![],
                            sender_id: "System".to_string(),
                            subject: "DisconnectFromServer".to_string(),
                            content: StaticDatum::String(e.reason()),
                            time_stamp: 0,
                        });
                    }
                });

                let onopen_callback = Closure::<dyn FnMut(_)>::new(move |_e: Event| {
                    if unsafe { crate::player::PLAYER_GENERATION } != conn_generation { return; }
                    multiuser_log!("Multiuser: WebSocket connected to {}", ws_url_clone);
                    let multiusr_manager =
                        unsafe { MULTIUSER_XTRA_MANAGER_OPT.as_mut() };
                    let Some(manager) = multiusr_manager else { return; };
                    let Some(instance) = manager.instances.get_mut(&instance_id) else { return; };
                    instance.dispatch_message(MultiuserMessage {
                        error_code: 0,
                        recipients: vec!["*".to_string()],
                        sender_id: "System".to_string(),
                        subject: "ConnectToNetServer".to_string(),
                        content: StaticDatum::Void,
                        time_stamp: 0, // TODO timestamp
                    });
                    if let MultiuserConnectionMode::Binary = instance.connection_mode {
                        // What goes where the password would be: the password, or (secure login) a public request.
                        #[cfg(feature = "rotl-secure-login")]
                        let password_field = if secure_requested {
                            // "Continue as ...": the page leaves a one-use ticket for this character; otherwise a normal login.
                            let begun = match take_login_ticket(&username) {
                                Some(ticket) => secure_login::SecureLogin::with_ticket(&username, &password, &ticket),
                                None => secure_login::SecureLogin::begin(&username, &password),
                            };
                            match begun {
                                Ok((login, field)) => {
                                    instance.secure_login = Some(login);
                                    field
                                }
                                Err(why) => {
                                    multiuser_log!("Multiuser: secure login could not start: {}", why);
                                    instance.dispatch_message(MultiuserMessage {
                                        error_code: -1,
                                        recipients: vec![],
                                        sender_id: "System".to_string(),
                                        subject: "ConnectToNetServer".to_string(),
                                        content: StaticDatum::String("Secure login could not start".to_string()),
                                        time_stamp: 0,
                                    });
                                    return;
                                }
                            }
                        } else {
                            password.clone()
                        };
                        #[cfg(not(feature = "rotl-secure-login"))]
                        let password_field = password.clone();
                        let message = MultiuserMessage {
                            error_code: 0,
                            recipients: vec!["System".to_string()],
                            sender_id: username.to_string(),
                            subject: "Logon".to_string(),
                            content: StaticDatum::List(vec![
                                StaticDatum::String(movie_id.clone()),
                                StaticDatum::String(username.clone()),
                                StaticDatum::String(password_field),
                            ]),
                            time_stamp: 0,
                        };
                        let resolved_encryption_key: Vec<u8> = if encryption_key.is_empty() {
                            DEFAULT_CIPHER_KEY.to_vec()
                        } else if encryption_key.len() < 20 {
                            // DEFAULT_CIPHER_KEY + &encryption_key
                            let mut key = DEFAULT_CIPHER_KEY.to_vec();
                            key.extend_from_slice(encryption_key.as_bytes());
                            key
                        } else {
                            encryption_key.as_bytes().to_vec()
                        };
                        let mut cipher = MUSBlowfish::new(&resolved_encryption_key);
                        let bytes = message.to_bytes(Some(&mut cipher));
                        instance.socket_tx.as_ref().unwrap().try_send(bytes.to_vec()).unwrap();
                    }
                });
                socket.set_onmessage(Some(onmessage_callback.as_ref().unchecked_ref()));
                socket.set_onerror(Some(onerror_callback.as_ref().unchecked_ref()));
                socket.set_onclose(Some(onclose_callback.as_ref().unchecked_ref()));
                socket.set_onopen(Some(onopen_callback.as_ref().unchecked_ref()));

                let (tx, rx) = async_std::channel::unbounded();
                instance.socket_tx = Some(tx);
                instance.socket = Some(socket.clone());
                // A flag of this connection's own: a second connectToNetServer on the same instance starts afresh.
                let close_when_drained = Rc::new(Cell::new(false));
                instance.close_when_drained = close_when_drained.clone();
                crate::player::spawn_player_local(async move {
                    while let Ok(message) = rx.recv().await {
                        // Check if WebSocket is open (readyState == 1) before sending
                        // readyState: 0 = CONNECTING, 1 = OPEN, 2 = CLOSING, 3 = CLOSED
                        if socket_clone.ready_state() != 1 {
                            multiuser_log!("Multiuser: Cannot send message, WebSocket not open (state={})", socket_clone.ready_state());
                            continue;
                        }
                        // multiuser_log!("Multiuser: Sending message: {:?}", message);
                        if let Err(e) = socket_clone.send_with_u8_array(&message) {
                            multiuser_log!("Multiuser: Failed to send message: {:?}", e);
                        }
                    }
                    // The sender is gone and everything queued has been sent. If that is because the movie let go
                    // of the instance, the connection ends here (see `MultiuserXtraInstance::hang_up`).
                    if close_when_drained.get() {
                        multiuser_log!("Multiuser: the movie released its connection; closing the WebSocket");
                        let _ = socket_clone.close();
                    }
                });

                // Forget the callbacks to keep them alive
                onmessage_callback.forget();
                onerror_callback.forget();
                onclose_callback.forget();
                onopen_callback.forget();

                Ok(DatumRef::Void)
            }
            "getnetmessage" => {
                let mut multiusr_manager = unsafe { MULTIUSER_XTRA_MANAGER_OPT.as_mut().unwrap() };
                let instance = multiusr_manager.instances.get_mut(&instance_id).unwrap();
                if let Some(message) = instance.next_message() {
                    reserve_player_mut(|player| {
                        let recipient_refs: VecDeque<DatumRef> = message
                            .recipients
                            .iter()
                            .map(|recipient| player.alloc_datum(Datum::String(recipient.clone())))
                            .collect();

                        let error_code = player.alloc_datum(Datum::Int(message.error_code));
                        let recipients =
                            player.alloc_datum(Datum::List(DatumType::List, recipient_refs, false));
                        let sender_id = player.alloc_datum(Datum::String(message.sender_id));
                        let subject = player.alloc_datum(Datum::String(message.subject));
                        let content = static_datum_to_runtime(&message.content, &mut player.allocator);
                        let time_stamp = player.alloc_datum(Datum::Int(message.time_stamp as i32)); // TODO: i64

                        // SYMBOL keys, not strings. The Multiuser Xtra delivers its
                        // message as a prop list keyed by #errorCode / #recipients /
                        // #senderID / #subject / #content / #timeStamp, and movies read
                        // it that way — Habbo v7's Connection Instance Class does
                        // `tNewMsg.getaProp(#errorCode)`, `pMsgStruct.getaProp(#content)`,
                        // `tMsg[#content]`.
                        //
                        // Director never matches a STRING key against a SYMBOL probe
                        // (measured: `["Foo":1].getaProp(#foo)` -> VOID), so building
                        // these as strings made every such read miss. It only appeared
                        // to work while our prop-list compare matched across the two
                        // types — and that same cross-type leniency is what let a symbol
                        // lookup alias onto a string registry key and send Habbo v7's
                        // room teardown into the Object/Window Manager recursion.
                        // Emitting the documented key type fixes the cause instead.
                        let error_code_key =
                            player.alloc_datum(Datum::Symbol(Symbol::from_str("errorCode")));
                        let recipients_key =
                            player.alloc_datum(Datum::Symbol(Symbol::from_str("recipients")));
                        let sender_id_key =
                            player.alloc_datum(Datum::Symbol(Symbol::from_str("senderID")));
                        let subject_key =
                            player.alloc_datum(Datum::Symbol(Symbol::from_str("subject")));
                        let content_key =
                            player.alloc_datum(Datum::Symbol(Symbol::from_str("content")));
                        let time_stamp_key =
                            player.alloc_datum(Datum::Symbol(Symbol::from_str("timeStamp")));

                        Ok(player.alloc_datum(Datum::PropList(
                            VecDeque::from(vec![
                                (error_code_key, error_code),
                                (recipients_key, recipients),
                                (sender_id_key, sender_id),
                                (subject_key, subject),
                                (content_key, content),
                                (time_stamp_key, time_stamp),
                            ]),
                            false,
                        )))
                    })
                } else {
                    Ok(DatumRef::Void)
                }
            }
            "sendnetmessage" => {
                let mut multiusr_manager = unsafe { MULTIUSER_XTRA_MANAGER_OPT.as_mut().unwrap() };
                let instance = multiusr_manager.instances.get_mut(&instance_id).unwrap();
                reserve_player_mut(|player| {
                    // multiuser_log!("sendNetMessage: {:?}", msg_string);
                    // Director accepts two forms:
                    //   sendNetMessage(recipients, subject, content)
                    //   sendNetMessage([#recipients: r, #subject: s, #content: c])
                    // Resolve both to the same three datums; a missing one is VOID.
                    let (recipients_ref, subject_ref, content_ref) = match (args.len(), args.get(0).map(|d| player.get_datum(d))) {
                        (1, Some(Datum::PropList(pairs, _))) => {
                            let find = |name: &str| pairs.iter()
                                .find(|(k, _)| player.get_datum(k).string_value().is_ok_and(|s| s.eq_ignore_ascii_case(name)))
                                .map(|(_, v)| v.clone());
                            (find("recipients"), find("subject"), find("content"))
                        }
                        _ => (args.get(0).cloned(), args.get(1).cloned(), args.get(2).cloned()),
                    };
                    if let Some(tx) = &instance.socket_tx {
                        let msg_bytes = match instance.connection_mode {
                            MultiuserConnectionMode::Text => {
                                let Some(content_ref) = &content_ref else {
                                    return Err(ScriptError::new("sendNetMessage: missing content".to_string()));
                                };
                                let msg_data = player.get_datum(content_ref);
                                let s = msg_data.string_value()?;
                                // Mirror of the receive side: a Unicode Director's
                                // Multiuser Xtra puts UTF-8 on the wire. Truncating
                                // each char to a byte (`c as u8`) silently mangled
                                // anything outside Latin-1 and disagreed with what the
                                // server sends back.
                                s.into_bytes()
                            }
                            MultiuserConnectionMode::Binary => {
                                let Some(subject_ref) = &subject_ref else {
                                    return Err(ScriptError::new("sendNetMessage: missing subject".to_string()));
                                };
                                let subject = player.get_datum(subject_ref).string_value()?;
                                #[cfg_attr(not(feature = "rotl-secure-login"), allow(unused_mut))]
                                let mut content = match &content_ref {
                                    Some(r) => StaticDatum::from(player.get_datum(r)),
                                    None => StaticDatum::Void,
                                };
                                // With a secure login the password stays on this side: the messages the movie puts it
                                // in are rewritten on their way out.
                                #[cfg(feature = "rotl-secure-login")]
                                let mut start_create: Option<String> = None;
                                #[cfg(feature = "rotl-secure-login")]
                                if let Some(secure) = instance.secure_login.as_mut() {
                                    secure.redact_outgoing(&subject, &mut content);
                                    if let secure_login::Outgoing::StartCreate(start) = secure.prepare_outgoing(&subject, &mut content) {
                                        start_create = Some(start);
                                    }
                                }
                                let void = Datum::Void;
                                let recipients_datum = match &recipients_ref {
                                    Some(r) => player.get_datum(r),
                                    None => &void,
                                };
                                let recipients_list = match recipients_datum {
                                    Datum::List(_, list, ..) => list.iter().map(|d| player.get_datum(d).string_value().unwrap_or_default()).collect_vec(),
                                    Datum::String(s) => vec![s.clone()],
                                    Datum::Int(0) | Datum::Void => vec![],
                                    _ => return Err(ScriptError::new("Invalid recipients argument, expected list or string".to_string())),
                                };

                                let message = MultiuserMessage {
                                    error_code: 0,
                                    recipients: recipients_list,
                                    sender_id: instance.sender_id.clone(),
                                    subject,
                                    content,
                                    time_stamp: 0,
                                };
                                // A new character's password is registered first: hold this message (the password is
                                // already out of it) and send the registration's first step in its place.
                                #[cfg(feature = "rotl-secure-login")]
                                if let Some(start) = start_create {
                                    instance.held_create = Some(message);
                                    instance.send_to_server(secure_login::SUBJECT_CREATE, start);
                                    return Ok(DatumRef::Void);
                                }
                                message.to_bytes(None)
                            }
                        };
                        tx.try_send(msg_bytes).unwrap();
                        Ok(DatumRef::Void)
                    } else {
                        Err(ScriptError::new("Socket not connected".to_string()))
                    }
                })
            }
            "getnetaddresscookie" => {
                reserve_player_mut(|player| {
                    Ok(player.alloc_datum(Datum::String("".to_string())))
                })
            }
            "getnumberwaitingnetmessages" => {
                let multiusr_manager = unsafe { MULTIUSER_XTRA_MANAGER_OPT.as_mut().unwrap() };
                let instance = multiusr_manager.instances.get(&instance_id).unwrap();
                let count = instance.message_queue.len() as i32;
                reserve_player_mut(|player| {
                    Ok(player.alloc_datum(Datum::Int(count)))
                })
            }
            "checknetmessages" => {
                // Process pending messages — in our implementation messages are already
                // dispatched via callbacks, so this is effectively a no-op
                Ok(DatumRef::Void)
            }
            "breakconnection" => {
                // Close the WebSocket connection
                let multiusr_manager = unsafe { MULTIUSER_XTRA_MANAGER_OPT.as_mut().unwrap() };
                if let Some(instance) = multiusr_manager.instances.get_mut(&instance_id) {
                    // Drop the sender to close the send loop; the WebSocket
                    // close is handled by the browser when the socket is dropped
                    instance.socket_tx = None;
                }
                Ok(DatumRef::Void)
            }
            "getneterrorstring" => {
                // Return a human-readable error string for an error code
                let error_code = reserve_player_ref(|player| {
                    player.get_datum(args.get(0).unwrap()).int_value()
                })?;
                let error_string = match error_code {
                    0 => "No error",
                    -1 => "Connection failed",
                    -2 => "Connection refused",
                    -3 => "Connection timed out",
                    -4 => "Invalid message",
                    -5 => "There is no current connection.",
                    _ => "Unknown error",
                };
                reserve_player_mut(|player| {
                    Ok(player.alloc_datum(Datum::String(error_string.to_string())))
                })
            }
            "getpeerconnectionlist" => {
                // Return an empty list — we don't track peer connections in the client
                reserve_player_mut(|player| {
                    Ok(player.alloc_datum(Datum::List(DatumType::List, VecDeque::new(), false)))
                })
            }
            "waitfornetconnection" => {
                // Server-side: start listening for incoming connections
                // In a browser WASM context, we can't act as a server — return error code
                reserve_player_mut(|player| {
                    Ok(player.alloc_datum(Datum::Int(-1)))
                })
            }
            _ => Err(ScriptError::new(format!(
                "No handler {} found for Multiuser xtra instance #{}",
                handler_name, instance_id
            ))),
        }
    }

    pub fn new() -> MultiuserXtraManager {
        unsafe { RELEASE_CANDIDATES.clear(); }
        MultiuserXtraManager {
            instances: FxHashMap::default(),
            instance_counter: 0,
        }
    }
}

/// Instances that lost a reference since the last check and may now have none (see `release_unreferenced_instances`).
static mut RELEASE_CANDIDATES: Vec<u32> = Vec::new();

/// Told by the allocator that a datum holding an Xtra instance was freed. That is only a hint: `Datum` is `Clone`, so
/// several datums can hold the same instance, and this one may not have been the last.
pub fn note_instance_datum_freed(xtra_name: &str, instance_id: u32) {
    if !xtra_name.eq_ignore_ascii_case("multiuser") {
        return;
    }
    unsafe {
        if !RELEASE_CANDIDATES.contains(&instance_id) {
            RELEASE_CANDIDATES.push(instance_id);
        }
    }
}

/// Whether any datum, in the host player or a nested `#movie` player, still holds this instance. Everything a movie
/// can keep a value in (globals, properties, lists, handler locals, the stack) is a `DatumRef` into one of these
/// arenas, so no hit means the movie can no longer reach the instance.
fn instance_is_referenced(instance_id: u32) -> bool {
    let holds = |player: &crate::player::DirPlayer| {
        player.allocator.datums.iter().any(|(_, entry)| {
            matches!(&entry.datum, Datum::XtraInstance(name, id) if *id == instance_id && name.eq_ignore_ascii_case("multiuser"))
        })
    };
    unsafe {
        crate::player::PLAYER_OPT.as_ref().is_some_and(&holds)
            || crate::player::NESTED_PLAYERS.iter().flatten().any(&holds)
    }
}

/// Ends the connection of every Multiuser instance the movie has let go of.
///
/// Director closes a Multiuser connection when the last reference to its Xtra instance goes; it is how movies log
/// out (ROTL: `gConnect = 0`). Here an instance lives in the manager's table under an id and a `Datum::XtraInstance`
/// only names that id, so nothing happened when the datum went: the socket stayed open, the character stayed online,
/// and the server's messages kept calling the movie's handler.
///
/// The check is a search rather than a count because datums are freed in more than one way and copied by value. The
/// allocator notes an instance each time a datum naming it is freed (`note_instance_datum_freed`); here, once per
/// frame and before each incoming message, every noted instance is looked for across the live datums and released if
/// none names it. With nothing noted — the usual case — this costs one length check.
pub fn release_unreferenced_instances() {
    let candidates = unsafe {
        if RELEASE_CANDIDATES.is_empty() {
            return;
        }
        std::mem::take(&mut RELEASE_CANDIDATES)
    };
    for instance_id in candidates {
        if instance_is_referenced(instance_id) {
            continue;
        }
        let Some(manager) = (unsafe { MULTIUSER_XTRA_MANAGER_OPT.as_mut() }) else { continue };
        if let Some(mut instance) = manager.instances.remove(&instance_id) {
            instance.hang_up();
        }
    }
}

pub fn borrow_multiuser_manager_mut<T>(callback: impl FnOnce(&mut MultiuserXtraManager) -> T) -> T {
    let mut manager = unsafe { MULTIUSER_XTRA_MANAGER_OPT.as_mut().unwrap() };
    callback(&mut *manager)
}

// lazy_static! {
//     pub static ref MULTIUSER_XTRA_MANAGER: Arc<Mutex<MultiuserXtraManager>> =
//         Arc::new(Mutex::new(MultiuserXtraManager::new()));
// }

pub static mut MULTIUSER_XTRA_MANAGER_OPT: Option<MultiuserXtraManager> = None;

#[cfg(test)]
mod release_tests {
    use super::*;
    use crate::player::symbols::symbol_table::init_symbol_table;
    use crate::player::testing::{run_test, TestPlayer};

    /// `new(xtra "Multiuser")`: an instance in the manager and the datum the movie holds it by.
    fn new_connection() -> (u32, DatumRef) {
        let id = borrow_multiuser_manager_mut(|manager| manager.create_instance(&vec![]));
        let datum = reserve_player_mut(|player| player.alloc_datum(Datum::XtraInstance("Multiuser".to_string(), id)));
        (id, datum)
    }

    fn is_open(id: u32) -> bool {
        borrow_multiuser_manager_mut(|manager| manager.instances.contains_key(&id))
    }

    /// ROTL logs out with `gConnect = 0`: the only reference goes, and the connection goes with it.
    #[test]
    fn the_connection_ends_when_the_movie_drops_its_only_reference() {
        init_symbol_table();
        run_test(async {
            let _p = TestPlayer::new();
            let (id, g_connect) = new_connection();
            release_unreferenced_instances();
            assert!(is_open(id), "released while the movie still held it");
            drop(g_connect);
            assert!(is_open(id), "released before the frame's check");
            release_unreferenced_instances();
            assert!(!is_open(id), "the instance outlived its last reference");
        });
    }

    /// A second variable, a list entry, or a copy of the value all keep the connection: only the last one to go ends it.
    #[test]
    fn the_connection_stays_while_anything_still_refers_to_it() {
        init_symbol_table();
        run_test(async {
            let _p = TestPlayer::new();
            let (id, g_connect) = new_connection();
            // The same datum reached from a second place (`other = gConnect`).
            let second_variable = g_connect.clone();
            // The same instance in a datum of its own (a value copy), kept inside a list.
            let in_a_list = reserve_player_mut(|player| {
                let copy = player.alloc_datum(Datum::XtraInstance("Multiuser".to_string(), id));
                player.alloc_datum(Datum::List(DatumType::List, VecDeque::from(vec![copy]), false))
            });

            drop(g_connect);
            release_unreferenced_instances();
            assert!(is_open(id), "closed with a second variable still holding it");

            drop(second_variable);
            release_unreferenced_instances();
            assert!(is_open(id), "closed with a copy still held in a list");

            drop(in_a_list);
            release_unreferenced_instances();
            assert!(!is_open(id));
        });
    }

    /// Instance ids are per Xtra, and one connection ending leaves the others alone.
    #[test]
    fn only_the_released_multiuser_instance_is_closed() {
        init_symbol_table();
        run_test(async {
            let _p = TestPlayer::new();
            let (first, first_ref) = new_connection();
            let (second, _second_ref) = new_connection();
            // Another Xtra's instance that happens to share the id is not a reference to the connection.
            let _file = reserve_player_mut(|player| player.alloc_datum(Datum::XtraInstance("FileIO".to_string(), first)));
            drop(first_ref);
            release_unreferenced_instances();
            assert!(!is_open(first));
            assert!(is_open(second));
        });
    }
}
