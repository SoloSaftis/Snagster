//! AMF0, the value format RTMP commands and stream metadata are written in (written from Adobe's
//! public "Action Message Format -- AMF 0" specification).

/// One AMF0 value. Objects and ECMA arrays keep their keys in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Amf {
    Number(f64),
    Bool(bool),
    Str(String),
    Object(Vec<(String, Amf)>),
    Null,
    Undefined,
    EcmaArray(Vec<(String, Amf)>),
    StrictArray(Vec<Amf>),
    Date(f64),
}

impl Amf {
    pub fn str(s: &str) -> Amf {
        Amf::Str(s.to_string())
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Amf::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_number(&self) -> Option<f64> {
        match self {
            Amf::Number(n) => Some(*n),
            _ => None,
        }
    }

    /// A field of an object or ECMA array.
    pub fn get(&self, key: &str) -> Option<&Amf> {
        match self {
            Amf::Object(f) | Amf::EcmaArray(f) => f.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
}

fn put_key(out: &mut Vec<u8>, k: &str) {
    let b = k.as_bytes();
    let n = b.len().min(u16::MAX as usize);
    out.extend_from_slice(&(n as u16).to_be_bytes());
    out.extend_from_slice(&b[..n]);
}

/// Append one value.
pub fn write(out: &mut Vec<u8>, v: &Amf) {
    match v {
        Amf::Number(n) => {
            out.push(0x00);
            out.extend_from_slice(&n.to_be_bytes());
        }
        Amf::Bool(b) => {
            out.push(0x01);
            out.push(*b as u8);
        }
        Amf::Str(s) => {
            if s.len() <= u16::MAX as usize {
                out.push(0x02);
                put_key(out, s);
            } else {
                out.push(0x0C);
                out.extend_from_slice(&(s.len() as u32).to_be_bytes());
                out.extend_from_slice(s.as_bytes());
            }
        }
        Amf::Object(fields) => {
            out.push(0x03);
            for (k, v) in fields {
                put_key(out, k);
                write(out, v);
            }
            out.extend_from_slice(&[0, 0, 0x09]);
        }
        Amf::Null => out.push(0x05),
        Amf::Undefined => out.push(0x06),
        Amf::EcmaArray(fields) => {
            out.push(0x08);
            out.extend_from_slice(&(fields.len() as u32).to_be_bytes());
            for (k, v) in fields {
                put_key(out, k);
                write(out, v);
            }
            out.extend_from_slice(&[0, 0, 0x09]);
        }
        Amf::StrictArray(items) => {
            out.push(0x0A);
            out.extend_from_slice(&(items.len() as u32).to_be_bytes());
            for v in items {
                write(out, v);
            }
        }
        Amf::Date(ms) => {
            out.push(0x0B);
            out.extend_from_slice(&ms.to_be_bytes());
            out.extend_from_slice(&[0, 0]);
        }
    }
}

/// Several values one after another (a command: name, transaction id, arguments).
pub fn encode(values: &[Amf]) -> Vec<u8> {
    let mut out = Vec::new();
    for v in values {
        write(&mut out, v);
    }
    out
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        if self.at + n > self.b.len() {
            return Err("AMF value cut short".into());
        }
        let s = &self.b[self.at..self.at + n];
        self.at += n;
        Ok(s)
    }

    fn u16(&mut self) -> Result<usize, String> {
        let s = self.take(2)?;
        Ok(u16::from_be_bytes([s[0], s[1]]) as usize)
    }

    fn u32(&mut self) -> Result<usize, String> {
        let s = self.take(4)?;
        Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]) as usize)
    }

    fn f64(&mut self) -> Result<f64, String> {
        let s = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(s);
        Ok(f64::from_be_bytes(a))
    }

    fn short_str(&mut self) -> Result<String, String> {
        let n = self.u16()?;
        Ok(String::from_utf8_lossy(self.take(n)?).into_owned())
    }

    fn fields(&mut self, depth: u32) -> Result<Vec<(String, Amf)>, String> {
        let mut f = Vec::new();
        loop {
            let k = self.short_str()?;
            if k.is_empty() && self.b.get(self.at) == Some(&0x09) {
                self.at += 1;
                return Ok(f);
            }
            let v = self.value(depth + 1)?;
            f.push((k, v));
        }
    }

    fn value(&mut self, depth: u32) -> Result<Amf, String> {
        if depth > 32 {
            return Err("AMF nested too deeply".into());
        }
        let t = self.take(1)?[0];
        Ok(match t {
            0x00 => Amf::Number(self.f64()?),
            0x01 => Amf::Bool(self.take(1)?[0] != 0),
            0x02 => Amf::Str(self.short_str()?),
            0x03 => Amf::Object(self.fields(depth)?),
            0x05 => Amf::Null,
            0x06 => Amf::Undefined,
            0x08 => {
                let _count = self.u32()?;
                Amf::EcmaArray(self.fields(depth)?)
            }
            0x0A => {
                let n = self.u32()?;
                // Every item takes at least one byte.
                if n > self.b.len() - self.at {
                    return Err("AMF array longer than the message".into());
                }
                // Room for what the count says only as items actually arrive: a count of a
                // million in front of junk mustn't reserve a million values (40 bytes each).
                let mut items = Vec::with_capacity(n.min(64));
                for _ in 0..n {
                    items.push(self.value(depth + 1)?);
                }
                Amf::StrictArray(items)
            }
            0x0B => {
                let ms = self.f64()?;
                self.take(2)?;
                Amf::Date(ms)
            }
            0x0C => {
                let n = self.u32()?;
                Amf::Str(String::from_utf8_lossy(self.take(n)?).into_owned())
            }
            other => return Err(format!("AMF type {other:#04x} isn't supported")),
        })
    }
}

