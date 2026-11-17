use super::config::{ConfigClass, Value};

// Reads a binarized config (`config.bin`): a header, then class bodies that
// point at each other by offset. None when the bytes are not that format.
pub fn parse_rap(bytes: &[u8]) -> Option<ConfigClass> {
    if bytes.get(..4)? != b"\0raP" {
        return None;
    }
    let mut root = ConfigClass::default();
    // A real file is read once through; one that points many classes at the
    // same bytes runs out of budget instead of memory.
    let mut reader = Bytes { bytes, at: 16, budget: bytes.len().saturating_mul(2) };
    reader.body(&mut root, 0)?;
    Some(root)
}

// Classes nested deeper than any real config.
const MAX_DEPTH: usize = 64;

struct Bytes<'a> {
    bytes: &'a [u8],
    at: usize,
    budget: usize,
}

impl Bytes<'_> {
    fn spend(&mut self, amount: usize) -> Option<()> {
        self.budget = self.budget.checked_sub(amount)?;
        Some(())
    }

    fn byte(&mut self) -> Option<u8> {
        let byte = *self.bytes.get(self.at)?;
        self.spend(1)?;
        self.at += 1;
        Some(byte)
    }

    fn word(&mut self) -> Option<[u8; 4]> {
        let end = self.at.checked_add(4)?;
        let word = self.bytes.get(self.at..end)?.try_into().ok()?;
        self.spend(4)?;
        self.at = end;
        Some(word)
    }

    // Text up to its closing zero byte.
    fn text(&mut self) -> Option<String> {
        let rest = self.bytes.get(self.at..)?;
        let length = rest.iter().position(|byte| *byte == 0)?;
        self.spend(length + 1)?;
        let text = String::from_utf8_lossy(&rest[..length]).into_owned();
        self.at += length + 1;
        Some(text)
    }

    // A count stored seven bits to the byte, low bits first.
    fn count(&mut self) -> Option<usize> {
        let mut value = 0usize;
        for shift in 0..5 {
            let byte = self.byte()?;
            value |= usize::from(byte & 0x7f) << (7 * shift);
            if byte & 0x80 == 0 {
                return Some(value);
            }
        }
        None
    }

    fn body(&mut self, class: &mut ConfigClass, depth: usize) -> Option<()> {
        if depth > MAX_DEPTH {
            return None;
        }
        // The class this one inherits from, which loading scripts never reads.
        self.text()?;
        for _ in 0..self.count()? {
            match self.byte()? {
                0 => {
                    let mut child = ConfigClass { name: self.text()?, ..ConfigClass::default() };
                    let offset = u32::from_le_bytes(self.word()?) as usize;
                    // A body sits after the entry that names it.
                    if offset < self.at {
                        return None;
                    }
                    let resume = std::mem::replace(&mut self.at, offset);
                    self.body(&mut child, depth + 1)?;
                    self.at = resume;
                    class.classes.push(child);
                }
                1 => {
                    let kind = self.byte()?;
                    let name = self.text()?;
                    class.entries.push((name, Value::Text(self.scalar(kind)?)));
                }
                2 => {
                    let name = self.text()?;
                    class.entries.push((name, self.list(0)?));
                }
                // An outside class named, or one deleted: a name and nothing else.
                3 | 4 => {
                    self.text()?;
                }
                // A list that adds to an inherited one carries four flag bytes.
                5 => {
                    self.word()?;
                    let name = self.text()?;
                    class.entries.push((name, self.list(0)?));
                }
                _ => return None,
            }
        }
        Some(())
    }

    fn scalar(&mut self, kind: u8) -> Option<String> {
        Some(match kind {
            0 | 4 => self.text()?,
            1 => f32::from_le_bytes(self.word()?).to_string(),
            2 => i32::from_le_bytes(self.word()?).to_string(),
            _ => return None,
        })
    }

    fn list(&mut self, depth: usize) -> Option<Value> {
        if depth > MAX_DEPTH {
            return None;
        }
        let mut items = Vec::new();
        for _ in 0..self.count()? {
            let kind = self.byte()?;
            items.push(match kind {
                3 => self.list(depth + 1)?,
                _ => Value::Text(self.scalar(kind)?),
            });
        }
        Some(Value::List(items))
    }
}
