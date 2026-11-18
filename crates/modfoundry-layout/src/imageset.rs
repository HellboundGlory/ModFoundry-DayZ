//! Read a `.imageset` file: its textures and where each image sits in them.

use std::fmt;

/// An image set: the textures it is cut from and the images in them.
#[derive(Debug, Clone, PartialEq)]
pub struct Imageset {
    pub name: String,
    /// The unit `Pos` and `Size` are written in, not texels.
    pub ref_size: (u32, u32),
    pub textures: Vec<ImagesetTexture>,
    /// In file order.
    pub images: Vec<ImagesetImage>,
}

/// One texture of a set; `mpix` is the quality tier (0, 1 or 2).
#[derive(Debug, Clone, PartialEq)]
pub struct ImagesetTexture {
    pub mpix: u32,
    /// Without the `{GUID}` prefix.
    pub path: String,
}

/// One image of a set, positioned in `ref_size` units.
#[derive(Debug, Clone, PartialEq)]
pub struct ImagesetImage {
    pub name: String,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    /// As written, such as `0`, `3` or `ISHorizontalTile`; empty when missing.
    pub flags: String,
}

/// The first problem in an imageset text.
#[derive(Debug, Clone, PartialEq)]
pub struct ImagesetError {
    pub line: usize,
    pub message: String,
}

impl fmt::Display for ImagesetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ImagesetError {}

/// Reads the text of an `.imageset` file.
pub fn parse_imageset(text: &str) -> Result<Imageset, ImagesetError> {
    let root = read_tree(text.strip_prefix('\u{feff}').unwrap_or(text))?;
    let set = match root.blocks.first() {
        Some(set) if set.header == "ImageSetClass" => set,
        Some(other) => return Err(error(other.line, "the first block must be ImageSetClass")),
        None => return Err(error(1, "no ImageSetClass block")),
    };
    Ok(Imageset {
        name: set.text("Name").unwrap_or_default(),
        ref_size: ref_size(set)?,
        textures: blocks_in(set, "Textures", "ImageSetTextureClass")
            .into_iter()
            .map(texture)
            .collect::<Result<_, _>>()?,
        images: blocks_in(set, "Images", "ImageSetDefClass")
            .into_iter()
            .chain(grouped_images(set))
            .map(image)
            .collect::<Result<_, _>>()?,
    })
}

/// How an image repeats to fill a larger box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tile {
    None,
    Horizontal,
    Vertical,
    Both,
}

impl ImagesetImage {
    /// From the words `ISHorizontalTile` and `ISVerticalTile` in `flags`; numbers and anything else give `Tile::None`.
    pub fn tile(&self) -> Tile {
        let has = |word: &str| {
            self.flags
                .split(|c: char| c.is_whitespace() || c == '|' || c == ',')
                .any(|flag| flag == word)
        };
        match (has("ISHorizontalTile"), has("ISVerticalTile")) {
            (true, true) => Tile::Both,
            (true, false) => Tile::Horizontal,
            (false, true) => Tile::Vertical,
            (false, false) => Tile::None,
        }
    }
}

impl Imageset {
    /// The image called `name`.
    pub fn image(&self, name: &str) -> Option<&ImagesetImage> {
        self.images.iter().find(|image| image.name == name)
    }

    /// The texture with the highest `mpix`; the engine draws style slices from it.
    pub fn best_texture(&self) -> Option<&ImagesetTexture> {
        self.textures.iter().max_by_key(|texture| texture.mpix)
    }

    /// Texel size per RefSize unit for a texture of `texture_width` x `texture_height` pixels.
    pub fn texel_scale(&self, texture_width: u32, texture_height: u32) -> (f32, f32) {
        (
            texture_width as f32 / self.ref_size.0 as f32,
            texture_height as f32 / self.ref_size.1 as f32,
        )
    }
}

fn error(line: usize, message: &str) -> ImagesetError {
    ImagesetError {
        line,
        message: message.to_owned(),
    }
}

fn ref_size(set: &Block) -> Result<(u32, u32), ImagesetError> {
    let prop = set.required("RefSize")?;
    let size = pair(prop)?;
    if size.0 == 0 || size.1 == 0 {
        return Err(error(prop.line, "RefSize has a zero side"));
    }
    Ok(size)
}

