use std::io::{self,BufRead};

pub const MAX_FRAME:usize=2*1024*1024;
#[derive(Debug,PartialEq,Eq)]
pub enum FrameError { Io, TooLarge, Invalid }

/// Returns a single newline-delimited JSON frame. Oversized input ends the connection;
/// no unbounded discard or allocation is performed while waiting for a delimiter.
pub fn read_frame<R:BufRead>(reader:&mut R)->Result<Option<serde_json::Value>,FrameError> {
    let mut bytes=Vec::new();
    loop {
        let part=reader.fill_buf().map_err(|_|FrameError::Io)?;
        if part.is_empty() {if bytes.is_empty() {return Ok(None)} return Err(FrameError::Invalid)}
        let end=part.iter().position(|b|*b==b'\n');
        let count=end.unwrap_or(part.len());
        // Permit one trailing CR beyond the JSON limit; it is a delimiter only
        // if the next byte is LF, otherwise the complete frame is rejected.
        if bytes.len().saturating_add(count)>MAX_FRAME+1 {return Err(FrameError::TooLarge)}
        bytes.extend_from_slice(&part[..count]);
        let used=count+usize::from(end.is_some()); reader.consume(used);
        if bytes.len()>MAX_FRAME && bytes.last()!=Some(&b'\r') {return Err(FrameError::TooLarge)}
        if end.is_some() {break}
    }
    if bytes.last()==Some(&b'\r') {bytes.pop();}
    if bytes.len()>MAX_FRAME {return Err(FrameError::TooLarge)}
    crate::protocol::raw::parse_strict_value(&bytes,MAX_FRAME).map(Some).map_err(|_|FrameError::Invalid)
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn fragments_crlf_unicode_and_following_frame() {
        let bytes="{\"id\":1,\"text\":\"中文\"}\r\n{\"id\":2}\n".as_bytes();
        let mut reader=io::BufReader::with_capacity(1,bytes);assert_eq!(read_frame(&mut reader).unwrap().unwrap()["text"],"中文");assert_eq!(read_frame(&mut reader).unwrap().unwrap()["id"],2);assert_eq!(read_frame(&mut reader).unwrap(),None);
    }
    #[test] fn rejects_duplicate_invalid_utf8_and_oversized_unterminated_frame() {
        for bytes in [br#"{"id":1,"\u0069d":2}
"#.as_slice(),b"\xff\n".as_slice(),b"{\"id\":1}".as_slice()] {assert_eq!(read_frame(&mut io::BufReader::new(bytes)),Err(FrameError::Invalid));}
        let bytes=vec![b' ';MAX_FRAME+1];assert_eq!(read_frame(&mut io::BufReader::new(bytes.as_slice())),Err(FrameError::TooLarge));
        let mut exact=vec![b' ';MAX_FRAME-2];exact.extend_from_slice(b"{}\n");assert!(read_frame(&mut io::BufReader::new(exact.as_slice())).unwrap().is_some());
    }
}
