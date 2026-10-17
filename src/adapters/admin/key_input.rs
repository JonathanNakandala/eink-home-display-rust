//! Reading a key from a person at a terminal or from a pipe, for `displayctl credential set`.
//!
//! A key is never taken from the command line: it would be in the shell's history and in the list of running programs,
//! where any user on the machine can read it. It is typed at a prompt that does not echo, or piped in, as from a
//! password manager: `pass show openweather | displayctl credential set open-weather`.

use std::io::{self, Read};

use secrecy::SecretString;

/// More than any key is, with room for a line break. A stream longer than this is not a key.
const MAX_BYTES: u64 = 1024;

/// The key in `input`: what is there, less the line break that ends it (`echo` and most password managers add one).
/// Anything else, a space or a second line, is left for the server to refuse, since what counts as a key depends on
/// the service.
pub fn key_from_stream(mut input: impl Read) -> io::Result<SecretString> {
    let mut bytes = Vec::new();
    // One byte more than the most there can be, to tell a key that fits from a stream that goes on.
    (&mut input).take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "that is too long to be a key; nothing was sent",
        ));
    }
    let text = String::from_utf8(bytes).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "that is not text, so it is not a key; nothing was sent",
        )
    })?;
    let key = text.strip_suffix('\n').unwrap_or(&text);
    let key = key.strip_suffix('\r').unwrap_or(key);
    if key.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "there was no key to read; nothing was sent",
        ));
    }
    Ok(SecretString::from(key.to_owned()))
}

#[cfg(test)]
mod tests {
    use secrecy::ExposeSecret;

    use super::*;

    fn read(input: &[u8]) -> io::Result<String> {
        key_from_stream(input).map(|key| key.expose_secret().to_owned())
    }

    #[test]
    fn a_key_is_read_without_the_line_break_that_ends_it() {
        assert_eq!(read(b"abc123").unwrap(), "abc123");
        assert_eq!(read(b"abc123\n").unwrap(), "abc123");
        assert_eq!(read(b"abc123\r\n").unwrap(), "abc123");
    }

    #[test]
    fn only_the_last_line_break_is_taken_so_a_second_line_is_left_for_the_server_to_refuse() {
        assert_eq!(read(b"abc\n\n").unwrap(), "abc\n");
        assert_eq!(read(b"abc\ndef\n").unwrap(), "abc\ndef");
    }

    #[test]
    fn nothing_at_all_is_not_a_key() {
        assert!(read(b"").is_err());
        assert!(read(b"\n").is_err());
        assert!(read(b"\r\n").is_err());
    }

    #[test]
    fn a_stream_that_goes_on_or_is_not_text_is_refused_without_saying_what_it_held() {
        let long = vec![b'k'; MAX_BYTES as usize + 1];
        let error = read(&long).unwrap_err().to_string();
        assert!(!error.contains("kkkk"), "{error}");
        assert!(read(&[0xff, 0xfe, 0x00]).is_err());
        assert!(read(&vec![b'k'; MAX_BYTES as usize]).is_ok());
    }
}