fn texture(block: &Block) -> Result<ImagesetTexture, ImagesetError> {
    let mpix = single(block.required("mpix")?)?;
    let path_prop = block.required("path")?;
    let path = strip_guid(path_prop.values.first().map_or("", String::as_str));
    if path.is_empty() {
        return Err(error(path_prop.line, "path is empty"));
    }
    Ok(ImagesetTexture {
        mpix,
        path: path.to_owned(),
    })
}

fn image(block: &Block) -> Result<ImagesetImage, ImagesetError> {
    let name = block
        .text("Name")
        .or_else(|| block.args.first().cloned())
        .ok_or_else(|| error(block.line, "image has no name"))?;
    let (x, y) = pair(block.required("Pos")?)?;
    let (w, h) = pair(block.required("Size")?)?;
    Ok(ImagesetImage {
        name,
        x,
        y,
        w,
        h,
        flags: block
            .prop("Flags")
            .map(|prop| prop.values.join(" "))
            .unwrap_or_default(),
    })
}

fn blocks_in<'a>(set: &'a Block, group: &str, class: &str) -> Vec<&'a Block> {
    set.blocks
        .iter()
        .filter(|block| block.header == group)
        .flat_map(|block| block.blocks.iter())
        .filter(|block| block.header == class)
        .collect()
}

fn grouped_images(set: &Block) -> impl Iterator<Item = &Block> {
    set.blocks
        .iter()
        .filter(|block| block.header == "Groups")
        .flat_map(|groups| groups.blocks.iter())
        .filter(|group| group.header == "ImageSetGroupClass")
        .flat_map(|group| blocks_in(group, "Images", "ImageSetDefClass"))
}

fn single(prop: &Prop) -> Result<u32, ImagesetError> {
    match prop.values.as_slice() {
        [value] => value
            .parse()
            .map_err(|_| error(prop.line, &format!("{} is not a whole number", prop.key))),
        _ => Err(error(prop.line, &format!("{} needs one number", prop.key))),
    }
}

fn pair(prop: &Prop) -> Result<(u32, u32), ImagesetError> {
    let numbers: Vec<u32> = prop
        .values
        .iter()
        .map(|value| value.parse())
        .collect::<Result<_, _>>()
        .map_err(|_| error(prop.line, &format!("{} is not a whole number", prop.key)))?;
    match numbers.as_slice() {
        [a, b] => Ok((*a, *b)),
        _ => Err(error(prop.line, &format!("{} needs two numbers", prop.key))),
    }
}

/// Drop a `{16 hex digits}` prefix; the engine finds the file without it.
fn strip_guid(path: &str) -> &str {
    match path.strip_prefix('{').and_then(|rest| rest.split_once('}')) {
        Some((id, rest)) if id.len() == 16 && id.bytes().all(|b| b.is_ascii_hexdigit()) => rest,
        _ => path,
    }
}

#[derive(Debug)]
struct Block {
    header: String,
    args: Vec<String>,
    line: usize,
    props: Vec<Prop>,
    blocks: Vec<Block>,
}

#[derive(Debug)]
struct Prop {
    key: String,
    values: Vec<String>,
    line: usize,
}

impl Block {
    fn root() -> Self {
        Block {
            header: String::new(),
            args: Vec::new(),
            line: 0,
            props: Vec::new(),
            blocks: Vec::new(),
        }
    }

    fn prop(&self, key: &str) -> Option<&Prop> {
        self.props.iter().find(|prop| prop.key == key)
    }

    fn required(&self, key: &str) -> Result<&Prop, ImagesetError> {
        self.prop(key)
            .ok_or_else(|| error(self.line, &format!("{key} is missing")))
    }

    fn text(&self, key: &str) -> Option<String> {
        self.prop(key)?
            .values
            .first()
            .filter(|value| !value.is_empty())
            .cloned()
    }
}

const MAX_DEPTH: usize = 64;

fn read_tree(text: &str) -> Result<Block, ImagesetError> {
    let mut stack = vec![Block::root()];
    for (index, source) in text.lines().enumerate() {
        let line = index + 1;
        let tokens = tokenize(source, line)?;
        match tokens.as_slice() {
            [] => {}
            [Token::Close] => {
                if stack.len() == 1 {
                    return Err(error(line, "} with no block to close"));
                }
                let block = stack.pop().expect("a block above the root");
                top(&mut stack).blocks.push(block);
            }
            [.., Token::Open] => {
                if stack.len() > MAX_DEPTH {
                    return Err(error(line, "blocks nest too deeply"));
                }
                stack.push(open_block(&tokens[..tokens.len() - 1], line)?);
            }
            _ => top(&mut stack).props.push(property(&tokens, line)?),
        }
    }
    if stack.len() > 1 {
        return Err(error(stack[1].line, "block is not closed"));
    }
    Ok(stack.pop().expect("the root block"))
}

