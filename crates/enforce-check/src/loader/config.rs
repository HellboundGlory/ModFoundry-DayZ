// A mod's config as far as loading its scripts needs it: nested classes whose
// entries are text or lists of text. Numbers are kept as written.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConfigClass {
    pub name: String,
    pub entries: Vec<(String, Value)>,
    pub classes: Vec<ConfigClass>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Text(String),
    List(Vec<Value>),
}

impl ConfigClass {
    // Config names are matched without regard to case, as the game does.
    pub fn class(&self, name: &str) -> Option<&ConfigClass> {
        self.classes.iter().find(|class| class.name.eq_ignore_ascii_case(name))
    }

    // The text items of a list entry; a single text value reads as one item.
    pub fn list(&self, name: &str) -> Vec<&str> {
        let entry = self.entries.iter().find(|(entry, _)| entry.eq_ignore_ascii_case(name));
        match entry {
            Some((_, Value::Text(text))) => vec![text.as_str()],
            Some((_, Value::List(items))) => items
                .iter()
                .filter_map(|item| match item {
                    Value::Text(text) => Some(text.as_str()),
                    Value::List(_) => None,
                })
                .collect(),
            None => Vec::new(),
        }
    }
}

// Reads a `config.cpp`. Anything it cannot make sense of is skipped up to the
// next `;`, so a config with unusual parts still yields the classes around
// them.
pub fn parse_text(text: &str) -> ConfigClass {
    let mut reader = Reader { words: words(text), at: 0, depth: 0 };
    let mut root = ConfigClass::default();
    // A `}` with nothing open does not end the file.
    while reader.peek().is_some() {
        reader.body(&mut root);
    }
    root
}

#[derive(Debug, PartialEq, Eq)]
enum Word {
    Name(String),
    Quoted(String),
    Symbol(char),
}

fn words(text: &str) -> Vec<Word> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        let c = chars[at];
        let next = chars.get(at + 1).copied();
        if c.is_whitespace() {
            at += 1;
        } else if c == '/' && next == Some('/') || c == '#' {
            // A comment or a preprocessor line runs to the end of the line.
            while at < chars.len() && chars[at] != '\n' {
                at += 1;
            }
        } else if c == '/' && next == Some('*') {
            at += 2;
            while at < chars.len() && !(chars[at] == '*' && chars.get(at + 1) == Some(&'/')) {
                at += 1;
            }
            at += 2;
        } else if c == '"' {
            let mut value = String::new();
            at += 1;
            while at < chars.len() {
                // A doubled quote is one quote inside the text.
                if chars[at] == '"' && chars.get(at + 1) == Some(&'"') {
                    value.push('"');
                    at += 2;
                } else if chars[at] == '"' {
                    break;
                } else {
                    value.push(chars[at]);
                    at += 1;
                }
            }
            at += 1;
            out.push(Word::Quoted(value));
        } else if "{}[];=,:".contains(c) {
            out.push(Word::Symbol(c));
            at += 1;
        } else {
            let start = at;
            while at < chars.len() && !chars[at].is_whitespace() && !"{}[];=,:\"".contains(chars[at]) {
                at += 1;
            }
            out.push(Word::Name(chars[start..at].iter().collect()));
        }
    }
    out
}

// Nesting deeper than any real config, so a hostile file cannot exhaust the
// stack.
const MAX_DEPTH: usize = 64;

struct Reader {
    words: Vec<Word>,
    at: usize,
    depth: usize,
}

impl Reader {
    fn peek(&self) -> Option<&Word> {
        self.words.get(self.at)
    }

    fn eat(&mut self, symbol: char) -> bool {
        let found = self.peek() == Some(&Word::Symbol(symbol));
        if found {
            self.at += 1;
        }
        found
    }

    fn name(&mut self) -> Option<String> {
        let Some(Word::Name(name)) = self.peek() else {
            return None;
        };
        let name = name.clone();
        self.at += 1;
        Some(name)
    }

    fn skip_statement(&mut self) {
        while let Some(word) = self.peek() {
            let done = matches!(word, Word::Symbol(';' | '}'));
            if *word != Word::Symbol('}') {
                self.at += 1;
            }
            if done {
                return;
            }
        }
    }

    // The entries of one class, up to its closing brace.
    fn body(&mut self, class: &mut ConfigClass) {
        while self.peek().is_some() && !self.eat('}') {
            let Some(name) = self.name() else {
                self.at += 1;
                continue;
            };
            if name == "class" {
                self.class(class);
            } else if self.eat('[') {
                self.eat(']');
                // `name[] += {..}` adds to an inherited list; the `+` before the
                // `=` is stepped over.
                while self.peek().is_some() && !self.eat('=') {
                    self.at += 1;
                }
                let list = self.list();
                class.entries.push((name, list));
                self.eat(';');
            } else if self.eat('=') {
                let value = self.scalar();
                class.entries.push((name, Value::Text(value)));
                self.eat(';');
            } else {
                self.skip_statement();
            }
        }
    }

    fn class(&mut self, parent: &mut ConfigClass) {
        let Some(name) = self.name() else {
            self.skip_statement();
            return;
        };
        let mut class = ConfigClass { name, ..ConfigClass::default() };
        if self.eat(':') {
            self.name();
        }
        if self.eat('{') {
            if self.depth >= MAX_DEPTH {
                self.words.truncate(self.at);
                return;
            }
            self.depth += 1;
            self.body(&mut class);
            self.depth -= 1;
        }
        self.eat(';');
        // A class written twice, or declared ahead of its body, is one class.
        match parent.classes.iter_mut().find(|have| have.name.eq_ignore_ascii_case(&class.name)) {
            Some(have) => {
                have.entries.extend(class.entries);
                have.classes.extend(class.classes);
            }
            None => parent.classes.push(class),
        }
    }

    fn list(&mut self) -> Value {
        let mut items = Vec::new();
        if !self.eat('{') {
            return Value::List(items);
        }
        if self.depth >= MAX_DEPTH {
            self.words.truncate(self.at);
            return Value::List(items);
        }
        self.depth += 1;
        while self.peek().is_some() && !self.eat('}') {
            if self.peek() == Some(&Word::Symbol('{')) {
                items.push(self.list());
            } else if !self.eat(',') {
                let before = self.at;
                items.push(Value::Text(self.scalar()));
                // A word no value can start with, such as a stray `;`.
                if self.at == before {
                    items.pop();
                    self.at += 1;
                }
            }
        }
        self.depth -= 1;
        Value::List(items)
    }

    // One value: quoted text, or bare words up to the next separator.
    fn scalar(&mut self) -> String {
        let mut parts: Vec<String> = Vec::new();
        while let Some(word) = self.peek() {
            match word {
                Word::Quoted(text) | Word::Name(text) => parts.push(text.clone()),
                Word::Symbol(';' | ',' | '}' | '{') => break,
                Word::Symbol(other) => parts.push(other.to_string()),
            }
            self.at += 1;
        }
        parts.join(" ")
    }
}
