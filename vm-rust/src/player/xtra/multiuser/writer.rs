use binary_rw::{BinaryError, BinaryWriter, Endian, MemoryStream};

use crate::{
    director::static_datum::StaticDatum,
    player::xtra::multiuser::{MultiuserMessage, blowfish::MUSBlowfish, types::MusLingoValueTag},
};

pub trait MusWriter {
    fn write_mus_string(&mut self, value: &str) -> Result<usize, BinaryError>;
    fn write_mus_lingo_value(&mut self, value: &StaticDatum) -> Result<usize, BinaryError>;
}

impl MusWriter for BinaryWriter<'_> {
    fn write_mus_string(&mut self, value: &str) -> Result<usize, BinaryError> {
        let bytes = latin1_bytes(value);
        let len = bytes.len();
        let mut written = self.write_u32(len as u32)?;
        written += self.write_bytes(&bytes)?;
        if len % 2 != 0 {
            written += self.write_u8(0)?;
        }
        Ok(written)
    }

    fn write_mus_lingo_value(&mut self, value: &StaticDatum) -> Result<usize, BinaryError> {
        match value {
            StaticDatum::Void => self.write_u16(MusLingoValueTag::Void as u16),
            StaticDatum::Int(i) => {
                let mut written = self.write_u16(MusLingoValueTag::Int as u16)?;
                written += self.write_i32(*i)?;
                Ok(written)
            }
            StaticDatum::String(s) => {
                let mut written = self.write_u16(MusLingoValueTag::String as u16)?;
                written += self.write_mus_string(s)?;
                Ok(written)
            }
            StaticDatum::Symbol(name) => {
                let mut written = self.write_u16(MusLingoValueTag::Symbol as u16)?;
                written += self.write_mus_string(name)?;
                Ok(written)
            }
            StaticDatum::List(items) => {
                let mut written = self.write_u16(MusLingoValueTag::List as u16)?;
                written += self.write_u32(items.len() as u32)?;
                for item in items {
                    written += self.write_mus_lingo_value(item)?;
                }
                Ok(written)
            }
            StaticDatum::PropList(pairs) => {
                let mut written = self.write_u16(MusLingoValueTag::PropList as u16)?;
                written += self.write_u32(pairs.len() as u32)?;
                for (key, value) in pairs {
                    written += self.write_mus_lingo_value(key)?;
                    written += self.write_mus_lingo_value(value)?;
                }
                Ok(written)
            }
            StaticDatum::IntPoint(x, y) => {
                let mut written = self.write_u16(MusLingoValueTag::Point as u16)?;
                written += self.write_i32(*x)?;
                written += self.write_i32(*y)?;
                Ok(written)
            }
            StaticDatum::IntRect(l, t, r, b) => {
                let mut written = self.write_u16(MusLingoValueTag::Rect as u16)?;
                written += self.write_i32(*t)?;
                written += self.write_i32(*l)?;
                written += self.write_i32(*b)?;
                written += self.write_i32(*r)?;
                Ok(written)
            }
            StaticDatum::Media(media_data) => {
                let mut written = self.write_u16(MusLingoValueTag::Media as u16)?;
                written += self.write_u32(media_data.len() as u32)?;
                written += self.write_bytes(&media_data)?;
                if media_data.len() % 2 != 0 {
                    written += self.write_u8(0)?;
                }
                Ok(written)
            }
            _ => self.write_u16(MusLingoValueTag::Void as u16),
        }
    }
}

impl MultiuserMessage {
    pub fn to_bytes(&self, cipher: Option<&mut MUSBlowfish>) -> Vec<u8> {
        let mut payload_stream = MemoryStream::new();
        {
            let mut w = BinaryWriter::new(&mut payload_stream, Endian::Big);
            w.write_i32(self.error_code).unwrap();
            w.write_u32(self.time_stamp).unwrap();
            w.write_mus_string(&self.subject).unwrap();
            w.write_mus_string(&self.sender_id).unwrap();
            w.write_u32(self.recipients.len() as u32).unwrap();
            for r in &self.recipients {
                w.write_mus_string(r).unwrap();
            }

            let is_encrypted = self.subject == "Logon"
                && self.recipients.len() == 1
                && self.recipients[0] == "System";

            if is_encrypted {
                if let Some(cipher) = cipher {
                    let mut content_stream = MemoryStream::new();
                    {
                        let mut cw = BinaryWriter::new(&mut content_stream, Endian::Big);
                        cw.write_mus_lingo_value(&self.content).unwrap();
                    }
                    let mut content_bytes: Vec<u8> = content_stream.into();
                    cipher.apply_stream(&mut content_bytes);
                    w.write_bytes(&content_bytes).unwrap();
                } else {
                    w.write_mus_lingo_value(&self.content).unwrap();
                }
            } else {
                w.write_mus_lingo_value(&self.content).unwrap();
            }
        }
        let payload_bytes: Vec<u8> = payload_stream.into();

        let mut msg_stream = MemoryStream::new();
        {
            let mut w = BinaryWriter::new(&mut msg_stream, Endian::Big);
            w.write_u16(0x7200).unwrap();
            w.write_u32(payload_bytes.len() as u32).unwrap();
            w.write_bytes(&payload_bytes).unwrap();
        }
        msg_stream.into()
    }
}


/// The bytes of a string as the game's wire carries it: one per letter, Latin-1 (NodeJS-ROTL docs/text-encoding.md). A letter past Latin-1
/// used to be cut to its low byte (`c as u8`: a typed "\u{2019}" became the control byte 0x19, "\u{20AC}" became "\u{AC}"); it now gets the
/// same plain stand-in the server and the page use (quotes, dashes, the ellipsis, the euro), or "?". Never `"` or `\\`.
pub fn latin1_bytes(value: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(value.len());
    for c in value.chars() {
        let code = c as u32;
        if code <= 0xFF {
            out.push(code as u8);
            continue;
        }
        let stand_in: &str = match c {
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' | '\u{2032}' | '\u{02BC}' | '\u{2039}' | '\u{203A}'
            | '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' | '\u{2033}' => "'",
            '\u{2010}'..='\u{2015}' | '\u{2212}' => "-",
            '\u{2026}' => "...",
            '\u{2022}' => "*",
            '\u{20AC}' => "EUR",
            '\u{2122}' => "TM",
            '\u{0152}' => "OE",
            '\u{0153}' => "oe",
            '\u{2000}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => " ",
            '\u{200B}'..='\u{200D}' | '\u{2060}' | '\u{FEFF}' | '\u{FE0E}' | '\u{FE0F}' => "",
            _ => "?",
        };
        out.extend_from_slice(stand_in.as_bytes());
    }
    out
}

#[cfg(test)]
mod latin1_tests {
    use super::latin1_bytes;

    #[test]
    fn portuguese_is_one_byte_per_letter() {
        assert_eq!(latin1_bytes("Voc\u{EA} p\u{E3}o \u{C7}"), vec![b'V', b'o', b'c', 0xEA, b' ', b'p', 0xE3, b'o', b' ', 0xC7]);
        assert_eq!(latin1_bytes("plain ASCII"), b"plain ASCII".to_vec());
    }

    #[test]
    fn letters_past_latin1_get_a_stand_in_not_a_control_byte() {
        assert_eq!(latin1_bytes("It\u{2019}s \u{201C}ok\u{201D} \u{2014} 5\u{20AC}\u{2026}"), b"It's 'ok' - 5EUR...".to_vec());
        assert_eq!(latin1_bytes("\u{65E5}\u{1F600}"), b"??".to_vec());
    }
}