fn top(stack: &mut [Block]) -> &mut Block {
    stack.last_mut().expect("the root block")
}

fn open_block(tokens: &[Token], line: usize) -> Result<Block, ImagesetError> {
    let Some((class, args)) = tokens.split_first() else {
        return Err(error(line, "block has no class name"));
    };
    let header = class
        .text()
        .ok_or_else(|| error(line, "block has no class name"))?;
    let args = values(args).ok_or_else(|| error(line, "unexpected brace"))?;
    Ok(Block {
        header: header.to_owned(),
        args,
        line,
        props: Vec::new(),
        blocks: Vec::new(),
    })
}

fn property(tokens: &[Token], line: usize) -> Result<Prop, ImagesetError> {
    let Some((key, rest)) = tokens.split_first() else {
        return Err(error(line, "property has no key"));
    };
    let key = key.text().ok_or_else(|| error(line, "unexpected brace"))?;
    let values = values(rest).ok_or_else(|| error(line, "unexpected brace"))?;
    Ok(Prop {
        key: key.to_owned(),
        values,
        line,
    })
}

fn values(tokens: &[Token]) -> Option<Vec<String>> {
    tokens
        .iter()
        .map(|token| token.text().map(str::to_owned))
        .collect()
}

#[derive(Debug, PartialEq)]
enum Token {
    Word(String),
    Quoted(String),
    Open,
    Close,
}

impl Token {
    fn text(&self) -> Option<&str> {
        match self {
            Token::Word(text) | Token::Quoted(text) => Some(text),
            Token::Open | Token::Close => None,
        }
    }
}

fn tokenize(source: &str, line: usize) -> Result<Vec<Token>, ImagesetError> {
    let mut tokens = Vec::new();
    let mut rest = source;
    loop {
        rest = rest.trim_start();
        if rest.is_empty() || rest.starts_with("//") {
            return Ok(tokens);
        }
        if let Some(after) = rest.strip_prefix('{') {
            tokens.push(Token::Open);
            rest = after;
        } else if let Some(after) = rest.strip_prefix('}') {
            tokens.push(Token::Close);
            rest = after;
        } else if let Some(quoted) = rest.strip_prefix('"') {
            let (text, after) =
                quoted_text(quoted).ok_or_else(|| error(line, "unterminated string"))?;
            tokens.push(Token::Quoted(text));
            rest = after;
        } else {
            let end = rest
                .find(|c: char| c.is_whitespace() || matches!(c, '{' | '}' | '"'))
                .unwrap_or(rest.len());
            let end = rest[..end].find("//").unwrap_or(end);
            tokens.push(Token::Word(rest[..end].to_owned()));
            rest = &rest[end..];
        }
    }
}

/// The text up to the closing quote, with `\"` and `\\` unescaped, and what follows it.
fn quoted_text(quoted: &str) -> Option<(String, &str)> {
    let mut text = String::new();
    let mut chars = quoted.char_indices();
    while let Some((at, c)) = chars.next() {
        match c {
            '"' => return Some((text, &quoted[at + 1..])),
            '\\' => match chars.next() {
                Some((_, escaped @ ('"' | '\\'))) => text.push(escaped),
                Some((_, other)) => {
                    text.push('\\');
                    text.push(other);
                }
                None => text.push('\\'),
            },
            _ => text.push(c),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{ImagesetImage, Tile};

    #[test]
    fn tile_reads_the_tiling_words_in_flags() {
        let cases = [
            ("ISHorizontalTile", Tile::Horizontal),
            ("ISVerticalTile", Tile::Vertical),
            ("ISHorizontalTile|ISVerticalTile", Tile::Both),
            ("0", Tile::None),
            ("3", Tile::None),
            ("", Tile::None),
            ("ISUnknownWord", Tile::None),
        ];
        for (flags, tile) in cases {
            let image = ImagesetImage {
                name: "image".to_string(),
                x: 0,
                y: 0,
                w: 1,
                h: 1,
                flags: flags.to_string(),
            };
            assert_eq!(image.tile(), tile, "flags {flags:?}");
        }
    }
}