/// Every value in a message body.
pub fn decode_all(b: &[u8]) -> Result<Vec<Amf>, String> {
    let mut r = Reader { b, at: 0 };
    let mut out = Vec::new();
    while r.at < b.len() {
        out.push(r.value(0)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_strings_and_null_have_their_spec_bytes() {
        assert_eq!(encode(&[Amf::Number(1.0)]), vec![0, 0x3F, 0xF0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(encode(&[Amf::str("ab")]), vec![2, 0, 2, b'a', b'b']);
        assert_eq!(encode(&[Amf::Null, Amf::Bool(true)]), vec![5, 1, 1]);
    }

    #[test]
    fn objects_end_with_the_end_marker() {
        let b = encode(&[Amf::Object(vec![("a".into(), Amf::Bool(false))])]);
        assert_eq!(b, vec![3, 0, 1, b'a', 1, 0, 0, 0, 9]);
    }

    #[test]
    fn everything_reads_back() {
        let vals = vec![
            Amf::str("connect"),
            Amf::Number(1.0),
            Amf::Object(vec![("app".into(), Amf::str("live")), ("n".into(), Amf::Number(3.5)), ("inner".into(), Amf::Object(vec![("x".into(), Amf::Null)]))]),
            Amf::EcmaArray(vec![("width".into(), Amf::Number(1920.0))]),
            Amf::StrictArray(vec![Amf::Bool(true), Amf::Undefined]),
            Amf::Str("x".repeat(70_000)),
            Amf::Date(5.0),
        ];
        assert_eq!(decode_all(&encode(&vals)).unwrap(), vals);
    }

    #[test]
    fn broken_input_is_an_error_not_a_panic() {
        assert!(decode_all(&[0x00, 1, 2]).is_err());
        assert!(decode_all(&[0x02, 0, 9, b'a']).is_err());
        assert!(decode_all(&[0x0A, 0xFF, 0xFF, 0xFF, 0xFF]).is_err());
        assert!(decode_all(&[0x42]).is_err());
        let deep: Vec<u8> = std::iter::repeat_n([0x0A, 0, 0, 0, 1], 100).flatten().collect();
        assert!(decode_all(&deep).is_err());
    }

    /// A command like the ones servers send, with every kind of value.
    fn sample() -> Vec<u8> {
        encode(&[
            Amf::str("_result"),
            Amf::Number(2.0),
            Amf::Object(vec![("fmsVer".into(), Amf::str("FMS/3,0,1,123")), ("caps".into(), Amf::Number(31.0))]),
            Amf::EcmaArray(vec![("level".into(), Amf::str("status")), ("deep".into(), Amf::Object(vec![("x".into(), Amf::Null)]))]),
            Amf::StrictArray(vec![Amf::Bool(true), Amf::Undefined, Amf::Date(1.0)]),
            Amf::Str("long ".repeat(20)),
        ])
    }

    #[test]
    fn random_and_broken_messages_never_panic_or_reserve_much() {
        let good = sample();
        let mut g = crate::testalloc::Bytes::new(1);
        for i in 0..20_000 {
            let input = match i % 3 {
                0 => g.mutate(&good),
                1 => g.noise(200),
                // Valid type bytes with random lengths after them.
                _ => {
                    let mut v = vec![[0x02, 0x03, 0x08, 0x0A, 0x0C][g.below(5)]];
                    v.extend(g.noise(12));
                    v
                }
            };
            let (r, largest, _) = crate::testalloc::measure(|| decode_all(&input));
            // Re-encoding what was read gives something that reads back the same.
            // (Compared as bytes: a random number may be NaN, which never equals itself.)
            if let Ok(vals) = r {
                let again = encode(&vals);
                assert_eq!(encode(&decode_all(&again).unwrap()), again, "seed 1, input {i}");
            }
            assert!(largest <= 64 * 1024 + 64 * input.len(), "input {i} ({} bytes) reserved {largest} bytes", input.len());
        }
    }

    /// Bug: the item count of a strict array reserved room for that many values before any had
    /// been read, so a few bytes claiming a big array cost 40 bytes per claimed item.
    #[test]
    fn an_array_count_reserves_only_what_arrives() {
        let mut b = vec![0x0A];
        b.extend_from_slice(&1_000_000u32.to_be_bytes());
        b.extend(std::iter::repeat_n(0x42u8, 1_000_000)); // unsupported type: fails at the first item
        let (r, largest, _) = crate::testalloc::measure(|| decode_all(&b));
        assert!(r.is_err());
        assert!(largest < 64 * 1024, "reserved {largest} bytes");
        // A real array of that size still reads.
        let n = 100_000;
        let big = encode(&[Amf::StrictArray(vec![Amf::Null; n])]);
        assert!(matches!(&decode_all(&big).unwrap()[0], Amf::StrictArray(v) if v.len() == n));
        // A count past the end of the message is refused at once.
        assert!(decode_all(&[0x02, 0, 0, 0x0A, 0, 0, 0, 2, 5]).is_err());
    }

    #[test]
    fn get_finds_fields() {
        let o = Amf::Object(vec![("code".into(), Amf::str("NetStream.Publish.Start"))]);
        assert_eq!(o.get("code").and_then(Amf::as_str), Some("NetStream.Publish.Start"));
        assert!(o.get("nope").is_none());
        assert_eq!(Amf::Number(2.0).as_number(), Some(2.0));
    }
}
